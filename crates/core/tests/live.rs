//! Against a real sbt and JVM. Ignored by default; run with
//! `SBT_SERVER_LIVE=1 cargo test -p sbt-server-core --test live -- --ignored --nocapture`.
//! `RUST_LOG=sbt_server_core=trace` prints every frame so new fixtures can
//! be frozen into `crates/fake-sbt/fixtures`. For spawn experiments,
//! `SBT_SERVER_LIVE_ARGS` (space-separated) overrides the launcher arguments.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use sbt_server_core::instance::{CommandLine, Instance, SbtConfig, ShutdownRpc};
use sbt_server_core::protocol::{ExecEvent, Outcome, OutputStream, Severity};
use sbt_server_core::routing::resolve_build_root;

fn enabled() -> bool {
    if std::env::var_os("SBT_SERVER_LIVE").is_none() {
        eprintln!("SBT_SERVER_LIVE is not set; skipping");
        return false;
    }
    drop(
        tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
            .with_test_writer()
            .try_init(),
    );
    true
}

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/hello-sbt")
}

fn config() -> SbtConfig {
    let mut config = SbtConfig::from_env().unwrap();
    if let Ok(args) = std::env::var("SBT_SERVER_LIVE_ARGS") {
        config.launcher.args = args.split_whitespace().map(OsString::from).collect();
    }
    eprintln!("launcher: {:?}", config.launcher);
    config
}

fn command(text: &str) -> CommandLine {
    CommandLine::new(text).unwrap()
}

fn done(events: &[ExecEvent]) -> (Outcome, i32, Duration) {
    let Some(ExecEvent::Done {
        outcome,
        exit_code,
        elapsed_ms,
        ..
    }) = events.last()
    else {
        panic!("no Done event: {events:#?}")
    };
    (*outcome, *exit_code, Duration::from_millis(*elapsed_ms))
}

fn dump(events: &[ExecEvent]) {
    for event in events {
        match event {
            ExecEvent::Output { stream, text } if text.len() > 120 => {
                eprintln!(
                    "Output {stream:?} ({} bytes): {:?}...",
                    text.len(),
                    &text[..120]
                );
            }
            other => eprintln!("{other:?}"),
        }
    }
}

fn prompt_escape_lines(events: &[ExecEvent]) -> usize {
    events
        .iter()
        .filter(|e| matches!(e, ExecEvent::Output { text, .. } if text.contains('\u{1b}')))
        .count()
}

#[tokio::test]
#[ignore = "needs sbt and a JVM; set SBT_SERVER_LIVE=1 and pass --ignored"]
async fn real_sbt_compiles_the_fixture_warm_and_cold() {
    if !enabled() {
        return;
    }
    let root = resolve_build_root(&fixture()).unwrap();
    let boot = Instant::now();
    let instance = Instance::start(&config(), root).await.unwrap();
    eprintln!("booted in {:?}", boot.elapsed());

    let events = instance.exec(command("compile"), false).collect().await;
    dump(&events);
    let (outcome, code, cold) = done(&events);
    assert_eq!((outcome, code), (Outcome::Success, 0));

    let events = instance
        .exec(command("doesNotExist"), false)
        .collect()
        .await;
    dump(&events);
    let (outcome, code, _) = done(&events);
    assert_eq!((outcome, code), (Outcome::Failure, 1));

    let events = instance.exec(command("shout"), false).collect().await;
    dump(&events);
    assert_eq!(done(&events).0, Outcome::Success);
    for (stream, text) in [
        (OutputStream::Stdout, "raw-from-subprocess"),
        (OutputStream::Stderr, "raw-from-subprocess-err"),
    ] {
        assert!(
            events.contains(&ExecEvent::Output {
                stream,
                text: text.into()
            }),
            "missing raw {stream:?} line {text:?}"
        );
    }

    let events = instance.exec(command("compile"), false).collect().await;
    let (outcome, _, warm) = done(&events);
    assert_eq!(outcome, Outcome::Success);
    eprintln!("cold compile {cold:?}, warm compile {warm:?}");
    assert!(warm < Duration::from_secs(10), "warm compile took {warm:?}");
    let noisy = prompt_escape_lines(&events);
    eprintln!("output lines carrying terminal escapes during the warm compile: {noisy}");
    assert_eq!(noisy, 0, "raw stdout carries console prompt escapes");

    let report = instance.shutdown().await;
    eprintln!("{report:?}");
    assert_ne!(report.rpc, ShutdownRpc::TimedOut);
    assert!(report.reaped);
}

fn copy_fixture_into(dir: &Path) {
    for relative in [
        "build.sbt",
        "project/build.properties",
        "src/main/scala/Hello.scala",
    ] {
        let target = dir.join(relative);
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::copy(fixture().join(relative), target).unwrap();
    }
}

#[tokio::test]
#[ignore = "needs sbt and a JVM; set SBT_SERVER_LIVE=1 and pass --ignored"]
async fn real_sbt_reports_compile_errors_as_diagnostics() {
    if !enabled() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    copy_fixture_into(dir.path());
    std::fs::write(
        dir.path().join("src/main/scala/Broken.scala"),
        "object Broken {\n  val n: Int = \"not an int\"\n}\n",
    )
    .unwrap();
    let root = resolve_build_root(dir.path()).unwrap();
    let instance = Instance::start(&config(), root).await.unwrap();

    let events = instance.exec(command("compile"), false).collect().await;
    dump(&events);
    let (outcome, code, _) = done(&events);
    assert_eq!((outcome, code), (Outcome::Failure, 1));
    let broken = events.iter().find_map(|e| match e {
        ExecEvent::Diagnostics { file, items } if file.ends_with("Broken.scala") => Some(items),
        _ => None,
    });
    let items = broken.expect("diagnostics for Broken.scala");
    assert_eq!(items[0].severity, Severity::Error);
    assert_eq!(items[0].line, 2);
    assert!(items[0].column >= 1);

    let report = instance.shutdown().await;
    assert!(report.reaped);
}
