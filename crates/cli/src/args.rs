//! The command line. Anything that is not one of sbtctl's own subcommands
//! is an sbt command line passed through verbatim, so sbtctl's flags must
//! come before it: `sbtctl -C proj --verbose testOnly com.ex.FooSpec`.

use std::ffi::OsString;
use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "sbtctl",
    version,
    about = "Run sbt commands against the warm sbt kept by sbtd",
    long_about = "Run sbt commands against the warm sbt kept by sbtd.\n\n\
                  Any words that are not a subcommand below are sent to sbt as one \
                  command line, e.g. `sbtctl compile` or `sbtctl testOnly com.ex.FooSpec`. \
                  sbtctl's own flags must precede them."
)]
pub struct Cli {
    /// Run as if started in this directory instead of the current one.
    #[arg(short = 'C', long = "directory", global = true, value_name = "DIR")]
    pub directory: Option<PathBuf>,

    /// Where sbtd listens.
    #[arg(
        long,
        global = true,
        env = "SBTCTL_DAEMON_URL",
        default_value = "http://127.0.0.1:8077",
        value_name = "URL"
    )]
    pub daemon_url: String,

    /// Where sbtd keeps its pid and log files; defaults to the XDG config dir.
    #[arg(long, global = true, env = "SBTD_STATE_DIR", value_name = "DIR")]
    pub state_dir: Option<PathBuf>,

    /// Keep sbt's debug-level log lines.
    #[arg(long, global = true)]
    pub verbose: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// List the warm sbt instances.
    Status,
    /// Stop the warm sbt for this build root, or every one with --all.
    Shutdown {
        #[arg(long)]
        all: bool,
    },
    /// Manage the sbtd daemon itself.
    Daemon {
        #[command(subcommand)]
        action: DaemonAction,
    },
    /// An sbt command line, passed through verbatim.
    #[command(external_subcommand)]
    Sbt(Vec<OsString>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Subcommand)]
pub enum DaemonAction {
    /// Start sbtd detached and wait until it answers /health.
    Start,
    /// Send sbtd SIGTERM and wait until /health stops answering.
    Stop,
    /// Stop (if running) then start.
    Restart,
    /// Report whether sbtd answers /health.
    Status,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_after_the_flags_are_the_sbt_command_line() {
        let cli = Cli::parse_from([
            "sbtctl",
            "-C",
            "/p",
            "--verbose",
            "testOnly",
            "com.ex.FooSpec",
        ]);
        assert_eq!(cli.directory.as_deref(), Some(std::path::Path::new("/p")));
        assert!(cli.verbose);
        let Command::Sbt(words) = cli.command else {
            panic!("expected passthrough")
        };
        assert_eq!(words, ["testOnly", "com.ex.FooSpec"]);
    }

    #[test]
    fn own_subcommands_take_precedence() {
        assert!(matches!(
            Cli::parse_from(["sbtctl", "shutdown", "--all"]).command,
            Command::Shutdown { all: true }
        ));
        assert!(matches!(
            Cli::parse_from(["sbtctl", "daemon", "restart"]).command,
            Command::Daemon {
                action: DaemonAction::Restart
            }
        ));
        assert_eq!(
            Cli::parse_from(["sbtctl", "status"]).daemon_url,
            "http://127.0.0.1:8077"
        );
    }
}
