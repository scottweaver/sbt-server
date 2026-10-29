//! sbt's connection descriptor: `project/target/active.json` names the
//! server's socket and, for TCP servers, a token file. Over sbt's default
//! local transport there is no token at all (gotcha 2).

use std::fmt;
use std::io;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use thiserror::Error;

/// The two transports sbt publishes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerUri {
    Tcp(SocketAddr),
    Local(PathBuf),
}

impl ServerUri {
    /// Parse `tcp://host:port` or `local:///absolute/path`.
    ///
    /// # Errors
    ///
    /// Any other scheme, a host that is not an IP literal, or a relative
    /// local path (Windows named pipes are `local:name` and unsupported).
    pub fn parse(uri: &str) -> Result<Self, DescriptorError> {
        let unsupported = || DescriptorError::UnsupportedUri {
            uri: uri.to_owned(),
        };
        if let Some(addr) = uri.strip_prefix("tcp://") {
            return addr.parse().map(Self::Tcp).map_err(|_| unsupported());
        }
        if let Some(path) = uri.strip_prefix("local://")
            && path.starts_with('/')
        {
            return Ok(Self::Local(PathBuf::from(path)));
        }
        Err(unsupported())
    }
}

impl fmt::Display for ServerUri {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Tcp(addr) => write!(f, "tcp://{addr}"),
            Self::Local(path) => write!(f, "local://{}", path.display()),
        }
    }
}

/// The authentication token a TCP server expects in `initialize`.
#[derive(Clone, PartialEq, Eq)]
pub struct Token(String);

impl Token {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Token(<redacted>)")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionInfo {
    pub uri: ServerUri,
    pub token: Option<Token>,
}

#[derive(Debug, Error)]
pub enum DescriptorError {
    #[error("cannot read {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("{path} is not valid JSON: {source}")]
    Json {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("unsupported server uri {uri:?} (expected tcp://host:port or local:///path)")]
    UnsupportedUri { uri: String },
}

impl DescriptorError {
    /// True when the descriptor simply is not there yet.
    #[must_use]
    pub fn is_not_found(&self) -> bool {
        match self {
            Self::Io { source, .. } => source.kind() == io::ErrorKind::NotFound,
            Self::Json { .. } | Self::UnsupportedUri { .. } => false,
        }
    }
}

/// What `active.json` says, before the token file is read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveJson {
    pub uri: ServerUri,
    pub token_file: Option<PathBuf>,
}

#[derive(Deserialize)]
struct RawActiveJson {
    uri: String,
    #[serde(default, rename = "tokenfilePath")]
    tokenfile_path: Option<PathBuf>,
}

#[derive(Deserialize)]
struct RawTokenFile {
    token: String,
}

/// Parse the text of `active.json`.
///
/// # Errors
///
/// Malformed JSON, a missing `uri`, or an unsupported uri.
pub fn parse_active_json(path: &Path, text: &str) -> Result<ActiveJson, DescriptorError> {
    let raw: RawActiveJson =
        serde_json::from_str(text).map_err(|source| DescriptorError::Json {
            path: path.to_path_buf(),
            source,
        })?;
    Ok(ActiveJson {
        uri: ServerUri::parse(&raw.uri)?,
        token_file: raw.tokenfile_path,
    })
}

