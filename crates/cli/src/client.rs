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
    #[error("sbtd sent something that is not its protocol: {0}")]
    Protocol(String),
}

impl ClientError {
    /// Lines the user should see under the message, when there are any.
    #[must_use]
    pub fn details(&self) -> &[String] {
        match self {
            Self::Api { body, .. } => body.output.as_deref().unwrap_or(&[]),
            Self::Unreachable { .. } | Self::Protocol(_) => &[],
        }
    }

    #[must_use]
    pub fn hint(&self) -> Option<&'static str> {
        match self {
            Self::Unreachable { .. } => Some("start it with `sbtctl daemon start`"),
            Self::Api { .. } | Self::Protocol(_) => None,
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
        let body = match serde_json::from_str(&text) {
            Ok(body) => body,
            Err(_) => ErrorResponse {
                code: ErrorCode::Internal,
                message: text,
                output: None,
            },
        };
        Err(ClientError::Api {
            status: status.as_u16(),
            body,
        })
    }
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
