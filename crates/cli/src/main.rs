//! `sbtctl`: what agents run. Everything it does goes over loopback HTTP to
//! `sbtd`; it never touches sbt directly.

mod args;
mod client;
mod daemon_ctl;
mod render;

use std::ffi::OsString;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use sbt_server_core::paths::{PathsError, StateDir};
use sbt_server_core::protocol::{ExecRequest, InstancePhase, ShutdownRequest, StatusResponse};
use thiserror::Error;

use crate::args::{Cli, Command, DaemonAction};
use crate::client::{ClientError, DaemonClient};
use crate::daemon_ctl::{DaemonCtlError, Liveness};

#[derive(Debug, Error)]
enum CliError {
    #[error(transparent)]
    Client(#[from] ClientError),
    #[error(transparent)]
    Daemon(#[from] DaemonCtlError),
    #[error("cannot determine the current directory: {0}")]
    Cwd(io::Error),
    #[error(transparent)]
    Paths(#[from] PathsError),
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    let client = DaemonClient::new(&cli.daemon_url);
    match run(cli, &client).await {
        Ok(code) => code,
        Err(error) => {
            report(&error);
            ExitCode::from(render::TOOL_FAILURE)
        }
    }
}

fn report(error: &CliError) {
    eprintln!("sbtctl: {error}");
    let client_error = match error {
        CliError::Client(inner) | CliError::Daemon(DaemonCtlError::Client(inner)) => Some(inner),
        CliError::Daemon(_) | CliError::Cwd(_) | CliError::Paths(_) => None,
    };
    if let Some(inner) = client_error {
        for line in inner.details() {
            eprintln!("  {line}");
        }
        if let Some(hint) = inner.hint() {
            eprintln!("hint: {hint}");
        }
    }
}

async fn run(cli: Cli, client: &DaemonClient) -> Result<ExitCode, CliError> {
    match cli.command {
        Command::Sbt(ref words) => exec(client, cwd(&cli)?, words, cli.verbose).await,
        Command::Status => {
            print_status(&client.status().await?);
            Ok(ExitCode::SUCCESS)
        }
        Command::Shutdown { all } => {
            let request = if all {
                ShutdownRequest::All
            } else {
                ShutdownRequest::One { cwd: cwd(&cli)? }
            };
            let response = client.shutdown(&request).await?;
            if response.stopped.is_empty() {
                println!("no warm sbt instance to stop");
            }
            for root in response.stopped {
                println!("stopped sbt for {}", root.display());
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::Daemon { action } => daemon(client, &cli, action).await,
    }
}

async fn exec(
    client: &DaemonClient,
    cwd: PathBuf,
    words: &[OsString],
    verbose: bool,
) -> Result<ExitCode, CliError> {
    let command_line = words
        .iter()
        .map(|word| word.to_string_lossy())
        .collect::<Vec<_>>()
        .join(" ");
    let request = ExecRequest {
        cwd,
        command_line,
        verbose,
    };
    let stdout = io::stdout();
    let stderr = io::stderr();
    let mut out = stdout.lock();
    let mut err = stderr.lock();
    let terminal = client
        .exec(&request, |event| {
            drop(render::render(event, &mut out, &mut err));
        })
        .await?;
    drop(out.flush());
    Ok(ExitCode::from(render::exit_code(terminal.as_ref())))
}

fn cwd(cli: &Cli) -> Result<PathBuf, CliError> {
    match &cli.directory {
        Some(directory) => Ok(directory.clone()),
        None => std::env::current_dir().map_err(CliError::Cwd),
    }
}

fn state_dir(cli: &Cli) -> Result<StateDir, CliError> {
    match &cli.state_dir {
        Some(path) => Ok(StateDir::at(path.clone())),
        None => Ok(StateDir::from_env()?),
    }
}

fn print_status(status: &StatusResponse) {
    if status.instances.is_empty() {
        println!("no warm sbt instances");
        return;
    }
    let width = status
        .instances
        .iter()
        .map(|i| i.build_root.display().to_string().len())
        .max()
        .unwrap_or(0)
        .max("BUILD ROOT".len());
    println!(
        "{:<width$}  {:>7}  {:<8}  {:>6}  {:>7}",
        "BUILD ROOT", "PID", "PHASE", "IDLE", "UPTIME"
    );
    for instance in &status.instances {
        let pid = instance
            .pid
            .map_or_else(|| "-".to_owned(), |pid| pid.to_string());
        let phase = match instance.phase {
            InstancePhase::Booting => "booting",
            InstancePhase::Idle => "idle",
            InstancePhase::Running => "running",
        };
        let idle = instance
            .idle_secs
            .map_or_else(|| "-".to_owned(), |s| format!("{s}s"));
        println!(
            "{:<width$}  {pid:>7}  {phase:<8}  {idle:>6}  {:>6}s",
            instance.build_root.display(),
            instance.uptime_secs
        );
    }
}

async fn daemon(
    client: &DaemonClient,
    cli: &Cli,
    action: DaemonAction,
) -> Result<ExitCode, CliError> {
    let state_dir = state_dir(cli)?;
    let bind = daemon_ctl::bind_from_url(&cli.daemon_url);
    match action {
        DaemonAction::Start => {
            let started = daemon_ctl::start(client, &state_dir, &bind).await?;
            println!(
                "sbtd started (pid {}) at {}",
                started.pid,
                client.base_url()
            );
            Ok(ExitCode::SUCCESS)
        }
        DaemonAction::Stop => match daemon_ctl::stop(client, &state_dir).await {
            Ok(stopped) => {
                println!("sbtd stopped (pid {})", stopped.pid);
                Ok(ExitCode::SUCCESS)
            }
            Err(DaemonCtlError::NotRunning { url }) => {
                println!("sbtd is not running at {url}");
                Ok(ExitCode::SUCCESS)
            }
            Err(error) => Err(error.into()),
        },
        DaemonAction::Restart => {
            let started = daemon_ctl::restart(client, &state_dir, &bind).await?;
            println!(
                "sbtd restarted (pid {}) at {}",
                started.pid,
                client.base_url()
            );
            Ok(ExitCode::SUCCESS)
        }
        DaemonAction::Status => match daemon_ctl::status(client).await? {
            Liveness::Running(health) => {
                println!(
                    "sbtd running: pid {} version {} up {}s at {}",
                    health.pid,
                    health.version,
                    health.uptime_secs,
                    client.base_url()
                );
                Ok(ExitCode::SUCCESS)
            }
            Liveness::NotRunning => {
                println!("sbtd is not running at {}", client.base_url());
                Ok(ExitCode::from(1))
            }
        },
    }
}
