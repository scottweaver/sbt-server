//! `sbtd`: the local daemon that keeps one warm sbt server per build root
//! and serves it to `sbtctl` over loopback HTTP. Whole path is `async`;
//! never start a second runtime from inside this one (gotcha 7).

mod config;
mod server;

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context;
use clap::Parser;
use sbt_server_core::paths::StateDir;
use sbt_server_core::registry::Registry;
use tokio::net::TcpListener;
use tokio::signal::unix::{Signal, SignalKind, signal};
use tracing_subscriber::EnvFilter;

use crate::config::Cli;
use crate::server::AppState;

const REAPER_INTERVAL: Duration = Duration::from_mins(1);

/// Run the daemon to completion on its own Tokio runtime.
///
/// # Errors
///
/// The runtime cannot be built, the state directory or the listener cannot
/// be prepared, or serving fails.
pub fn run() -> anyhow::Result<()> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("cannot start the async runtime")?
        .block_on(serve())
}

async fn serve() -> anyhow::Result<()> {
    let cli = Cli::parse();
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let state_dir = cli.state_dir()?;
    tokio::fs::create_dir_all(state_dir.path())
        .await
        .with_context(|| format!("cannot create {}", state_dir.path().display()))?;
    let listener = TcpListener::bind(cli.bind.socket_addr())
        .await
        .with_context(|| format!("cannot listen on {}", cli.bind))?;
    let addr = listener.local_addr()?;
    let sigterm = signal(SignalKind::terminate()).context("cannot listen for SIGTERM")?;
    write_state_files(&state_dir, addr).await?;

    let registry = Registry::new(cli.registry_config()?);
    let reaper = registry.spawn_reaper(REAPER_INTERVAL);
    let state = Arc::new(AppState {
        registry: Arc::clone(&registry),
        started_at: Instant::now(),
    });
    tracing::info!(%addr, pid = std::process::id(), "sbtd listening");

    axum::serve(listener, server::router(state))
        .with_graceful_shutdown(drain_on_signal(sigterm, registry))
        .await?;

    if let Some(reaper) = reaper {
        reaper.abort();
    }
    remove_state_files(&state_dir).await;
    tracing::info!("sbtd stopped");
    Ok(())
}

async fn write_state_files(state_dir: &StateDir, addr: std::net::SocketAddr) -> anyhow::Result<()> {
    tokio::fs::write(state_dir.pid_file(), std::process::id().to_string())
        .await
        .with_context(|| format!("cannot write {}", state_dir.pid_file().display()))?;
    tokio::fs::write(state_dir.addr_file(), addr.to_string())
        .await
        .with_context(|| format!("cannot write {}", state_dir.addr_file().display()))?;
    Ok(())
}

async fn remove_state_files(state_dir: &StateDir) {
    for path in [state_dir.pid_file(), state_dir.addr_file()] {
        drop(tokio::fs::remove_file(path).await);
    }
}

/// Resolves once every instance is down, so in-flight `/exec` streams have
/// already ended (with `aborted: daemon_shutdown`) by the time axum stops
/// accepting and waits for connections to drain.
async fn drain_on_signal(mut sigterm: Signal, registry: Arc<Registry>) {
    tokio::select! {
        _ = sigterm.recv() => tracing::info!("SIGTERM received"),
        result = tokio::signal::ctrl_c() => match result {
            Ok(()) => tracing::info!("interrupted"),
            Err(error) => tracing::warn!(%error, "ctrl-c handler failed; waiting for SIGTERM"),
        },
    }
    let stopped = registry.shutdown_all().await;
    tracing::info!(instances = stopped.len(), "all sbt instances stopped");
}
