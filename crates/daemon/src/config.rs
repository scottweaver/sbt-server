//! Command line and environment for `sbtd`. The bind address is a
//! [`LoopbackAddr`]: the agent transport invariant (loopback HTTP only) is
//! enforced by the type, so a non-loopback address cannot even be parsed.

use std::fmt;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::str::FromStr;
use std::time::Duration;

use clap::Parser;
use sbt_server_core::instance::{Launcher, SbtConfig, Timeouts};
use sbt_server_core::paths::{PathsError, StateDir};
use sbt_server_core::registry::RegistryConfig;
use thiserror::Error;

/// A socket address whose IP is a loopback address.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoopbackAddr(SocketAddr);

impl LoopbackAddr {
    #[must_use]
    pub fn socket_addr(self) -> SocketAddr {
        self.0
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum BindError {
    #[error("{0:?} is not a socket address (expected host:port)")]
    Unparseable(String),
    #[error("{0} is not a loopback address; agents may only reach sbtd over loopback HTTP")]
    NotLoopback(SocketAddr),
}

impl FromStr for LoopbackAddr {
    type Err = BindError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let addr: SocketAddr = text
            .parse()
            .map_err(|_| BindError::Unparseable(text.to_owned()))?;
        if addr.ip().is_loopback() {
            Ok(Self(addr))
        } else {
            Err(BindError::NotLoopback(addr))
        }
    }
}

impl fmt::Display for LoopbackAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Debug, Parser)]
#[command(
    name = "sbtd",
    version,
    about = "Keeps one warm sbt server per build root for sbtctl"
)]
pub struct Cli {
    /// Loopback address to listen on; port 0 picks a free port.
    #[arg(long, env = "SBTD_BIND", default_value = "127.0.0.1:8077")]
    pub bind: LoopbackAddr,

    /// Shut down instances idle for this many seconds; 0 keeps them warm forever.
    #[arg(long, env = "SBTD_IDLE_TIMEOUT_SECS", default_value_t = 7200)]
    pub idle_timeout_secs: u64,

    /// The sbt launcher to spawn.
    #[arg(long, env = "SBTD_SBT_PROGRAM", default_value = "sbt")]
    pub sbt_program: PathBuf,

    /// Seconds to wait for a fresh sbt to publish its server descriptor.
    #[arg(long, env = "SBTD_BOOT_TIMEOUT_SECS", default_value_t = 180)]
    pub boot_timeout_secs: u64,

    /// Where the pid and address files live; defaults to the XDG config dir.
    #[arg(long, env = "SBTD_STATE_DIR")]
    pub state_dir: Option<PathBuf>,
}

impl Cli {
    #[must_use]
    pub fn registry_config(&self) -> RegistryConfig {
        RegistryConfig {
            sbt: SbtConfig {
                launcher: Launcher {
                    program: self.sbt_program.clone(),
                    ..Launcher::default()
                },
                timeouts: Timeouts {
                    boot: Duration::from_secs(self.boot_timeout_secs),
                    ..Timeouts::default()
                },
            },
            idle_timeout: (self.idle_timeout_secs > 0)
                .then(|| Duration::from_secs(self.idle_timeout_secs)),
        }
    }

    /// # Errors
    ///
    /// No `--state-dir` was given and the environment names no home.
    pub fn state_dir(&self) -> Result<StateDir, PathsError> {
        match &self.state_dir {
            Some(path) => Ok(StateDir::at(path.clone())),
            None => StateDir::from_env(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_addresses_parse_and_others_are_refused() {
        for text in ["127.0.0.1:8077", "127.0.0.1:0", "[::1]:8077", "127.2.3.4:1"] {
            let addr: LoopbackAddr = text.parse().unwrap();
            assert!(addr.socket_addr().ip().is_loopback(), "{text}");
        }
        for text in ["0.0.0.0:8077", "192.168.1.2:1", "[::]:8077"] {
            assert!(
                matches!(text.parse::<LoopbackAddr>(), Err(BindError::NotLoopback(_))),
                "{text}"
            );
        }
        assert!(matches!(
            "localhost:8077".parse::<LoopbackAddr>(),
            Err(BindError::Unparseable(_))
        ));
    }

    #[test]
    fn zero_idle_timeout_disables_the_reaper() {
        let cli = Cli::parse_from(["sbtd", "--idle-timeout-secs", "0"]);
        assert_eq!(cli.registry_config().idle_timeout, None);
        let cli = Cli::parse_from(["sbtd"]);
        assert_eq!(
            cli.registry_config().idle_timeout,
            Some(Duration::from_hours(2))
        );
        assert_eq!(cli.bind.to_string(), "127.0.0.1:8077");
    }
}
