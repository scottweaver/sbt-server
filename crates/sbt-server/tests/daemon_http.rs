//! `sbtd` as a real process, driven over HTTP, with the fake sbt binary as
//! its sbt.

use std::fs::Permissions;
use std::net::SocketAddr;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use fake_sbt::{BOOT_FAIL_ENV, SCENARIO_ENV};
use sbt_server_core::protocol::{
    AbortReason, ErrorCode, ErrorResponse, ExecEvent, ExecRequest, HealthResponse, InstancePhase,
    LogLevel, ShutdownRequest, ShutdownResponse, StatusResponse,
};
use tokio_stream::StreamExt;

struct Daemon {
    child: Child,
    addr: SocketAddr,
    state_dir: tempfile::TempDir,
    client: reqwest::Client,
}

impl Daemon {
    async fn start(env: &[(&str, &str)]) -> Self {
        let state_dir = tempfile::tempdir().unwrap();
        std::fs::set_permissions(state_dir.path(), Permissions::from_mode(0o755)).unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_sbtd"));
        command
            .args(["--bind", "127.0.0.1:0", "--idle-timeout-secs", "0"])
            .arg("--state-dir")
            .arg(state_dir.path())
            .arg("--sbt-program")
            .arg(fake_sbt::bin_path().unwrap())
            .env_remove(SCENARIO_ENV)
            .env_remove(BOOT_FAIL_ENV)
            .stdout(Stdio::null())
            .stderr(Stdio::inherit());
        for (key, value) in env {
            command.env(key, value);
        }
        let child = command.spawn().unwrap();
        let addr_file = state_dir.path().join("sbtd.addr");
        let deadline = Instant::now() + Duration::from_secs(10);
        let addr = loop {
            if let Ok(text) = std::fs::read_to_string(&addr_file)
                && let Ok(addr) = text.parse()
            {
                break addr;
            }
            assert!(Instant::now() < deadline, "sbtd never wrote its address");
            tokio::time::sleep(Duration::from_millis(20)).await;
        };
        Self {
            child,
            addr,
            state_dir,
            client: reqwest::Client::new(),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.addr)
    }

    async fn health(&self) -> Option<HealthResponse> {
        self.client
            .get(self.url("/health"))
            .send()
            .await
            .ok()?
            .json()
            .await
            .ok()
    }

    async fn status(&self) -> StatusResponse {
        self.client
            .get(self.url("/status"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap()
    }

    async fn exec(&self, cwd: &Path, command_line: &str, verbose: bool) -> reqwest::Response {
        self.client
            .post(self.url("/exec"))
            .json(&ExecRequest {
                cwd: cwd.to_path_buf(),
                command_line: command_line.to_owned(),
                verbose,
            })
            .send()
            .await
            .unwrap()
    }

    async fn exec_events(&self, cwd: &Path, command_line: &str, verbose: bool) -> Vec<ExecEvent> {
        let response = self.exec(cwd, command_line, verbose).await;
        assert_eq!(response.status(), 200);
        assert_eq!(
            response.headers()["content-type"].to_str().unwrap(),
            "application/x-ndjson"
        );
        parse_ndjson(&response.text().await.unwrap())
    }

    async fn shutdown(&self, request: ShutdownRequest) -> ShutdownResponse {
        self.client
            .post(self.url("/shutdown"))
            .json(&request)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap()
    }

    fn signal(&self, signal: &str) {
        Command::new("kill")
            .args([signal, &self.child.id().to_string()])
            .status()
            .unwrap();
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        drop(self.child.kill());
        drop(self.child.wait());
    }
}

fn parse_ndjson(text: &str) -> Vec<ExecEvent> {
    text.lines()
        .map(|line| serde_json::from_str(line).unwrap_or_else(|e| panic!("{e}: {line}")))
        .collect()
}

fn build_root() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("build.sbt"), "").unwrap();
    dir
}

fn pid_alive(pid: u32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .status()
        .is_ok_and(|s| s.success())
}

