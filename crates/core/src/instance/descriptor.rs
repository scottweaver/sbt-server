//! sbt's connection descriptor: `project/target/active.json` names the
//! server's socket and, for TCP servers, a token file. Over sbt's default
//! local transport there is no token at all (gotcha 2).
//!
//! The descriptor is untrusted input: a cloned repository can ship one. It
//! is parsed into [`ActiveJson`] only when a TCP endpoint is loopback and a
//! token file lies under `~/.sbt`, which is where sbt itself binds and
//! writes; anything else is refused before a socket is opened or a token
//! file is read.

use std::ffi::OsString;
use std::fmt;
use std::io;
use std::net::SocketAddr;
use std::path::{Component, Path, PathBuf};

use serde::Deserialize;
use thiserror::Error;

/// A TCP address on the loopback interface; sbt's TCP server binds
/// `127.0.0.1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoopbackSocket(SocketAddr);

impl LoopbackSocket {
    /// # Errors
    ///
    /// `NonLoopbackHost` for any address outside the loopback range,
    /// including the unspecified `0.0.0.0` and `::`.
    pub fn parse(addr: SocketAddr) -> Result<Self, DescriptorError> {
        if addr.ip().to_canonical().is_loopback() {
            Ok(Self(addr))
        } else {
            Err(DescriptorError::NonLoopbackHost { addr })
        }
    }

    #[must_use]
    pub fn socket_addr(self) -> SocketAddr {
        self.0
    }
}

/// The two transports sbt publishes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerUri {
    Tcp(LoopbackSocket),
    Local(PathBuf),
}

impl ServerUri {
    /// Parse `tcp://host:port` or `local:///absolute/path`.
    ///
    /// # Errors
    ///
    /// `NonLoopbackHost` for a TCP address off the loopback interface;
    /// `UnsupportedUri` for any other scheme, a host that is not an IP
    /// literal, or a relative local path (Windows named pipes are
    /// `local:name` and unsupported).
    pub fn parse(uri: &str) -> Result<Self, DescriptorError> {
        let unsupported = || DescriptorError::UnsupportedUri {
            uri: uri.to_owned(),
        };
        if let Some(addr) = uri.strip_prefix("tcp://") {
            let addr = addr.parse().map_err(|_| unsupported())?;
            return LoopbackSocket::parse(addr).map(Self::Tcp);
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
            Self::Tcp(addr) => write!(f, "tcp://{}", addr.socket_addr()),
            Self::Local(path) => write!(f, "local://{}", path.display()),
        }
    }
}

/// `$HOME/.sbt`, the only directory a token file may come from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SbtUserDir(PathBuf);

#[derive(Debug, Error, PartialEq, Eq)]
pub enum SbtUserDirError {
    #[error("HOME is not set; cannot locate ~/.sbt, where sbt keeps its server tokens")]
    NoHome,
    #[error("HOME must be an absolute path without `..`, got {0}")]
    UnusableHome(PathBuf),
}

impl SbtUserDir {
    /// The directory the current environment's `HOME` designates.
    ///
    /// # Errors
    ///
    /// See [`SbtUserDir::resolve`].
    pub fn from_env() -> Result<Self, SbtUserDirError> {
        Self::resolve(std::env::var_os("HOME"))
    }

    /// Pure core of [`SbtUserDir::from_env`]: `home/.sbt`.
    ///
    /// # Errors
    ///
    /// `NoHome` when `home` is unset or empty, `UnusableHome` when it is
    /// relative or climbs with `..`.
    pub fn resolve(home: Option<OsString>) -> Result<Self, SbtUserDirError> {
        let home = PathBuf::from(
            home.filter(|h| !h.is_empty())
                .ok_or(SbtUserDirError::NoHome)?,
        );
        if is_plain_absolute(&home) {
            Ok(Self(home.join(".sbt")))
        } else {
            Err(SbtUserDirError::UnusableHome(home))
        }
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.0
    }
}

/// A token file path that lies strictly inside [`SbtUserDir`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenFile(PathBuf);

impl TokenFile {
    /// The check is lexical: an absolute path with no `..` that starts with
    /// `sbt_dir` component-wise.
    ///
    /// # Errors
    ///
    /// `UntrustedTokenFile` for anything else.
    pub fn parse(path: PathBuf, sbt_dir: &SbtUserDir) -> Result<Self, DescriptorError> {
        if is_plain_absolute(&path) && path.starts_with(sbt_dir.path()) && path != sbt_dir.path() {
            Ok(Self(path))
        } else {
            Err(DescriptorError::UntrustedTokenFile {
                path,
                sbt_dir: sbt_dir.path().to_path_buf(),
            })
        }
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.0
    }
}

