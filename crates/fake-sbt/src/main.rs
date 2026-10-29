//! The fake sbt child process: prints a line to each of stdout and stderr the
//! way a JVM would, writes `project/target/active.json` under the current
//! directory, serves the scripted protocol, and exits on `shutdown` or
//! SIGTERM. Configured entirely through `FAKE_SBT_*` environment variables.

use std::process::ExitCode;
use std::time::Duration;

use fake_sbt::{BOOT_DELAY_MS_ENV, BOOT_FAIL_ENV, FakeSbtServer, Options};
use tokio::signal::unix::{SignalKind, signal};

#[tokio::main]
async fn main() -> ExitCode {
    println!("fake-sbt: booting");
    eprintln!("fake-sbt: a warning on stderr");
    if std::env::var_os(BOOT_FAIL_ENV).is_some() {
        eprintln!("[error] fake-sbt: project loading failed");
        return ExitCode::from(1);
    }
    let options = match Options::from_env() {
        Ok(options) => options,
        Err(error) => return fail(&format!("bad configuration: {error}")),
    };
    if let Some(delay) = boot_delay() {
        tokio::time::sleep(delay).await;
    }
    let cwd = match std::env::current_dir() {
        Ok(cwd) => cwd,
        Err(error) => return fail(&format!("no current directory: {error}")),
    };
    let server = match FakeSbtServer::start(&cwd, options).await {
        Ok(server) => server,
        Err(error) => return fail(&format!("cannot start: {error}")),
    };
    let mut sigterm = match signal(SignalKind::terminate()) {
        Ok(sigterm) => sigterm,
        Err(error) => return fail(&format!("cannot listen for SIGTERM: {error}")),
    };
    println!("fake-sbt: serving at {}", server.endpoint().uri());
    tokio::select! {
        () = server.finished() => {}
        _ = sigterm.recv() => server.stop().await,
    }
    ExitCode::SUCCESS
}

fn boot_delay() -> Option<Duration> {
    std::env::var(BOOT_DELAY_MS_ENV)
        .ok()
        .and_then(|ms| ms.parse().ok())
        .map(Duration::from_millis)
}

fn fail(message: &str) -> ExitCode {
    eprintln!("fake-sbt: {message}");
    ExitCode::from(2)
}
