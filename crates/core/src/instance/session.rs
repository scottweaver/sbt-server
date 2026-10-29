//! sbt's protocol on top of the generic JSON-RPC client, as a typestate: a
//! connection is [`Unauthenticated`] until `initialize` succeeds, and only an
//! [`SbtSession`] can run commands.

use std::fmt;
use std::num::NonZeroI32;

use serde_json::{Value, json};
use thiserror::Error;

use super::descriptor::Token;
use crate::jsonrpc::{Connection, RequestError, RpcError};

/// A non-empty sbt command line, exactly as a user would type it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CommandLine(String);

#[derive(Debug, Error, PartialEq, Eq)]
#[error("sbt command line is empty")]
pub struct EmptyCommandLine;

impl CommandLine {
    /// # Errors
    ///
    /// The text is empty or whitespace.
    pub fn new(text: impl Into<String>) -> Result<Self, EmptyCommandLine> {
        let text = text.into();
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Err(EmptyCommandLine);
        }
        Ok(Self(trimmed.to_owned()))
    }

    /// sbt's own `shutdown` command: the only way to stop a server over the
    /// protocol, since there is no `sbt/shutdown` method (gotcha 4).
    #[must_use]
    pub fn shutdown() -> Self {
        Self("shutdown".to_owned())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for CommandLine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A nonzero exit code sbt reported for a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExitCode(NonZeroI32);

impl ExitCode {
    #[must_use]
    pub fn new(code: i32) -> Option<Self> {
        NonZeroI32::new(code).map(Self)
    }

    #[must_use]
    pub fn get(self) -> i32 {
        self.0.get()
    }
}

impl fmt::Display for ExitCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// How sbt answered an `sbt/exec`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecOutcome {
    Succeeded,
    /// sbt sent an error response for the command: `code` is its exit code
    /// and `message` sbt's own description of the failure, often empty.
    Failed {
        code: ExitCode,
        message: String,
    },
    /// sbt sent an error that is not an exit code (an internal failure).
    Internal(RpcError),
}

impl ExecOutcome {
    /// The process exit code a CLI should propagate.
    #[must_use]
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::Succeeded => 0,
            Self::Failed { code, .. } => code.get(),
            Self::Internal(_) => 1,
        }
    }

    /// sbt routes a zero or absent `exitCode` to a result response and any
    /// nonzero one to an error response whose `code` is that exit code; an
    /// error code outside the process exit range is an internal failure.
    #[must_use]
    pub fn from_response(response: Result<Value, RpcError>) -> Self {
        match response {
            Ok(result) => result
                .get("exitCode")
                .and_then(Value::as_i64)
                .and_then(|code| i32::try_from(code).ok())
                .and_then(ExitCode::new)
                .map_or(Self::Succeeded, |code| Self::Failed {
                    code,
                    message: String::new(),
                }),
            Err(error) => match exit_code_from_error_code(error.code) {
                Some(code) => Self::Failed {
                    code,
                    message: error.message,
                },
                None => Self::Internal(error),
            },
        }
    }
}

/// sbt's `ErrorCodes.UnknownError`: sent for an unknown command, with an
/// empty message (captured live).
const SBT_UNKNOWN_ERROR: i64 = -33000;
/// JSON-RPC `InternalError`: what sbt sends when a task fails, with the
/// failure text as the message, e.g. `(Compile / compileIncremental)
/// Compilation failed` (captured live).
const JSON_RPC_INTERNAL_ERROR: i64 = -32603;

fn exit_code_from_error_code(code: i64) -> Option<ExitCode> {
    if code == SBT_UNKNOWN_ERROR || code == JSON_RPC_INTERNAL_ERROR {
        return ExitCode::new(1);
    }
    u8::try_from(code)
        .ok()
        .and_then(|c| ExitCode::new(i32::from(c)))
}

#[derive(Debug, Error)]
pub enum HandshakeError {
    #[error("sbt rejected initialize: {0}")]
    Rejected(RpcError),
    #[error("connection to sbt closed during initialize")]
    ConnectionClosed,
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
#[error("connection to sbt closed")]
pub struct ConnectionClosed;

/// A connection that has not completed `initialize`.
#[derive(Debug, Clone)]
pub struct Unauthenticated(Connection);

impl Unauthenticated {
    #[must_use]
    pub fn new(connection: Connection) -> Self {
        Self(connection)
    }

