//! `Registry` against the fake sbt binary.

use std::ffi::OsString;
use std::sync::Arc;
use std::time::{Duration, Instant};

use fake_sbt::{BOOT_FAIL_ENV, SCENARIO_ENV};
use sbt_server_core::instance::{CommandLine, Launcher, Phase, SbtConfig, StartError, Timeouts};
use sbt_server_core::protocol::{ExecEvent, InstancePhase};
use sbt_server_core::registry::{Registry, RegistryConfig};
use sbt_server_core::routing::{BuildRoot, resolve_build_root};

fn registry(env: &[(&str, &str)], idle_timeout: Option<Duration>) -> Arc<Registry> {
    Registry::new(RegistryConfig {
        sbt: SbtConfig {
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
        },
        idle_timeout,
    })
}

fn build_root() -> (tempfile::TempDir, BuildRoot) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("build.sbt"), "").unwrap();
    let root = resolve_build_root(dir.path()).unwrap();
    (dir, root)
}

fn pid_alive(pid: u32) -> bool {
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .status()
        .is_ok_and(|s| s.success())
}

async fn wait_until(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn concurrent_first_callers_share_one_boot() {
    let registry = registry(&[], None);
    let (_dir, root) = build_root();
    let mut set = tokio::task::JoinSet::new();
    for _ in 0..5 {
        let registry = Arc::clone(&registry);
        let root = root.clone();
        set.spawn(async move { registry.get_or_start(root).await.unwrap().id() });
    }
    let mut ids = Vec::new();
    while let Some(joined) = set.join_next().await {
        ids.push(joined.unwrap());
    }
    assert!(ids.iter().all(|id| *id == ids[0]), "{ids:?}");
    assert_eq!(registry.status().len(), 1);
    registry.shutdown_all().await;
    assert!(registry.status().is_empty());
}

#[tokio::test]
async fn a_dead_instance_is_replaced_on_the_next_call() {
    let registry = registry(&[], None);
    let (_dir, root) = build_root();
    let first = registry.get_or_start(root.clone()).await.unwrap();
    std::process::Command::new("kill")
        .args(["-9", &first.pid().get().to_string()])
        .status()
        .unwrap();
    wait_until("the child to be reported dead", || first.phase().is_dead()).await;
    assert!(
        registry.status().is_empty(),
        "dead instances are not listed"
    );

    let second = registry.get_or_start(root).await.unwrap();
    assert_ne!(second.id(), first.id());
    assert_ne!(second.pid(), first.pid());
    assert!(matches!(second.phase(), Phase::Idle { .. }));
    registry.shutdown_all().await;
}

#[tokio::test]
async fn a_failed_boot_is_shared_then_retried() {
    let registry = registry(&[(BOOT_FAIL_ENV, "1")], None);
    let (_dir, root) = build_root();
    let (a, b) = tokio::join!(
        registry.get_or_start(root.clone()),
        registry.get_or_start(root.clone())
    );
    for outcome in [a, b] {
        let error = outcome.unwrap_err();
        assert!(
            matches!(*error, StartError::ProjectLoadFailed { .. }),
            "{error}"
        );
    }
    assert!(registry.status().is_empty());
    let again = registry.get_or_start(root).await.unwrap_err();
    assert!(matches!(*again, StartError::ProjectLoadFailed { .. }));
}

#[tokio::test]
async fn shutdown_one_leaves_the_others_warm() {
    let registry = registry(&[], None);
    let (_dir_a, root_a) = build_root();
    let (_dir_b, root_b) = build_root();
    let a = registry.get_or_start(root_a.clone()).await.unwrap();
    let b = registry.get_or_start(root_b.clone()).await.unwrap();

    let report = registry.shutdown(&root_a).await.unwrap();
    assert!(report.reaped);
    assert!(!pid_alive(a.pid().get()));
    assert!(pid_alive(b.pid().get()));
    let status = registry.status();
    assert_eq!(status.len(), 1);
    assert_eq!(status[0].build_root, root_b.as_path());
    assert_eq!(status[0].pid, Some(b.pid().get()));
    assert_eq!(status[0].phase, InstancePhase::Idle);
    assert!(registry.shutdown(&root_a).await.is_none());

    let stopped = registry.shutdown_all().await;
    assert_eq!(stopped.len(), 1);
    assert!(!pid_alive(b.pid().get()));
}

#[tokio::test]
async fn the_reaper_evicts_idle_instances_but_not_running_ones() {
    let registry = registry(
        &[(SCENARIO_ENV, "exec=mute")],
        Some(Duration::from_millis(300)),
    );
    let (_dir_idle, root_idle) = build_root();
    let (_dir_busy, root_busy) = build_root();
    let idle = registry.get_or_start(root_idle.clone()).await.unwrap();
    let busy = registry.get_or_start(root_busy.clone()).await.unwrap();
    let mut running = busy.exec(CommandLine::new("compile").unwrap(), false);
    assert!(matches!(
        running.next().await,
        Some(ExecEvent::Started { .. })
    ));

    let reaper = registry.spawn_reaper(Duration::from_millis(100)).unwrap();
    wait_until("the idle instance to be reaped", || {
        !pid_alive(idle.pid().get())
    })
    .await;
    let status = registry.status();
    assert_eq!(status.len(), 1, "{status:?}");
    assert_eq!(status[0].phase, InstancePhase::Running);
    assert!(pid_alive(busy.pid().get()));

    reaper.abort();
    registry.shutdown_all().await;
    assert!(!pid_alive(busy.pid().get()));
}

#[tokio::test]
async fn no_idle_timeout_means_no_reaper() {
    let registry = registry(&[], None);
    assert!(registry.spawn_reaper(Duration::from_millis(10)).is_none());
}
