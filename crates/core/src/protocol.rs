//! The agent-facing HTTP contract between `sbtctl` and `sbtd`.
//!
//! These DTOs are the wire shape; richer internal types are parsed from or
//! rendered into them at the daemon and CLI boundaries. Free text always
//! travels in a field named `text`, and every `/exec` stream ends with exactly
//! one terminal event (`done` or `aborted`).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// `GET /health`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthResponse {
    pub status: HealthStatus,
    pub pid: u32,
    pub version: String,
    pub uptime_secs: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthStatus {
    Ok,
}

/// `GET /status`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusResponse {
    pub instances: Vec<InstanceStatus>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstanceStatus {
    pub build_root: PathBuf,
    /// Absent while the instance is still being spawned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    pub phase: InstancePhase,
    /// Present only while `phase` is `idle`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idle_secs: Option<u64>,
    pub uptime_secs: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstancePhase {
    Booting,
    Idle,
    Running,
}

/// `POST /exec`; the response is an NDJSON stream of [`ExecEvent`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecRequest {
    pub cwd: PathBuf,
    pub command_line: String,
    /// Keep sbt's debug-level log notifications instead of dropping them.
    #[serde(default)]
    pub verbose: bool,
}

/// `POST /shutdown`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "target", rename_all = "snake_case")]
pub enum ShutdownRequest {
    One { cwd: PathBuf },
    All,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShutdownResponse {
    pub stopped: Vec<PathBuf>,
}

/// Any non-2xx body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorResponse {
    pub code: ErrorCode,
    pub message: String,
    /// Boot output when the project failed to load, so the agent sees why.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<Vec<String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    NoBuildRoot,
    ProjectLoadFailed,
    StartFailed,
    BadRequest,
    Forbidden,
    Internal,
}

/// One line of the `/exec` NDJSON stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ExecEvent {
    /// Always first. `cold_start` tells a 60 s JVM boot from a slow compile.
    Started {
        build_root: PathBuf,
        pid: u32,
        cold_start: bool,
    },
    /// A `build/logMessage` from sbt's own logger.
    Log { level: LogLevel, text: String },
    /// One whole line of process output.
    Output { stream: OutputStream, text: String },
    /// Compiler diagnostics for one file; an empty `items` means it is clean now.
    Diagnostics {
        file: PathBuf,
        items: Vec<Diagnostic>,
    },
    /// Terminal: sbt answered. `summary` is the synthesized completion line.
    Done {
        outcome: Outcome,
        exit_code: i32,
        elapsed_ms: u64,
        summary: String,
    },
    /// Terminal: sbt never answered; never carries a fake exit code.
    Aborted {
        reason: AbortReason,
        summary: String,
    },
}

impl ExecEvent {
    /// Build a `done` event, deriving `outcome` from the exit code.
    #[must_use]
    pub fn done(exit_code: i32, elapsed_ms: u64, summary: String) -> Self {
        let outcome = if exit_code == 0 {
            Outcome::Success
        } else {
            Outcome::Failure
        };
        Self::Done {
            outcome,
            exit_code,
            elapsed_ms,
            summary,
        }
    }

    #[must_use]
    pub fn is_terminal(&self) -> bool {
        match self {
            Self::Done { .. } | Self::Aborted { .. } => true,
            Self::Started { .. }
            | Self::Log { .. }
            | Self::Output { .. }
            | Self::Diagnostics { .. } => false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogLevel {
    Debug,
    Info,
    Warning,
    Error,
}

/// `stdout`/`stderr` are the JVM's raw pipes; `sbt_*` are sbt's forwarded
/// `sbt/systemOut` and `sbt/systemErr` notifications.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputStream {
    Stdout,
    Stderr,
    SbtStdout,
    SbtStderr,
}

/// 1-based `line` and `column`, as agents and editors count them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostic {
    pub line: u32,
    pub column: u32,
    pub severity: Severity,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Error,
    Warning,
    Information,
    Hint,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Success,
    Failure,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AbortReason {
    SbtDied,
    ConnectionLost,
    DaemonShutdown,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(event: &ExecEvent) -> ExecEvent {
        let line = serde_json::to_string(event).unwrap();
        assert!(!line.contains('\n'), "NDJSON lines must be single-line");
        serde_json::from_str(&line).unwrap()
    }

    #[test]
    fn exec_events_round_trip_and_use_the_type_tag() {
        let events = [
            ExecEvent::Started {
                build_root: PathBuf::from("/p"),
                pid: 7,
                cold_start: true,
            },
            ExecEvent::Log {
                level: LogLevel::Warning,
                text: "careful".into(),
            },
            ExecEvent::Output {
                stream: OutputStream::SbtStderr,
                text: "boom".into(),
            },
            ExecEvent::Diagnostics {
                file: PathBuf::from("/p/A.scala"),
                items: vec![Diagnostic {
                    line: 3,
                    column: 5,
                    severity: Severity::Error,
                    text: "not found".into(),
                }],
            },
            ExecEvent::done(0, 12, "[success] 'compile' completed in 12ms".into()),
            ExecEvent::Aborted {
                reason: AbortReason::SbtDied,
                summary: "sbt exited".into(),
            },
        ];
        for event in &events {
            assert_eq!(&round_trip(event), event);
        }
        let json = serde_json::to_value(&events[1]).unwrap();
        assert_eq!(json["type"], "log");
        assert_eq!(json["level"], "warning");
    }

    #[test]
    fn done_derives_outcome_from_exit_code() {
        let ExecEvent::Done { outcome, .. } = ExecEvent::done(1, 0, String::new()) else {
            panic!("expected Done")
        };
        assert_eq!(outcome, Outcome::Failure);
        assert!(ExecEvent::done(0, 0, String::new()).is_terminal());
    }

    #[test]
    fn shutdown_request_is_tagged_by_target() {
        let all = serde_json::to_value(ShutdownRequest::All).unwrap();
        assert_eq!(all, serde_json::json!({ "target": "all" }));
        let one: ShutdownRequest =
            serde_json::from_value(serde_json::json!({ "target": "one", "cwd": "/x" })).unwrap();
        assert_eq!(
            one,
            ShutdownRequest::One {
                cwd: PathBuf::from("/x")
            }
        );
    }

    #[test]
    fn exec_request_verbose_defaults_to_false() {
        let request: ExecRequest =
            serde_json::from_str(r#"{"cwd":"/x","command_line":"compile"}"#).unwrap();
        assert!(!request.verbose);
    }
}