fn is_plain_absolute(path: &Path) -> bool {
    path.is_absolute()
        && path
            .components()
            .all(|c| !matches!(c, Component::ParentDir | Component::CurDir))
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
    #[error("refusing server address {addr}: sbt servers listen on loopback only")]
    NonLoopbackHost { addr: SocketAddr },
    #[error(
        "refusing token file {path}: sbt writes its tokens under {sbt_dir}",
        path = path.display(),
        sbt_dir = sbt_dir.display()
    )]
    UntrustedTokenFile { path: PathBuf, sbt_dir: PathBuf },
}

impl DescriptorError {
    /// True when the descriptor simply is not there yet.
    #[must_use]
    pub fn is_not_found(&self) -> bool {
        match self {
            Self::Io { source, .. } => source.kind() == io::ErrorKind::NotFound,
            Self::Json { .. }
            | Self::UnsupportedUri { .. }
            | Self::NonLoopbackHost { .. }
            | Self::UntrustedTokenFile { .. } => false,
        }
    }

    /// True when the descriptor was well formed but names an endpoint or a
    /// token file sbt itself would never write.
    #[must_use]
    pub fn is_refusal(&self) -> bool {
        match self {
            Self::NonLoopbackHost { .. } | Self::UntrustedTokenFile { .. } => true,
            Self::Io { .. } | Self::Json { .. } | Self::UnsupportedUri { .. } => false,
        }
    }
}