#[tokio::test]
async fn health_reports_the_daemon_pid_and_version() {
    let daemon = Daemon::start(&[]).await;
    let health = daemon.health().await.unwrap();
    assert_eq!(health.pid, daemon.child.id());
    assert_eq!(health.version, env!("CARGO_PKG_VERSION"));
    let pid_file = std::fs::read_to_string(daemon.state_dir.path().join("sbtd.pid")).unwrap();
    assert_eq!(pid_file.trim(), daemon.child.id().to_string());
}

fn mode(path: &Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

#[tokio::test]
async fn the_state_dir_and_its_files_are_private_to_the_owner() {
    let daemon = Daemon::start(&[]).await;
    let dir = daemon.state_dir.path();
    assert_eq!(mode(dir), 0o700, "an open state dir is restricted");
    for file in ["sbtd.pid", "sbtd.addr"] {
        assert_eq!(mode(&dir.join(file)), 0o600, "{file}");
    }
}

#[tokio::test]
async fn exec_streams_ndjson_ending_in_exactly_one_terminal_event() {
    let daemon = Daemon::start(&[]).await;
    let root = build_root();
    let events = daemon.exec_events(root.path(), "compile", false).await;
    assert!(
        matches!(
            &events[0],
            ExecEvent::Started {
                cold_start: true,
                ..
            }
        ),
        "{events:?}"
    );
    assert!(events.last().unwrap().is_terminal());
    assert_eq!(events.iter().filter(|e| e.is_terminal()).count(), 1);
    assert!(!events.iter().any(|e| matches!(
        e,
        ExecEvent::Log {
            level: LogLevel::Debug,
            ..
        }
    )));
    let Some(ExecEvent::Done {
        exit_code, summary, ..
    }) = events.last()
    else {
        panic!("expected done")
    };
    assert_eq!(*exit_code, 0);
    assert!(
        summary.starts_with("[success] 'compile' completed"),
        "{summary}"
    );

    let verbose = daemon.exec_events(root.path(), "compile", true).await;
    assert!(matches!(
        &verbose[0],
        ExecEvent::Started {
            cold_start: false,
            ..
        }
    ));
    assert!(verbose.iter().any(|e| matches!(
        e,
        ExecEvent::Log {
            level: LogLevel::Debug,
            ..
        }
    )));
}

#[tokio::test]
async fn a_directory_without_a_build_is_a_400_with_a_json_error() {
    let daemon = Daemon::start(&[]).await;
    let plain = tempfile::tempdir().unwrap();
    let response = daemon.exec(plain.path(), "compile", false).await;
    assert_eq!(response.status(), 400);
    let error: ErrorResponse = response.json().await.unwrap();
    assert_eq!(error.code, ErrorCode::NoBuildRoot);

    let response = daemon
        .exec(&build_root().path().join("nope"), "compile", false)
        .await;
    assert_eq!(response.status(), 400);
    let error: ErrorResponse = response.json().await.unwrap();
    assert_eq!(error.code, ErrorCode::BadRequest);
}

#[tokio::test]
async fn a_project_that_fails_to_load_is_a_500_carrying_the_boot_output() {
    let daemon = Daemon::start(&[(BOOT_FAIL_ENV, "1")]).await;
    let root = build_root();
    let response = daemon.exec(root.path(), "compile", false).await;
    assert_eq!(response.status(), 500);
    let error: ErrorResponse = response.json().await.unwrap();
    assert_eq!(error.code, ErrorCode::ProjectLoadFailed);
    let output = error.output.unwrap();
    assert!(
        output.iter().any(|l| l.contains("project loading failed")),
        "{output:?}"
    );
}

#[tokio::test]
async fn status_lists_warm_instances_and_shutdown_removes_them() {
    let daemon = Daemon::start(&[]).await;
    let root = build_root();
    daemon.exec_events(root.path(), "compile", false).await;

    let status = daemon.status().await;
    assert_eq!(status.instances.len(), 1);
    let instance = &status.instances[0];
    assert_eq!(instance.phase, InstancePhase::Idle);
    assert_eq!(instance.build_root, root.path().canonicalize().unwrap());
    let pid = instance.pid.unwrap();
    assert!(pid_alive(pid));

    let stopped = daemon
        .shutdown(ShutdownRequest::One {
            cwd: root.path().to_path_buf(),
        })
        .await;
    assert_eq!(stopped.stopped, vec![root.path().canonicalize().unwrap()]);
    assert!(daemon.status().await.instances.is_empty());
    assert!(!pid_alive(pid));

    let events = daemon.exec_events(root.path(), "compile", false).await;
    assert!(matches!(
        &events[0],
        ExecEvent::Started {
            cold_start: true,
            ..
        }
    ));
    let stopped = daemon.shutdown(ShutdownRequest::All).await;
    assert_eq!(stopped.stopped.len(), 1);
    assert!(daemon.status().await.instances.is_empty());
}

#[tokio::test]
async fn sigterm_stops_every_instance_and_removes_the_state_files() {
    let mut daemon = Daemon::start(&[]).await;
    let root = build_root();
    daemon.exec_events(root.path(), "compile", false).await;
    let pid = daemon.status().await.instances[0].pid.unwrap();

    daemon.signal("-TERM");
    let status = daemon.child.wait().unwrap();
    assert!(status.success(), "{status}");
    assert!(!pid_alive(pid));
    assert!(!daemon.state_dir.path().join("sbtd.pid").exists());
    assert!(!daemon.state_dir.path().join("sbtd.addr").exists());
    assert!(daemon.health().await.is_none());
}

#[tokio::test]
async fn an_exec_in_flight_during_shutdown_is_aborted_not_faked() {
    let mut daemon = Daemon::start(&[(SCENARIO_ENV, "exec=mute")]).await;
    let root = build_root();
    let response = daemon.exec(root.path(), "compile", false).await;
    let mut stream = response.bytes_stream();
    let first = stream.next().await.unwrap().unwrap();
    let first = parse_ndjson(std::str::from_utf8(&first).unwrap());
    assert!(matches!(&first[0], ExecEvent::Started { .. }));

    daemon.signal("-TERM");
    let mut rest = Vec::new();
    while let Some(chunk) = stream.next().await {
        rest.extend_from_slice(&chunk.unwrap());
    }
    let events = parse_ndjson(std::str::from_utf8(&rest).unwrap());
    assert!(
        matches!(
            events.last(),
            Some(ExecEvent::Aborted {
                reason: AbortReason::DaemonShutdown,
                ..
            })
        ),
        "{events:?}"
    );
    assert!(daemon.child.wait().unwrap().success());
}

#[tokio::test]
async fn a_non_loopback_bind_is_refused_before_anything_starts() {
    let state_dir = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_sbtd"))
        .args(["--bind", "0.0.0.0:0"])
        .arg("--state-dir")
        .arg(state_dir.path())
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("loopback"), "{stderr}");
    assert!(!state_dir.path().join("sbtd.pid").exists());
}

#[tokio::test]
async fn concurrent_execs_on_one_build_root_are_serialized() {
    let daemon = Daemon::start(&[(SCENARIO_ENV, "delay=300")]).await;
    let root = build_root();
    let started = Instant::now();
    let (a, b) = tokio::join!(
        daemon.exec_events(root.path(), "compile", false),
        daemon.exec_events(root.path(), "test", false)
    );
    assert!(a.last().unwrap().is_terminal() && b.last().unwrap().is_terminal());
    assert!(
        started.elapsed() >= Duration::from_millis(600),
        "execs overlapped: {:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn a_malformed_body_gets_sbtds_own_error_shape() {
    let daemon = Daemon::start(&[]).await;
    let response = daemon
        .client
        .post(daemon.url("/exec"))
        .header("content-type", "application/json")
        .body(r#"{"cwd":"/tmp"}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 400);
    let error: ErrorResponse = response.json().await.unwrap();
    assert_eq!(error.code, ErrorCode::BadRequest);
    assert!(error.message.contains("command_line"), "{}", error.message);
}