    /// Run sbt's `initialize` handshake, with the token when the transport
    /// requires one, then `sbt/attach` as a non-interactive client. Until a
    /// client attaches, the channel's terminal is sbt's `NullTerminal` and
    /// everything written to it (test-framework output, for one) is lost;
    /// attaching installs a terminal that forwards it as `sbt/systemOut`.
    ///
    /// # Errors
    ///
    /// sbt rejected the token or the attach, or the socket closed first.
    pub async fn initialize(self, token: Option<&Token>) -> Result<SbtSession, HandshakeError> {
        self.request("initialize", initialize_params(token)).await?;
        self.request("sbt/attach", attach_params()).await?;
        Ok(SbtSession(self.0))
    }

    async fn request(&self, method: &str, params: Value) -> Result<Value, HandshakeError> {
        self.0
            .request(method, params)
            .await
            .map_err(|error| match error {
                RequestError::Rpc(rpc) => HandshakeError::Rejected(rpc),
                RequestError::ConnectionClosed => HandshakeError::ConnectionClosed,
            })
    }
}

/// Non-interactive, like sbtn running a batch command: sbt then never asks
/// us for stdin or shows a prompt, but still forwards the terminal.
#[must_use]
pub fn attach_params() -> Value {
    json!({ "interactive": false })
}

#[must_use]
pub fn initialize_params(token: Option<&Token>) -> Value {
    match token {
        Some(token) => json!({ "initializationOptions": { "token": token.as_str() } }),
        None => json!({ "initializationOptions": {} }),
    }
}

/// An initialized connection: the only thing that can run commands.
#[derive(Debug, Clone)]
pub struct SbtSession(Connection);

impl SbtSession {
    /// Run one command line and wait for sbt's terminal reply.
    ///
    /// # Errors
    ///
    /// Only when the socket closes before sbt answers; sbt's own failures
    /// are [`ExecOutcome`]s.
    pub async fn exec(&self, command: &CommandLine) -> Result<ExecOutcome, ConnectionClosed> {
        let params = json!({ "commandLine": command.as_str() });
        match self.0.request("sbt/exec", params).await {
            Ok(result) => Ok(ExecOutcome::from_response(Ok(result))),
            Err(RequestError::Rpc(error)) => Ok(ExecOutcome::from_response(Err(error))),
            Err(RequestError::ConnectionClosed) => Err(ConnectionClosed),
        }
    }

    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.0.is_closed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rpc(code: i64) -> RpcError {
        RpcError {
            code,
            message: String::new(),
            data: None,
        }
    }

    fn failed(code: i32, message: &str) -> ExecOutcome {
        ExecOutcome::Failed {
            code: ExitCode::new(code).unwrap(),
            message: message.to_owned(),
        }
    }

    #[test]
    fn command_lines_are_trimmed_and_non_empty() {
        assert_eq!(CommandLine::new("  compile ").unwrap().as_str(), "compile");
        assert_eq!(CommandLine::new("   "), Err(EmptyCommandLine));
        assert_eq!(CommandLine::shutdown().as_str(), "shutdown");
    }

    #[test]
    fn outcomes_follow_sbts_response_routing() {
        let done = json!({ "status": "Done", "exitCode": 0 });
        assert_eq!(ExecOutcome::from_response(Ok(done)), ExecOutcome::Succeeded);
        assert_eq!(
            ExecOutcome::from_response(Ok(json!({ "status": "Done" }))),
            ExecOutcome::Succeeded
        );
        assert_eq!(ExecOutcome::from_response(Err(rpc(1))), failed(1, ""));
        let compile_failed = RpcError {
            code: -32603,
            message: "(Compile / compileIncremental) Compilation failed".into(),
            data: None,
        };
        assert_eq!(
            ExecOutcome::from_response(Err(compile_failed)),
            failed(1, "(Compile / compileIncremental) Compilation failed")
        );
        assert_eq!(
            ExecOutcome::from_response(Err(rpc(-32600))),
            ExecOutcome::Internal(rpc(-32600))
        );
        assert_eq!(ExecOutcome::from_response(Err(rpc(-33000))), failed(1, ""));
        assert_eq!(ExecOutcome::Internal(rpc(-32600)).exit_code(), 1);
        assert_eq!(failed(7, "").exit_code(), 7);
    }

    #[test]
    fn initialize_params_carry_the_token_only_when_present() {
        assert_eq!(
            initialize_params(None),
            json!({ "initializationOptions": {} })
        );
        let token = super::super::descriptor::parse_token_file(
            std::path::Path::new("/t"),
            r#"{"token":"42"}"#,
        )
        .unwrap();
        assert_eq!(
            initialize_params(Some(&token)),
            json!({ "initializationOptions": { "token": "42" } })
        );
    }
}