/// What `active.json` says, validated, before the token file is read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveJson {
    pub uri: ServerUri,
    pub token_file: Option<TokenFile>,
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
/// Malformed JSON, a missing `uri`, an unsupported or non-loopback uri, or
/// a token file outside `sbt_dir`.
pub fn parse_active_json(
    path: &Path,
    text: &str,
    sbt_dir: &SbtUserDir,
) -> Result<ActiveJson, DescriptorError> {
    let raw: RawActiveJson =
        serde_json::from_str(text).map_err(|source| DescriptorError::Json {
            path: path.to_path_buf(),
            source,
        })?;
    Ok(ActiveJson {
        uri: ServerUri::parse(&raw.uri)?,
        token_file: raw
            .tokenfile_path
            .map(|token_path| TokenFile::parse(token_path, sbt_dir))
            .transpose()?,
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

/// Read and validate `active.json` and, when it names one, the token file.
///
/// # Errors
///
/// Either file is unreadable or malformed, or the descriptor is refused;
/// `is_not_found` tells a not-yet-written descriptor from a broken one and
/// `is_refusal` an untrusted one.
pub async fn read(
    active_json: &Path,
    sbt_dir: &SbtUserDir,
) -> Result<ConnectionInfo, DescriptorError> {
    let text = read_text(active_json).await?;
    let parsed = parse_active_json(active_json, &text, sbt_dir)?;
    let token = match parsed.token_file {
        Some(token_file) => Some(parse_token_file(
            token_file.path(),
            &read_text(token_file.path()).await?,
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

    fn sbt_dir() -> SbtUserDir {
        SbtUserDir::resolve(Some("/u".into())).unwrap()
    }

    fn parse(text: &str) -> Result<ActiveJson, DescriptorError> {
        parse_active_json(Path::new(PATH), text, &sbt_dir())
    }

    fn loopback(addr: &str) -> ServerUri {
        ServerUri::Tcp(LoopbackSocket::parse(addr.parse().unwrap()).unwrap())
    }

    #[test]
    fn local_descriptor_has_a_unix_socket_and_no_token_file() {
        let parsed =
            parse(r#"{"uri":"local:///Users/u/.sbt/1.0/server/0845deda85cb41abdb9f/sock"}"#)
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
        let parsed = parse(
            r#"{"uri":"tcp://127.0.0.1:5010","tokenfilePath":"/u/.sbt/1.0/server/x/token.json","tokenfileUri":"file:///u/.sbt/1.0/server/x/token.json"}"#,
        )
        .unwrap();
        assert_eq!(parsed.uri, loopback("127.0.0.1:5010"));
        assert_eq!(
            parsed.token_file.as_ref().map(TokenFile::path),
            Some(Path::new("/u/.sbt/1.0/server/x/token.json"))
        );
    }

    #[test]
    fn loopback_tcp_addresses_are_accepted() {
        for uri in [
            "tcp://127.0.0.1:5010",
            "tcp://127.1.2.3:5010",
            "tcp://[::1]:5010",
            "tcp://[::ffff:127.0.0.1]:5010",
        ] {
            assert!(
                matches!(ServerUri::parse(uri), Ok(ServerUri::Tcp(_))),
                "{uri}"
            );
        }
    }

    #[test]
    fn non_loopback_tcp_addresses_are_refused() {
        for uri in [
            "tcp://10.0.0.5:5010",
            "tcp://203.0.113.7:5010",
            "tcp://0.0.0.0:5010",
            "tcp://[::]:5010",
            "tcp://[2001:db8::1]:5010",
            "tcp://[::ffff:10.0.0.5]:5010",
        ] {
            let error = ServerUri::parse(uri).unwrap_err();
            assert!(
                matches!(error, DescriptorError::NonLoopbackHost { .. }),
                "{uri}: {error}"
            );
            assert!(error.is_refusal(), "{uri}");
            assert!(error.to_string().contains("loopback"), "{error}");
        }
    }

    #[test]
    fn token_files_outside_the_sbt_dir_are_refused() {
        for path in [
            "/etc/passwd",
            "/u/.ssh/id_ed25519",
            "/u/.sbtx/token.json",
            "/u/.sbt",
            "/u/.sbt/../.ssh/id_ed25519",
            "u/.sbt/token.json",
            ".sbt/token.json",
        ] {
            let text = serde_json::json!({ "uri": "tcp://127.0.0.1:5010", "tokenfilePath": path });
            let error = parse(&text.to_string()).unwrap_err();
            assert!(
                matches!(error, DescriptorError::UntrustedTokenFile { .. }),
                "{path}: {error}"
            );
            assert!(error.is_refusal(), "{path}");
            assert!(error.to_string().contains("/u/.sbt"), "{error}");
        }
    }

    #[test]
    fn sbt_dir_needs_an_absolute_home() {
        assert_eq!(sbt_dir().path(), Path::new("/u/.sbt"));
        assert_eq!(SbtUserDir::resolve(None), Err(SbtUserDirError::NoHome));
        assert_eq!(
            SbtUserDir::resolve(Some(OsString::new())),
            Err(SbtUserDirError::NoHome)
        );
        for home in ["home/u", "/home/../u"] {
            assert!(
                matches!(
                    SbtUserDir::resolve(Some(home.into())),
                    Err(SbtUserDirError::UnusableHome(_))
                ),
                "{home}"
            );
        }
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
            assert!(!error.is_refusal(), "{uri}");
        }
    }

    #[test]
    fn malformed_json_names_the_file() {
        let error = parse("{").unwrap_err();
        assert!(error.to_string().starts_with(PATH), "{error}");
        assert!(!error.is_not_found());
    }

    #[tokio::test]
    async fn read_distinguishes_missing_from_broken() {
        let dir = tempfile::tempdir().unwrap();
        let sbt_dir = SbtUserDir::resolve(Some(dir.path().into())).unwrap();
        let path = dir.path().join("active.json");
        let error = read(&path, &sbt_dir).await.unwrap_err();
        assert!(error.is_not_found(), "{error}");

        let missing_token = sbt_dir.path().join("1.0/server/x/token.json");
        std::fs::write(
            &path,
            serde_json::json!({ "uri": "tcp://127.0.0.1:1", "tokenfilePath": missing_token })
                .to_string(),
        )
        .unwrap();
        let error = read(&path, &sbt_dir).await.unwrap_err();
        assert!(
            error.is_not_found(),
            "token file missing counts as not found: {error}"
        );

        std::fs::write(&path, r#"{"uri":"tcp://127.0.0.1:1"}"#).unwrap();
        let info = read(&path, &sbt_dir).await.unwrap();
        assert_eq!(info.token, None);
        assert_eq!(info.uri.to_string(), "tcp://127.0.0.1:1");
    }

    #[tokio::test]
    async fn read_refuses_before_opening_an_outside_token_file() {
        let dir = tempfile::tempdir().unwrap();
        let sbt_dir = SbtUserDir::resolve(Some(dir.path().join("home").into())).unwrap();
        let secret = dir.path().join("secret.json");
        std::fs::write(&secret, r#"{"token":"not-for-you"}"#).unwrap();
        let path = dir.path().join("active.json");
        std::fs::write(
            &path,
            serde_json::json!({ "uri": "tcp://127.0.0.1:1", "tokenfilePath": secret }).to_string(),
        )
        .unwrap();
        let error = read(&path, &sbt_dir).await.unwrap_err();
        assert!(
            matches!(error, DescriptorError::UntrustedTokenFile { .. }),
            "{error}"
        );
    }
}
