//! `Instance` against the fake sbt binary as a real child process.

use std::ffi::OsString;
use std::path::Path;
use std::time::{Duration, Instant};

use fake_sbt::{
    BOOT_DELAY_MS_ENV, BOOT_FAIL_ENV, FakeSbtServer, Options, PRINT_ON_EXEC_ENV, SCENARIO_ENV,
    Scenario, ShutdownBehaviour, TOKEN_ENV, TRANSPORT_ENV, Transport,
};
use sbt_server_core::instance::{
    CommandLine, Instance, Launcher, Phase, SbtConfig, ShutdownRpc, StartError, Timeouts,
};
use sbt_server_core::protocol::{AbortReason, ExecEvent, LogLevel, Outcome, OutputStream};
use sbt_server_core::routing::{BuildRoot, resolve_build_root};

fn config(env: &[(&str, &str)]) -> SbtConfig {
    SbtConfig {
        launcher: Launcher {
            program: fake_sbt::bin_path().unwrap(),
            args: Vec::new(),
            env: env
                .iter()
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

fn build_root() -> (tempfile::TempDir, BuildRoot) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("build.sbt"), "").unwrap();
    let root = resolve_build_root(dir.path()).unwrap();
    (dir, root)
}

fn command(text: &str) -> CommandLine {
    CommandLine::new(text).unwrap()
}

fn pid_alive(pid: u32) -> bool {
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .status()
        .is_ok_and(|s| s.success())
}

fn terminal(events: &[ExecEvent]) -> &ExecEvent {
    let last = events.last().unwrap();
    assert!(last.is_terminal(), "last event is not terminal: {last:?}");
    assert_eq!(
        events.iter().filter(|e| e.is_terminal()).count(),
        1,
        "exactly one terminal event: {events:?}"
    );
    last
}

async fn wait_until(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn cold_start_then_exec_streams_the_scripted_output() {
    let (_dir, root) = build_root();
    let instance = Instance::start(&config(&[]), root).await.unwrap();
    assert!(matches!(instance.phase(), Phase::Idle { .. }));
    assert!(pid_alive(instance.pid().get()));

    let events = instance.exec(command("compile"), false).collect().await;
    assert!(
        matches!(&events[0], ExecEvent::Started { cold_start: true, pid, .. } if *pid == instance.pid().get()),
        "{events:?}"
    );
    let logs: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            ExecEvent::Log { level, text } => Some((*level, text.as_str())),
            _ => None,
        })
        .collect();
    assert_eq!(
        logs[0].0,
        LogLevel::Info,
        "debug noise is dropped: {logs:?}"
    );
    assert!(logs.iter().all(|(level, _)| *level != LogLevel::Debug));
    assert!(events.contains(&ExecEvent::Output {
        stream: OutputStream::SbtStdout,
        text: "hello from println".into()
    }));
    assert!(matches!(
        events.iter().find(|e| matches!(e, ExecEvent::Diagnostics { .. })),
        Some(ExecEvent::Diagnostics { items, .. }) if items.is_empty()
    ));
    let ExecEvent::Done {
        outcome,
        exit_code,
        summary,
        ..
    } = terminal(&events)
    else {
        panic!("expected Done")
    };
    assert_eq!((*outcome, *exit_code), (Outcome::Success, 0));
    assert!(
        summary.starts_with("[success] 'compile' completed in"),
        "{summary}"
    );

    let warm = instance.exec(command("test"), false).collect().await;
    assert!(matches!(
        &warm[0],
        ExecEvent::Started {
            cold_start: false,
            ..
        }
    ));

    let report = instance.shutdown().await;
    assert_eq!(report.rpc, ShutdownRpc::ConnectionClosed);
    assert!(report.reaped);
    assert!(instance.phase().is_dead());
    assert!(!pid_alive(instance.pid().get()));
}

#[tokio::test]
async fn verbose_keeps_debug_logs() {
    let (_dir, root) = build_root();
    let instance = Instance::start(&config(&[]), root).await.unwrap();
    let events = instance.exec(command("compile"), true).collect().await;
    assert!(events.contains(&ExecEvent::Log {
        level: LogLevel::Debug,
        text: "Processing sbt/exec".into()
    }));
    instance.shutdown().await;
}

