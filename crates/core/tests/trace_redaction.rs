//! sbt's auth token must never reach a trace line, even with every frame
//! traced. Its own test binary, so the capturing subscriber sees every
//! callsite from the first event on.

use std::ffi::OsString;
use std::io;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fake_sbt::{TOKEN_ENV, TRANSPORT_ENV};
use sbt_server_core::instance::{CommandLine, Instance, Launcher, SbtConfig, Timeouts};
use sbt_server_core::routing::resolve_build_root;
use tracing_subscriber::fmt::MakeWriter;

const TOKEN: &str = "84046191245433876643612047032303751629";

#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl Captured {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

impl io::Write for Captured {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for Captured {
    type Writer = Self;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

fn config() -> SbtConfig {
    SbtConfig {
        launcher: Launcher {
            program: fake_sbt::bin_path().unwrap(),
            args: Vec::new(),
            env: [(TRANSPORT_ENV, "tcp"), (TOKEN_ENV, TOKEN)]
                .into_iter()
                .map(|(k, v)| (OsString::from(k), OsString::from(v)))
                .collect(),
        },
        timeouts: Timeouts {
            connect: Duration::from_secs(2),
            handshake: Duration::from_secs(2),
            boot: Duration::from_secs(10),
            takeover: Duration::from_secs(2),
            shutdown: Duration::from_millis(500),
        },
    }
}

#[tokio::test]
async fn the_token_handshake_traced_in_full_never_shows_the_token() {
    let captured = Captured::default();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .with_writer(captured.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("build.sbt"), "").unwrap();
    let root = resolve_build_root(dir.path()).unwrap();
    let instance = Instance::start(&config(), root).await.unwrap();
    drop(
        instance
            .exec(CommandLine::new("compile").unwrap(), false)
            .collect()
            .await,
    );
    instance.shutdown().await;

    let log = captured.text();
    assert!(
        log.lines()
            .any(|line| line.contains("-> sbt") && line.contains("initialize")),
        "the initialize frame was not traced:\n{log}"
    );
    assert!(!log.contains(TOKEN), "the token leaked:\n{log}");
}
