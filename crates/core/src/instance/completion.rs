//! The completion line sbt never sends to a client (gotcha 6): sbt skips its
//! `[success] Total time: ...` banner for JSON-RPC callers, so the instance
//! synthesizes an equivalent from the command, its outcome and the elapsed
//! time.

use std::time::Duration;

use super::session::{CommandLine, ExecOutcome};

/// `[success] 'compile' completed in 1.2s` or
/// `[error] 'compile' failed (exit code 1) in 0.8s`.
#[must_use]
pub fn render(command: &CommandLine, outcome: &ExecOutcome, elapsed: Duration) -> String {
    let elapsed = format_elapsed(elapsed);
    match outcome {
        ExecOutcome::Succeeded => format!("[success] '{command}' completed in {elapsed}"),
        ExecOutcome::Failed { code, message } if message.is_empty() => {
            format!("[error] '{command}' failed (exit code {code}) in {elapsed}")
        }
        ExecOutcome::Failed { code, message } => {
            format!("[error] '{command}' failed (exit code {code}) in {elapsed}: {message}")
        }
        ExecOutcome::Internal(error) => {
            format!("[error] '{command}' failed (sbt {error}) in {elapsed}")
        }
    }
}

/// `812ms` below a second, `1.2s` below a minute, `2m 05s` beyond.
#[must_use]
pub fn format_elapsed(elapsed: Duration) -> String {
    let secs = elapsed.as_secs();
    if secs == 0 {
        format!("{}ms", elapsed.as_millis())
    } else if secs < 60 {
        format!("{:.1}s", elapsed.as_secs_f64())
    } else {
        format!("{}m {:02}s", secs / 60, secs % 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::instance::session::ExitCode;
    use crate::jsonrpc::RpcError;

    #[test]
    fn success_and_failure_lines() {
        let compile = CommandLine::new("compile").unwrap();
        assert_eq!(
            render(
                &compile,
                &ExecOutcome::Succeeded,
                Duration::from_millis(1234)
            ),
            "[success] 'compile' completed in 1.2s"
        );
        let failed = ExecOutcome::Failed {
            code: ExitCode::new(1).unwrap(),
            message: String::new(),
        };
        assert_eq!(
            render(&compile, &failed, Duration::from_millis(812)),
            "[error] 'compile' failed (exit code 1) in 812ms"
        );
        let explained = ExecOutcome::Failed {
            code: ExitCode::new(1).unwrap(),
            message: "(Compile / compileIncremental) Compilation failed".into(),
        };
        assert_eq!(
            render(&compile, &explained, Duration::from_millis(1105)),
            "[error] 'compile' failed (exit code 1) in 1.1s: (Compile / compileIncremental) Compilation failed"
        );
        let internal = ExecOutcome::Internal(RpcError {
            code: -33000,
            message: "boom".into(),
            data: None,
        });
        assert_eq!(
            render(&compile, &internal, Duration::from_secs(125)),
            "[error] 'compile' failed (sbt error -33000: boom) in 2m 05s"
        );
    }
}