#[tokio::test]
async fn failing_exec_reports_the_exit_code_and_diagnostics() {
    let (_dir, root) = build_root();
    let instance = Instance::start(&config(&[(SCENARIO_ENV, "exec=fails:1")]), root)
        .await
        .unwrap();
    let events = instance.exec(command("compile"), false).collect().await;
    let ExecEvent::Done {
        outcome,
        exit_code,
        summary,
        ..
    } = terminal(&events)
    else {
        panic!("expected Done")
    };
    assert_eq!((*outcome, *exit_code), (Outcome::Failure, 1));
    assert!(
        summary.starts_with("[error] 'compile' failed (exit code 1) in"),
        "{summary}"
    );
    let Some(ExecEvent::Diagnostics { file, items }) = events.iter().find(
        |e| matches!(e, ExecEvent::Diagnostics { file, .. } if file.ends_with("Broken.scala")),
    ) else {
        panic!("expected diagnostics")
    };
    assert_eq!(file, Path::new("/tmp/hello/src/main/scala/Broken.scala"));
    assert_eq!((items[0].line, items[0].column), (2, 16));
    assert!(
        summary.ends_with(": (Compile / compileIncremental) Compilation failed"),
        "{summary}"
    );
    instance.shutdown().await;
}

#[tokio::test]
async fn raw_jvm_output_during_an_exec_is_captured_from_the_pipes() {
    let (_dir, root) = build_root();
    let cfg = config(&[(SCENARIO_ENV, "delay=300"), (PRINT_ON_EXEC_ENV, "1")]);
    let instance = Instance::start(&cfg, root).await.unwrap();
    let events = instance.exec(command("compile"), false).collect().await;
    assert!(
        events.contains(&ExecEvent::Output {
            stream: OutputStream::Stdout,
            text: "fake-sbt: exec compile".into()
        }),
        "{events:?}"
    );
    instance.shutdown().await;
}

#[tokio::test]
async fn execs_on_one_instance_are_serialized_even_if_a_consumer_leaves() {
    let (_dir, root) = build_root();
    let instance = Instance::start(&config(&[(SCENARIO_ENV, "delay=300")]), root)
        .await
        .unwrap();
    let issued = Instant::now();
    drop(instance.exec(command("compile"), false));
    let mut second = instance.exec(command("test"), false);
    let first_event = second.next().await.unwrap();
    assert!(matches!(first_event, ExecEvent::Started { .. }));
    assert!(
        issued.elapsed() >= Duration::from_millis(300),
        "second exec started before the first finished"
    );
    assert!(matches!(instance.phase(), Phase::Running { .. }));
    terminal(&second.collect().await);
    assert!(matches!(instance.phase(), Phase::Idle { .. }));
    instance.shutdown().await;
}

#[tokio::test]
async fn child_death_mid_exec_aborts_with_sbt_died() {
    let (_dir, root) = build_root();
    let instance = Instance::start(&config(&[(SCENARIO_ENV, "exec=mute")]), root)
        .await
        .unwrap();
    let mut output = instance.exec(command("compile"), false);
    assert!(matches!(
        output.next().await.unwrap(),
        ExecEvent::Started { .. }
    ));
    std::process::Command::new("kill")
        .args(["-9", &instance.pid().get().to_string()])
        .status()
        .unwrap();
    let events = output.collect().await;
    assert!(
        matches!(
            terminal(&events),
            ExecEvent::Aborted {
                reason: AbortReason::SbtDied,
                ..
            }
        ),
        "{events:?}"
    );
    wait_until("phase to become dead", || instance.phase().is_dead()).await;
    let report = instance.shutdown().await;
    assert_eq!(report.rpc, ShutdownRpc::ConnectionClosed);
    assert!(report.reaped);
}

#[tokio::test]
async fn mute_shutdown_is_bounded_and_the_child_is_still_reaped() {
    let (_dir, root) = build_root();
    let instance = Instance::start(&config(&[(SCENARIO_ENV, "shutdown=mute")]), root)
        .await
        .unwrap();
    let pid = instance.pid().get();
    let started = Instant::now();
    let report = instance.shutdown().await;
    assert_eq!(report.rpc, ShutdownRpc::TimedOut);
    assert!(report.reaped);
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
    assert!(!pid_alive(pid));
}

