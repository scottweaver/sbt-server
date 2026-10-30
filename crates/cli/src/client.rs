//! HTTP client for sbtd's contract (`sbt_server_core::protocol`).

use sbt_server_core::protocol::{
    ErrorCode, ErrorResponse, ExecEvent, ExecRequest, HealthResponse, ShutdownRequest,
    ShutdownResponse, StatusResponse,
};
use serde::de::DeserializeOwned;
use thiserror::Error;
use tokio_stream::StreamExt;

#[derive(Debug, Error)]
pub enum ClientError {
    #[error("cannot reach sbtd at {url}: {source}")]
    Unreachable {
        url: String,
        #[source]
        source: reqwest::Error,
    },
    #[error("sbtd answered {status}: {}", body.message)]
    Api { status: u16, body: ErrorResponse },
    #[error("the server at {url} answered {status} but does not speak sbtd's protocol: {excerpt}")]
    NotSbtd {
        url: String,
        status: u16,
        excerpt: String,
    },
    #[error("sbtd sent something that is not its protocol: {0}")]
    Protocol(String),
}

impl ClientError {
    /// Lines the user should see under the message, when there are any.
    #[must_use]
    pub fn details(&self) -> &[String] {
        match self {
            Self::Api { body, .. } => body.output.as_deref().unwrap_or(&[]),
            Self::Unreachable { .. } | Self::NotSbtd { .. } | Self::Protocol(_) => &[],
        }
    }

    #[must_use]
    pub fn hint(&self) -> Option<&'static str> {
        match self {
            Self::Unreachable { .. } => Some("start it with `sbtctl daemon start`"),
            Self::Api { body, .. } => match body.code {
                ErrorCode::NoBuildRoot => {
                    Some("run this inside an sbt project, or pass `-C <dir>`")
                }
                ErrorCode::ProjectLoadFailed => {
                    Some("fix what the lines above report, then run the command again")
                }
                ErrorCode::Forbidden => Some(
                    "sbtd answers only when addressed as 127.0.0.1, [::1] or localhost; check `--daemon-url` (or SBTCTL_DAEMON_URL)",
                ),
                ErrorCode::StartFailed | ErrorCode::BadRequest | ErrorCode::Internal => None,
            },
            Self::NotSbtd { .. } | Self::Protocol(_) => Some(
                "check `--daemon-url` (or SBTCTL_DAEMON_URL); something other than sbtd may be listening there",
            ),
        }
    }
}

pub struct DaemonClient {
    base: String,
    http: reqwest::Client,
}

impl DaemonClient {
    #[must_use]
    pub fn new(base_url: &str) -> Self {
        Self {
            base: base_url.trim_end_matches('/').to_owned(),
            http: reqwest::Client::new(),
        }
    }

    #[must_use]
    pub fn base_url(&self) -> &str {
        &self.base
    }

    /// # Errors
    ///
    /// sbtd is unreachable or answered outside its contract.
    pub async fn health(&self) -> Result<HealthResponse, ClientError> {
        self.get("/health").await
    }

    /// # Errors
    ///
    /// sbtd is unreachable or answered outside its contract.
    pub async fn status(&self) -> Result<StatusResponse, ClientError> {
        self.get("/status").await
    }

    /// # Errors
    ///
    /// sbtd is unreachable, refused the request, or answered outside its
    /// contract.
    pub async fn shutdown(
        &self,
        request: &ShutdownRequest,
    ) -> Result<ShutdownResponse, ClientError> {
        let response = self
            .send(self.http.post(self.url("/shutdown")).json(request))
            .await?;
        decode(response).await
    }

    /// Stream the exec's events to `on_event` as they arrive; returns the
    /// terminal event, or `None` if the stream ended without one.
    ///
    /// # Errors
    ///
    /// sbtd is unreachable, refused the request (no build root, project
    /// failed to load), or sent a line that is not an event.
    pub async fn exec(
        &self,
        request: &ExecRequest,
        mut on_event: impl FnMut(&ExecEvent),
    ) -> Result<Option<ExecEvent>, ClientError> {
        let response = self
            .send(self.http.post(self.url("/exec")).json(request))
            .await?;
        let mut stream = response.bytes_stream();
        let mut buffer = Vec::new();
        let mut terminal = None;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|source| self.unreachable(source))?;
            buffer.extend_from_slice(&chunk);
            while let Some(newline) = buffer.iter().position(|b| *b == b'\n') {
                let line: Vec<u8> = buffer.drain(..=newline).collect();
                let event = parse_event(&line[..line.len() - 1])?;
                on_event(&event);
                if event.is_terminal() {
                    terminal = Some(event);
                }
            }
        }
        Ok(terminal)
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    fn unreachable(&self, source: reqwest::Error) -> ClientError {
        ClientError::Unreachable {
            url: self.base.clone(),
            source,
        }
    }

    async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T, ClientError> {
        let response = self.send(self.http.get(self.url(path))).await?;
        decode(response).await
    }

    async fn send(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<reqwest::Response, ClientError> {
        let response = request
            .send()
            .await
            .map_err(|source| self.unreachable(source))?;
        let status = response.status();
        if status.is_success() {
            return Ok(response);
        }
        let text = response.text().await.unwrap_or_default();
        match serde_json::from_str::<ErrorResponse>(&text) {
            Ok(body) => Err(ClientError::Api {
                status: status.as_u16(),
                body,
            }),
            Err(_) => Err(ClientError::NotSbtd {
                url: self.base.clone(),
                status: status.as_u16(),
                excerpt: excerpt(&text),
            }),
        }
    }
}

/// The first line of a body, cut to one screen width, for an error message.
fn excerpt(text: &str) -> String {
    let line = text.lines().next().unwrap_or("").trim();
    if line.is_empty() {
        return "(empty body)".to_owned();
    }
    let mut cut: String = line.chars().take(120).collect();
    if cut.len() < line.len() {
        cut.push_str("...");
    }
    cut
}

async fn decode<T: DeserializeOwned>(response: reqwest::Response) -> Result<T, ClientError> {
    let text = response
        .text()
        .await
        .map_err(|error| ClientError::Protocol(error.to_string()))?;
    serde_json::from_str(&text).map_err(|error| ClientError::Protocol(format!("{error}: {text}")))
}

fn parse_event(line: &[u8]) -> Result<ExecEvent, ClientError> {
    serde_json::from_slice(line).map_err(|error| {
        ClientError::Protocol(format!("{error}: {}", String::from_utf8_lossy(line)))
    })
}