/// Parse the text of the token file `active.json` points at.
///
/// # Errors
///
/// Malformed JSON or a missing `token`.
pub fn parse_token_file(path: &Path, text: &str) -> Result<Token, DescriptorError> {
    let raw: RawTokenFile = serde_json::from_str(text).map_err(|source| DescriptorError::Json {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(Token(raw.token))
}

/// Read `active.json` and, when it names one, the token file.
///
/// # Errors
///
/// Either file is unreadable or malformed; `is_not_found` tells a
/// not-yet-written descriptor from a broken one.
pub async fn read(active_json: &Path) -> Result<ConnectionInfo, DescriptorError> {
    let text = read_text(active_json).await?;
    let parsed = parse_active_json(active_json, &text)?;
    let token = match parsed.token_file {
        Some(token_path) => Some(parse_token_file(
            &token_path,
            &read_text(&token_path).await?,
        )?),
        None => None,
    };
    Ok(ConnectionInfo {
        uri: parsed.uri,
        token,
    })
}

async fn read_text(path: &Path) -> Result<String, DescriptorError> {
    tokio::fs::read_to_string(path)
        .await
        .map_err(|source| DescriptorError::Io {
            path: path.to_path_buf(),
            source,
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    const PATH: &str = "/b/project/target/active.json";

    #[test]
    fn local_descriptor_has_a_unix_socket_and_no_token_file() {
        let parsed = parse_active_json(
            Path::new(PATH),
            r#"{"uri":"local:///Users/u/.sbt/1.0/server/0845deda85cb41abdb9f/sock"}"#,
        )
        .unwrap();
        assert_eq!(
            parsed.uri,
            ServerUri::Local(PathBuf::from(
                "/Users/u/.sbt/1.0/server/0845deda85cb41abdb9f/sock"
            ))
        );
        assert_eq!(parsed.token_file, None);
    }

    #[test]
    fn tcp_descriptor_names_a_token_file() {
        let parsed = parse_active_json(
            Path::new(PATH),
            r#"{"uri":"tcp://127.0.0.1:5010","tokenfilePath":"/u/.sbt/1.0/server/x/token.json","tokenfileUri":"file:///u/.sbt/1.0/server/x/token.json"}"#,
        )
        .unwrap();
        assert_eq!(
            parsed.uri,
            ServerUri::Tcp("127.0.0.1:5010".parse().unwrap())
        );
        assert_eq!(
            parsed.token_file.as_deref(),
            Some(Path::new("/u/.sbt/1.0/server/x/token.json"))
        );
    }

    #[test]
    fn token_file_yields_a_redacted_token() {
        let token = parse_token_file(
            Path::new("/t/token.json"),
            r#"{"uri":"tcp://127.0.0.1:5010","token":"84046191245433876643612047032303751629"}"#,
        )
        .unwrap();
        assert_eq!(token.as_str(), "84046191245433876643612047032303751629");
        assert_eq!(format!("{token:?}"), "Token(<redacted>)");
    }

    #[test]
    fn unsupported_uris_are_rejected() {
        for uri in [
            "local:sbt-server-0845deda85cb41abdb9f",
            "tcp://localhost:5010",
            "http://127.0.0.1:5010",
            "local://relative/sock",
        ] {
            let error = ServerUri::parse(uri).unwrap_err();
            assert!(
                matches!(error, DescriptorError::UnsupportedUri { .. }),
                "{uri}: {error}"
            );
        }
    }

    #[test]
    fn malformed_json_names_the_file() {
        let error = parse_active_json(Path::new(PATH), "{").unwrap_err();
        assert!(error.to_string().starts_with(PATH), "{error}");
        assert!(!error.is_not_found());
    }

    #[tokio::test]
    async fn read_distinguishes_missing_from_broken() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("active.json");
        let error = read(&path).await.unwrap_err();
        assert!(error.is_not_found(), "{error}");

        std::fs::write(
            &path,
            r#"{"uri":"tcp://127.0.0.1:1","tokenfilePath":"/nope/token.json"}"#,
        )
        .unwrap();
        let error = read(&path).await.unwrap_err();
        assert!(
            error.is_not_found(),
            "token file missing counts as not found: {error}"
        );

        std::fs::write(&path, r#"{"uri":"tcp://127.0.0.1:1"}"#).unwrap();
        let info = read(&path).await.unwrap();
        assert_eq!(info.token, None);
        assert_eq!(info.uri.to_string(), "tcp://127.0.0.1:1");
    }
}
