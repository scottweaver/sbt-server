//! How sbtctl prints exec events and turns the terminal event into a
//! process exit code.
//!
//! sbt's exit code passes through, clamped to 1..=124 when nonzero, and
//! 125 is reserved for the tool itself failing (an unreachable daemon, an
//! aborted exec, a protocol error), so "my build is broken" and "the tool
//! is broken" never collide.

use std::io::Write;

use sbt_server_core::protocol::{ExecEvent, LogLevel, OutputStream};

/// The docker/git convention for "the tool failed, not the command".
pub const TOOL_FAILURE: u8 = 125;
const MAX_BUILD_EXIT: i32 = 124;

/// Print one event the way sbt's own shell would have: logs to stdout
/// with their level tag, raw output to the stream it came from, the
/// summary last. Diagnostics are not printed: sbt already logs every
/// compiler message as an `error`/`warning` line, and the structured form
/// is for HTTP consumers.
///
/// # Errors
///
/// The underlying write failed (a closed pipe, typically).
pub fn render(
    event: &ExecEvent,
    stdout: &mut impl Write,
    stderr: &mut impl Write,
) -> std::io::Result<()> {
    match event {
        ExecEvent::Started {
            build_root,
            cold_start: true,
            ..
        } => writeln!(
            stderr,
            "[sbtctl] starting sbt for {} (cold start)",
            build_root.display()
        ),
        ExecEvent::Started {
            cold_start: false, ..
        }
        | ExecEvent::Diagnostics { .. } => Ok(()),
        ExecEvent::Log { level, text } => writeln!(stdout, "[{}] {text}", level_tag(*level)),
        ExecEvent::Output {
            stream: OutputStream::Stdout | OutputStream::SbtStdout,
            text,
        } => writeln!(stdout, "{text}"),
        ExecEvent::Output {
            stream: OutputStream::Stderr | OutputStream::SbtStderr,
            text,
        } => writeln!(stderr, "{text}"),
        ExecEvent::Done { summary, .. } => writeln!(stdout, "{summary}"),
        ExecEvent::Aborted { summary, .. } => writeln!(stderr, "[sbtctl] {summary}"),
    }
}

fn level_tag(level: LogLevel) -> &'static str {
    match level {
        LogLevel::Debug => "debug",
        LogLevel::Info => "info",
        LogLevel::Warning => "warn",
        LogLevel::Error => "error",
    }
}

/// The process exit code for an exec that ended with `terminal`.
#[must_use]
pub fn exit_code(terminal: Option<&ExecEvent>) -> u8 {
    match terminal {
        Some(ExecEvent::Done { exit_code, .. }) => clamp_build_exit(*exit_code),
        Some(
            ExecEvent::Aborted { .. }
            | ExecEvent::Started { .. }
            | ExecEvent::Log { .. }
            | ExecEvent::Output { .. }
            | ExecEvent::Diagnostics { .. },
        )
        | None => TOOL_FAILURE,
    }
}

fn clamp_build_exit(code: i32) -> u8 {
    if code == 0 {
        0
    } else {
        u8::try_from(code.clamp(1, MAX_BUILD_EXIT)).unwrap_or(1)
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use sbt_server_core::protocol::{AbortReason, Outcome};

    use super::*;

    fn rendered(event: &ExecEvent) -> (String, String) {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        render(event, &mut out, &mut err).unwrap();
        (
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    }

    #[test]
    fn exit_codes_pass_through_clamped_and_reserve_125() {
        let done = |code| ExecEvent::done(code, 1, String::new());
        assert_eq!(exit_code(Some(&done(0))), 0);
        assert_eq!(exit_code(Some(&done(3))), 3);
        assert_eq!(exit_code(Some(&done(124))), 124);
        assert_eq!(exit_code(Some(&done(125))), 124);
        assert_eq!(exit_code(Some(&done(300))), 124);
        assert_eq!(exit_code(Some(&done(-7))), 1);
        let aborted = ExecEvent::Aborted {
            reason: AbortReason::SbtDied,
            summary: String::new(),
        };
        assert_eq!(exit_code(Some(&aborted)), TOOL_FAILURE);
        assert_eq!(exit_code(None), TOOL_FAILURE);
    }

    #[test]
    fn events_go_to_the_stream_sbt_would_have_used() {
        let (out, err) = rendered(&ExecEvent::Log {
            level: LogLevel::Warning,
            text: "careful".into(),
        });
        assert_eq!((out.as_str(), err.as_str()), ("[warn] careful\n", ""));

        let (out, err) = rendered(&ExecEvent::Output {
            stream: OutputStream::SbtStderr,
            text: "boom".into(),
        });
        assert_eq!((out.as_str(), err.as_str()), ("", "boom\n"));

        let (out, err) = rendered(&ExecEvent::Started {
            build_root: PathBuf::from("/p"),
            pid: 1,
            cold_start: true,
        });
        assert_eq!(out, "");
        assert_eq!(err, "[sbtctl] starting sbt for /p (cold start)\n");

        let (out, _) = rendered(&ExecEvent::Done {
            outcome: Outcome::Success,
            exit_code: 0,
            elapsed_ms: 5,
            summary: "[success] 'compile' completed in 5ms".into(),
        });
        assert_eq!(out, "[success] 'compile' completed in 5ms\n");

        let (out, err) = rendered(&ExecEvent::Diagnostics {
            file: PathBuf::from("/p/A.scala"),
            items: Vec::new(),
        });
        assert_eq!((out.as_str(), err.as_str()), ("", ""));
    }
}