#[tokio::test]
async fn takeover_asks_a_cooperative_foreign_server_to_shut_down_first() {
    let (dir, root) = build_root();
    let foreign = FakeSbtServer::start(dir.path(), Options::default())
        .await
        .unwrap();
    let foreign_uri = foreign.endpoint().uri();

    let instance = Instance::start(&config(&[]), root).await.unwrap();
    let foreign_execs = foreign.exec_requests();
    assert_eq!(foreign_execs.len(), 1);
    assert_eq!(foreign_execs[0].params["commandLine"], "shutdown");
    assert!(foreign.is_finished());
    let descriptor = std::fs::read_to_string(instance.root().active_json()).unwrap();
    assert!(!descriptor.contains(&foreign_uri), "{descriptor}");
    terminal(&instance.exec(command("compile"), false).collect().await);
    instance.shutdown().await;
}

#[tokio::test]
async fn takeover_of_a_mute_foreign_server_is_bounded_and_spawns_anyway() {
    let (dir, root) = build_root();
    let options = Options {
        scenario: Scenario {
            shutdown: ShutdownBehaviour::Mute,
            ..Scenario::default()
        },
        ..Options::default()
    };
    let foreign = FakeSbtServer::start(dir.path(), options).await.unwrap();
    let started = Instant::now();
    let instance = Instance::start(&config(&[]), root).await.unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(8),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(foreign.exec_requests().len(), 1);
    assert!(!foreign.is_finished());
    terminal(&instance.exec(command("compile"), false).collect().await);
    instance.shutdown().await;
    foreign.stop().await;
}

#[tokio::test]
async fn takeover_waits_for_a_slow_exit_so_the_new_descriptor_survives() {
    let (dir, root) = build_root();
    let options = Options {
        scenario: Scenario {
            shutdown: ShutdownBehaviour::DelayedExit(Duration::from_millis(600)),
            ..Scenario::default()
        },
        ..Options::default()
    };
    let foreign = FakeSbtServer::start(dir.path(), options).await.unwrap();
    let instance = Instance::start(&config(&[]), root).await.unwrap();
    assert!(foreign.is_finished());
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        instance.root().active_json().is_file(),
        "the new descriptor was clobbered"
    );
    terminal(&instance.exec(command("compile"), false).collect().await);
    instance.shutdown().await;
}

#[tokio::test]
async fn project_load_failure_surfaces_the_boot_output() {
    let (_dir, root) = build_root();
    let error = Instance::start(&config(&[(BOOT_FAIL_ENV, "1")]), root)
        .await
        .unwrap_err();
    let StartError::ProjectLoadFailed { output, .. } = &error else {
        panic!("expected ProjectLoadFailed, got {error}")
    };
    assert!(
        output.iter().any(|l| l.contains("project loading failed")),
        "{output:?}"
    );
    assert!(error.to_string().contains("project loading failed"));
}

#[tokio::test]
async fn boot_timeout_kills_the_child() {
    let (_dir, root) = build_root();
    let mut cfg = config(&[(BOOT_DELAY_MS_ENV, "5000")]);
    cfg.timeouts.boot = Duration::from_millis(400);
    let error = Instance::start(&cfg, root).await.unwrap_err();
    assert!(matches!(error, StartError::BootTimeout { .. }), "{error}");
    assert!(
        error.output().iter().any(|l| l == "fake-sbt: booting"),
        "{:?}",
        error.output()
    );
}

#[tokio::test]
async fn unix_socket_and_token_transports_are_supported() {
    let socket = std::env::temp_dir().join(format!("sbts-{}.sock", std::process::id()));
    let transport = format!("local:{}", socket.display());
    let (_dir, root) = build_root();
    let instance = Instance::start(&config(&[(TRANSPORT_ENV, &transport)]), root)
        .await
        .unwrap();
    terminal(&instance.exec(command("compile"), false).collect().await);
    instance.shutdown().await;
    assert!(!socket.exists());

    let (_dir, root) = build_root();
    let instance = Instance::start(
        &config(&[(TOKEN_ENV, "84046191245433876643612047032303751629")]),
        root,
    )
    .await
    .unwrap();
    terminal(&instance.exec(command("compile"), false).collect().await);
    instance.shutdown().await;
    drop(Transport::Tcp);
}
