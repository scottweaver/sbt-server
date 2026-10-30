//! `sbtctl daemon start|stop|restart|status`. Every transition verb confirms
//! the transition through `/health` instead of trusting that a spawn or a
//! signal worked.

use std::fs::File;
use std::io;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;
use sbt_server_core::paths::{StateDir, StateDirError, StateFile};
use sbt_server_core::protocol::HealthResponse;
use thiserror::Error;

use crate::client::{ClientError, DaemonClient};

const START_TIMEOUT: Duration = Duration::from_secs(5);
const STOP_TIMEOUT: Duration = Duration::from_secs(10);
const POLL: Duration = Duration::from_millis(100);

#[derive(Debug, Error)]
pub enum DaemonCtlError {
    #[error("sbtd is already running (pid {pid}) at {url}")]
    AlreadyRunning { pid: u32, url: String },
    #[error("cannot spawn {}: {source}", program.display())]
    Spawn {
        program: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error(transparent)]
    StateDir(#[from] StateDirError),
    #[error("cannot prepare {}: {source}", path.display())]
    LogHandle {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("sbtd started (pid {pid}) but {url}/health did not answer within {waited:?}; see {}", log.display())]
    NotHealthy {
        pid: u32,
        url: String,
        waited: Duration,
        log: PathBuf,
    },
    #[error("sbtd is not running at {url}")]
    NotRunning { url: String },
    #[error("{} says pid {file_pid} but the daemon at {url} reports pid {live_pid}; refusing to signal", pid_file.display())]
    PidMismatch {
        pid_file: PathBuf,
        file_pid: u32,
        live_pid: u32,
        url: String,
    },
    #[error("cannot read {}: {source}", pid_file.display())]
    PidFile {
        pid_file: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("{} does not hold a pid: {text:?}", pid_file.display())]
    BadPidFile { pid_file: PathBuf, text: String },
    #[error("cannot signal pid {pid}: {source}")]
    Signal {
        pid: u32,
        #[source]
        source: nix::Error,
    },
    #[error("sbtd (pid {pid}) still answers {url}/health after {waited:?}")]
    StillRunning {
        pid: u32,
        url: String,
        waited: Duration,
    },
    #[error(transparent)]
    Client(ClientError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Started {
    pub pid: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stopped {
    pub pid: u32,
}

#[derive(Debug)]
pub enum Liveness {
    Running(HealthResponse),
    NotRunning,
}

/// `host:port` for sbtd's `--bind`, taken from the URL sbtctl will use.
/// `--bind` takes a socket-address literal, so the one name the daemon's
/// Host check accepts, `localhost`, becomes `127.0.0.1` here.
#[must_use]
pub fn bind_from_url(url: &str) -> String {
    let authority = url.trim_start_matches("http://").trim_end_matches('/');
    match authority.rsplit_once(':') {
        Some((host, port)) if host.eq_ignore_ascii_case("localhost") => {
            format!("127.0.0.1:{port}")
        }
        _ if authority.eq_ignore_ascii_case("localhost") => "127.0.0.1:80".to_owned(),
        _ => authority.to_owned(),
    }
}

/// Where `sbtd` is: next to this executable, else on `PATH`.
#[must_use]
pub fn locate_sbtd() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("sbtd")))
        .filter(|candidate| candidate.is_file())
        .unwrap_or_else(|| PathBuf::from("sbtd"))
}

/// # Errors
///
/// sbtd is unreachable (so there is nothing to ask); the client's own
/// errors otherwise.
pub async fn status(client: &DaemonClient) -> Result<Liveness, DaemonCtlError> {
    match client.health().await {
        Ok(health) => Ok(Liveness::Running(health)),
        Err(ClientError::Unreachable { .. }) => Ok(Liveness::NotRunning),
        Err(error) => Err(DaemonCtlError::Client(error)),
    }
}

/// Spawn sbtd detached (its own process group, stdio to the log file) and
/// wait until `/health` answers.
///
/// # Errors
///
/// A daemon already answers, the spawn failed, or it never became healthy.
pub async fn start(
    client: &DaemonClient,
    state_dir: &StateDir,
    bind: &str,
) -> Result<Started, DaemonCtlError> {
    if let Liveness::Running(health) = status(client).await? {
        return Err(DaemonCtlError::AlreadyRunning {
            pid: health.pid,
            url: client.base_url().to_owned(),
        });
    }
    let log = open_log(state_dir)?;
    let program = locate_sbtd();
    let child = Command::new(&program)
        .arg("--bind")
        .arg(bind)
        .arg("--state-dir")
        .arg(state_dir.path())
        .stdin(Stdio::null())
        .stdout(Stdio::from(log.try_clone().map_err(|source| {
            DaemonCtlError::LogHandle {
                path: state_dir.log_file(),
                source,
            }
        })?))
        .stderr(Stdio::from(log))
        .process_group(0)
        .spawn()
        .map_err(|source| DaemonCtlError::Spawn { program, source })?;
    let pid = child.id();

    let deadline = Instant::now() + START_TIMEOUT;
    loop {
        if let Ok(health) = client.health().await {
            return Ok(Started { pid: health.pid });
        }
        if Instant::now() >= deadline {
            return Err(DaemonCtlError::NotHealthy {
                pid,
                url: client.base_url().to_owned(),
                waited: START_TIMEOUT,
                log: state_dir.log_file(),
            });
        }
        tokio::time::sleep(POLL).await;
    }
}

/// SIGTERM the daemon named by the pid file, after checking that the one
/// answering `/health` is that very process, and wait until it is gone.
///
/// # Errors
///
/// Nothing answers `/health`, the pid file and the live daemon disagree,
/// the signal failed, or the daemon outlived the stop timeout.
pub async fn stop(client: &DaemonClient, state_dir: &StateDir) -> Result<Stopped, DaemonCtlError> {
    let live = match status(client).await? {
        Liveness::Running(health) => health,
        Liveness::NotRunning => {
            return Err(DaemonCtlError::NotRunning {
                url: client.base_url().to_owned(),
            });
        }
    };
    let file_pid = read_pid_file(state_dir)?;
    if file_pid != live.pid {
        return Err(DaemonCtlError::PidMismatch {
            pid_file: state_dir.pid_file(),
            file_pid,
            live_pid: live.pid,
            url: client.base_url().to_owned(),
        });
    }
    let raw = i32::try_from(live.pid).map_err(|_| DaemonCtlError::BadPidFile {
        pid_file: state_dir.pid_file(),
        text: live.pid.to_string(),
    })?;
    kill(Pid::from_raw(raw), Signal::SIGTERM).map_err(|source| DaemonCtlError::Signal {
        pid: live.pid,
        source,
    })?;

    let deadline = Instant::now() + STOP_TIMEOUT;
    loop {
        if let Liveness::NotRunning = status(client).await? {
            return Ok(Stopped { pid: live.pid });
        }
        if Instant::now() >= deadline {
            return Err(DaemonCtlError::StillRunning {
                pid: live.pid,
                url: client.base_url().to_owned(),
                waited: STOP_TIMEOUT,
            });
        }
        tokio::time::sleep(POLL).await;
    }
}

/// Stop when running, then start.
///
/// # Errors
///
/// As [`stop`] (except "not running", which is fine here) and [`start`].
pub async fn restart(
    client: &DaemonClient,
    state_dir: &StateDir,
    bind: &str,
) -> Result<Started, DaemonCtlError> {
    match stop(client, state_dir).await {
        Ok(_) | Err(DaemonCtlError::NotRunning { .. }) => {}
        Err(error) => return Err(error),
    }
    start(client, state_dir, bind).await
}

fn open_log(state_dir: &StateDir) -> Result<File, StateDirError> {
    state_dir.ensure()?;
    state_dir.open(StateFile::Log)
}

fn read_pid_file(state_dir: &StateDir) -> Result<u32, DaemonCtlError> {
    let pid_file = state_dir.pid_file();
    let text = std::fs::read_to_string(&pid_file).map_err(|source| DaemonCtlError::PidFile {
        pid_file: pid_file.clone(),
        source,
    })?;
    text.trim()
        .parse()
        .map_err(|_| DaemonCtlError::BadPidFile { pid_file, text })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bind_is_the_authority_of_the_daemon_url() {
        assert_eq!(bind_from_url("http://127.0.0.1:8077"), "127.0.0.1:8077");
        assert_eq!(bind_from_url("http://127.0.0.1:9000/"), "127.0.0.1:9000");
        assert_eq!(bind_from_url("http://[::1]:8077"), "[::1]:8077");
    }

    #[test]
    fn localhost_becomes_the_v4_loopback_literal_sbtd_can_bind() {
        assert_eq!(bind_from_url("http://localhost:8077"), "127.0.0.1:8077");
        assert_eq!(bind_from_url("http://LocalHost:9000/"), "127.0.0.1:9000");
        assert_eq!(bind_from_url("http://localhost"), "127.0.0.1:80");
    }
}
