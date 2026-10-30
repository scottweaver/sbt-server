//! `sbtctl` as a real process against an in-test daemon that speaks the
//! contract, plus one lifecycle test against the real `sbtd` binary.

use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use axum::extract::State;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use sbt_server_core::protocol::{
    AbortReason, ExecEvent, ExecRequest, HealthResponse, HealthStatus, InstancePhase,
    InstanceStatus, LogLevel, OutputStream, ShutdownRequest, ShutdownResponse, StatusResponse,
};

struct FakeState {
    events: Vec<ExecEvent>,
    error: Option<(u16, String)>,
    execs: Mutex<Vec<ExecRequest>>,
    shutdowns: Mutex<Vec<ShutdownRequest>>,
}

struct FakeDaemon {
    url: String,
    state: Arc<FakeState>,
    task: tokio::task::JoinHandle<()>,
}

impl FakeDaemon {
    async fn start(events: Vec<ExecEvent>) -> Self {
        Self::new(events, None).await
    }

    /// Answers every `/exec` with this status and body instead of events.
    async fn start_failing(status: u16, body: &str) -> Self {
        Self::new(Vec::new(), Some((status, body.to_owned()))).await
    }

    async fn new(events: Vec<ExecEvent>, error: Option<(u16, String)>) -> Self {
        let state = Arc::new(FakeState {
            events,
            error,
            execs: Mutex::new(Vec::new()),
            shutdowns: Mutex::new(Vec::new()),
        });
        let router = Router::new()
            .route("/health", get(health))
            .route("/status", get(status))
            .route("/exec", post(exec))
            .route("/shutdown", post(shutdown))
            .with_state(Arc::clone(&state));
        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let addr = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        Self {
            url: format!("http://{addr}"),
            state,
            task,
        }
    }

    fn execs(&self) -> Vec<ExecRequest> {
        self.state
            .execs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn shutdowns(&self) -> Vec<ShutdownRequest> {
        self.state
            .shutdowns
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl Drop for FakeDaemon {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: HealthStatus::Ok,
        pid: 4242,
        version: "fake".into(),
        uptime_secs: 7,
    })
}

async fn status() -> Json<StatusResponse> {
    Json(StatusResponse {
        instances: vec![InstanceStatus {
            build_root: PathBuf::from("/work/proj"),
            pid: Some(99),
            phase: InstancePhase::Idle,
            idle_secs: Some(12),
            uptime_secs: 340,
        }],
    })
}

async fn exec(
    State(state): State<Arc<FakeState>>,
    Json(request): Json<ExecRequest>,
) -> axum::response::Response {
    state
        .execs
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(request);
    if let Some((status, body)) = &state.error {
        let status = axum::http::StatusCode::from_u16(*status).unwrap();
        return (status, body.clone()).into_response();
    }
    state
        .events
        .iter()
        .map(|event| {
            serde_json::to_string(event).unwrap()
                + "
"
        })
        .collect::<String>()
        .into_response()
}

async fn shutdown(
    State(state): State<Arc<FakeState>>,
    Json(request): Json<ShutdownRequest>,
) -> Json<ShutdownResponse> {
    state
        .shutdowns
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(request);
    Json(ShutdownResponse {
        stopped: vec![PathBuf::from("/work/proj")],
    })
}

/// A state dir of this test run's own, so sbtctl never reads the token of
/// a daemon that happens to be running for the developer.
fn isolated_state_dir() -> &'static Path {
    static DIR: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
    DIR.get_or_init(|| tempfile::tempdir().unwrap()).path()
}

fn sbtctl(url: &str) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_sbtctl"));
    command
        .env("SBTCTL_DAEMON_URL", url)
        .env("SBTD_STATE_DIR", isolated_state_dir());
    command
}

fn run(command: &mut Command) -> (Output, String, String) {
    let output = command.output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    (output, stdout, stderr)
}

fn sbtd_path() -> PathBuf {
    let sbtd = PathBuf::from(env!("CARGO_BIN_EXE_sbtctl"))
        .parent()
        .unwrap()
        .join("sbtd");
    assert!(
        sbtd.is_file(),
        "sbtd must be built next to sbtctl (cargo test --workspace does that)"
    );
    sbtd
}

struct RealSbtd {
    child: Child,
    port: u16,
    state_dir: tempfile::TempDir,
}

impl RealSbtd {
    fn start() -> Self {
        let state_dir = tempfile::tempdir().unwrap();
        let child = Command::new(sbtd_path())
            .args(["--bind", "127.0.0.1:0", "--idle-timeout-secs", "0"])
            .arg("--state-dir")
            .arg(state_dir.path())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let addr_file = state_dir.path().join("sbtd.addr");
        let deadline = Instant::now() + Duration::from_secs(10);
        let addr: SocketAddr = loop {
            if let Ok(text) = std::fs::read_to_string(&addr_file)
                && let Ok(addr) = text.parse()
            {
                break addr;
            }
            assert!(Instant::now() < deadline, "sbtd never wrote its address");
            std::thread::sleep(Duration::from_millis(20));
        };
        Self {
            child,
            port: addr.port(),
            state_dir,
        }
    }
}

impl Drop for RealSbtd {
    fn drop(&mut self) {
        drop(self.child.kill());
        drop(self.child.wait());
    }
}

fn scripted_events(exit_code: i32) -> Vec<ExecEvent> {
    vec![
        ExecEvent::Started {
            build_root: PathBuf::from("/work/proj"),
            pid: 99,
            cold_start: true,
        },
        ExecEvent::Log {
            level: LogLevel::Info,
            text: "compiling 1 Scala source".into(),
        },
        ExecEvent::Output {
            stream: OutputStream::SbtStdout,
            text: "hello from println".into(),
        },
        ExecEvent::Output {
            stream: OutputStream::Stderr,
            text: "a warning on stderr".into(),
        },
        ExecEvent::Diagnostics {
            file: PathBuf::from("/work/proj/A.scala"),
            items: Vec::new(),
        },
        ExecEvent::done(exit_code, 12, format!("[done] exit {exit_code}")),
    ]
}

#[tokio::test(flavor = "multi_thread")]
async fn passthrough_sends_the_joined_words_and_the_directory() {
    let daemon = FakeDaemon::start(scripted_events(0)).await;
    let dir = tempfile::tempdir().unwrap();
    let (output, _, _) = run(sbtctl(&daemon.url).arg("-C").arg(dir.path()).args([
        "--verbose",
        "testOnly",
        "com.ex.FooSpec",
    ]));
    assert_eq!(output.status.code(), Some(0));
    let execs = daemon.execs();
    assert_eq!(execs.len(), 1);
    assert_eq!(execs[0].cwd, dir.path());
    assert_eq!(execs[0].command_line, "testOnly com.ex.FooSpec");
    assert!(execs[0].verbose);

    let (_, _, _) = run(sbtctl(&daemon.url).current_dir(dir.path()).arg("compile"));
    let execs = daemon.execs();
    assert_eq!(
        execs[1].cwd.canonicalize().unwrap(),
        dir.path().canonicalize().unwrap()
    );
    assert_eq!(execs[1].command_line, "compile");
    assert!(!execs[1].verbose);
}

#[tokio::test(flavor = "multi_thread")]
async fn events_are_rendered_to_the_right_streams_and_sbts_exit_code_is_kept() {
    let daemon = FakeDaemon::start(scripted_events(3)).await;
    let (output, stdout, stderr) = run(sbtctl(&daemon.url).args(["-C", "/tmp", "compile"]));
    assert_eq!(output.status.code(), Some(3));
    assert_eq!(
        stdout,
        "[info] compiling 1 Scala source\nhello from println\n[done] exit 3\n"
    );
    assert!(
        stderr.contains("[sbtctl] starting sbt for /work/proj (cold start)"),
        "{stderr}"
    );
    assert!(stderr.contains("a warning on stderr"), "{stderr}");
    assert!(!stdout.contains("A.scala") && !stderr.contains("A.scala"));
}

#[tokio::test(flavor = "multi_thread")]
async fn exit_codes_are_clamped_and_aborts_exit_125() {
    let daemon = FakeDaemon::start(scripted_events(300)).await;
    let (output, _, _) = run(sbtctl(&daemon.url).args(["-C", "/tmp", "compile"]));
    assert_eq!(output.status.code(), Some(124));

    let aborted = vec![
        ExecEvent::Started {
            build_root: PathBuf::from("/work/proj"),
            pid: 99,
            cold_start: false,
        },
        ExecEvent::Aborted {
            reason: AbortReason::SbtDied,
            summary: "'compile' aborted: the sbt process died".into(),
        },
    ];
    let daemon = FakeDaemon::start(aborted).await;
    let (output, _, stderr) = run(sbtctl(&daemon.url).args(["-C", "/tmp", "compile"]));
    assert_eq!(output.status.code(), Some(125));
    assert!(stderr.contains("the sbt process died"), "{stderr}");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unreachable_daemon_exits_125_with_a_hint() {
    let closed = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let addr: SocketAddr = closed.local_addr().unwrap();
    drop(closed);
    let (output, _, stderr) =
        run(sbtctl(&format!("http://{addr}")).args(["-C", "/tmp", "compile"]));
    assert_eq!(output.status.code(), Some(125));
    assert!(stderr.contains("cannot reach sbtd"), "{stderr}");
    assert!(stderr.contains("sbtctl daemon start"), "{stderr}");

    let (output, stdout, _) = run(sbtctl(&format!("http://{addr}")).args(["daemon", "status"]));
    assert_eq!(output.status.code(), Some(1));
    assert!(stdout.contains("not running"), "{stdout}");
}

#[tokio::test(flavor = "multi_thread")]
async fn status_and_shutdown_speak_the_contract() {
    let daemon = FakeDaemon::start(Vec::new()).await;
    let (output, stdout, _) = run(sbtctl(&daemon.url).arg("status"));
    assert_eq!(output.status.code(), Some(0));
    assert!(stdout.contains("/work/proj"), "{stdout}");
    assert!(stdout.contains("idle"), "{stdout}");
    assert!(stdout.contains("99"), "{stdout}");

    let (output, stdout, _) = run(sbtctl(&daemon.url).args(["shutdown", "--all"]));
    assert_eq!(output.status.code(), Some(0));
    assert!(stdout.contains("stopped sbt for /work/proj"), "{stdout}");
    let (output, _, _) = run(sbtctl(&daemon.url).args(["-C", "/tmp", "shutdown"]));
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        daemon.shutdowns(),
        vec![
            ShutdownRequest::All,
            ShutdownRequest::One {
                cwd: PathBuf::from("/tmp")
            }
        ]
    );

    let (output, stdout, _) = run(sbtctl(&daemon.url).args(["daemon", "status"]));
    assert_eq!(output.status.code(), Some(0));
    assert!(stdout.contains("pid 4242"), "{stdout}");
}

fn mode(path: &Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

#[tokio::test(flavor = "multi_thread")]
async fn daemon_start_status_stop_drive_the_real_sbtd() {
    let free = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let url = format!("http://{}", free.local_addr().unwrap());
    drop(free);
    let tmp = tempfile::tempdir().unwrap();
    let state_dir = tmp.path().join("sbt-server");
    let ctl = || {
        let mut command = sbtctl(&url);
        command.arg("--state-dir").arg(&state_dir);
        command
    };

    let (output, stdout, stderr) = run(ctl().args(["daemon", "start"]));
    assert_eq!(output.status.code(), Some(0), "{stderr}");
    assert!(stdout.starts_with("sbtd started (pid "), "{stdout}");
    assert_eq!(mode(&state_dir), 0o700);
    for file in ["sbtd.pid", "sbtd.addr", "sbtd.token", "sbtd.log"] {
        assert_eq!(mode(&state_dir.join(file)), 0o600, "{file}");
    }

    let (output, _, stderr) = run(ctl().args(["daemon", "start"]));
    assert_eq!(output.status.code(), Some(125));
    assert!(stderr.contains("already running"), "{stderr}");

    let (output, stdout, _) = run(ctl().args(["daemon", "status"]));
    assert_eq!(output.status.code(), Some(0));
    assert!(stdout.starts_with("sbtd running: pid "), "{stdout}");

    let (output, stdout, _) = run(ctl().arg("status"));
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(stdout, "no warm sbt instances\n");

    let (output, stdout, stderr) = run(ctl().args(["daemon", "restart"]));
    assert_eq!(output.status.code(), Some(0), "{stderr}");
    assert!(stdout.starts_with("sbtd restarted (pid "), "{stdout}");

    let (output, stdout, stderr) = run(ctl().args(["daemon", "stop"]));
    assert_eq!(output.status.code(), Some(0), "{stderr}");
    assert!(stdout.starts_with("sbtd stopped (pid "), "{stdout}");
    assert!(!state_dir.join("sbtd.pid").exists());

    let (output, stdout, _) = run(ctl().args(["daemon", "stop"]));
    assert_eq!(output.status.code(), Some(0));
    assert!(stdout.contains("not running"), "{stdout}");
    let (output, _, _) = run(ctl().args(["daemon", "status"]));
    assert_eq!(output.status.code(), Some(1));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_bare_daemon_verb_is_refused_with_a_pointer_to_daemon() {
    let daemon = FakeDaemon::start(scripted_events(0)).await;
    for verb in ["start", "stop", "restart"] {
        let (output, _, stderr) = run(sbtctl(&daemon.url).args(["-C", "/tmp", verb]));
        assert_eq!(output.status.code(), Some(2), "{verb}: {stderr}");
        assert!(
            stderr.contains(&format!("sbtctl daemon {verb}")),
            "{stderr}"
        );
        assert!(stderr.contains(&format!("sbtctl -- {verb}")), "{stderr}");
    }
    assert!(daemon.execs().is_empty(), "nothing was sent to sbt");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_double_dash_sends_the_word_to_sbt_anyway() {
    let daemon = FakeDaemon::start(scripted_events(0)).await;
    let (output, _, _) = run(sbtctl(&daemon.url).args(["-C", "/tmp", "--", "start"]));
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(daemon.execs()[0].command_line, "start");
}

#[tokio::test(flavor = "multi_thread")]
async fn sbtds_own_errors_come_with_a_hint() {
    let daemon = FakeDaemon::start_failing(
        400,
        r#"{"code":"no_build_root","message":"no build.sbt or project/build.properties in /tmp or any parent directory"}"#,
    )
    .await;
    let (output, _, stderr) = run(sbtctl(&daemon.url).args(["-C", "/tmp", "compile"]));
    assert_eq!(output.status.code(), Some(125));
    assert!(stderr.contains("no build.sbt"), "{stderr}");
    assert!(stderr.contains("-C <dir>"), "{stderr}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_server_that_is_not_sbtd_is_named_as_such() {
    let daemon = FakeDaemon::start_failing(400, r#"{"message":"missing field `command`"}"#).await;
    let (output, _, stderr) = run(sbtctl(&daemon.url).args(["-C", "/tmp", "compile"]));
    assert_eq!(output.status.code(), Some(125));
    assert!(
        stderr.contains("does not speak sbtd's protocol"),
        "{stderr}"
    );
    assert!(stderr.contains("missing field"), "{stderr}");
    assert!(stderr.contains("--daemon-url"), "{stderr}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_state_dir_without_the_daemons_token_is_refused_with_a_hint() {
    let sbtd = RealSbtd::start();
    let url = format!("http://127.0.0.1:{}", sbtd.port);
    let (output, _, stderr) = run(sbtctl(&url).arg("status"));
    assert_eq!(output.status.code(), Some(125), "{stderr}");
    assert!(stderr.contains("401"), "{stderr}");
    assert!(stderr.contains("sbtd.token"), "{stderr}");
    assert!(stderr.contains("--state-dir"), "{stderr}");
    let (output, stdout, stderr) = run(sbtctl(&url).args(["daemon", "status"]));
    assert_eq!(output.status.code(), Some(0), "{stderr}");
    assert!(
        stdout.starts_with("sbtd running: pid "),
        "/health needs no token: {stdout}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_corrupt_token_file_is_reported_before_any_request() {
    let sbtd = RealSbtd::start();
    let url = format!("http://127.0.0.1:{}", sbtd.port);
    let state_dir = tempfile::tempdir().unwrap();
    std::fs::write(state_dir.path().join("sbtd.token"), "not a token\n").unwrap();
    let (output, _, stderr) = run(sbtctl(&url)
        .arg("--state-dir")
        .arg(state_dir.path())
        .arg("status"));
    assert_eq!(output.status.code(), Some(125), "{stderr}");
    assert!(stderr.contains("does not hold a token"), "{stderr}");
    assert!(stderr.contains("sbtd.token"), "{stderr}");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_localhost_daemon_url_is_served_by_the_real_sbtd() {
    let sbtd = RealSbtd::start();
    let url = format!("http://localhost:{}", sbtd.port);
    let ctl = || {
        let mut command = sbtctl(&url);
        command.arg("--state-dir").arg(sbtd.state_dir.path());
        command
    };
    let (output, stdout, stderr) = run(ctl().arg("status"));
    assert_eq!(output.status.code(), Some(0), "{stderr}");
    assert_eq!(stdout, "no warm sbt instances\n");
    let (output, stdout, stderr) = run(ctl().args(["daemon", "status"]));
    assert_eq!(output.status.code(), Some(0), "{stderr}");
    assert!(stdout.starts_with("sbtd running: pid "), "{stdout}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_forbidden_answer_points_at_the_daemon_url() {
    let daemon = FakeDaemon::start_failing(
        403,
        r#"{"code":"forbidden","message":"Host \"build.example:8077\" is not loopback; sbtd answers only to Host 127.0.0.1, [::1] or localhost"}"#,
    )
    .await;
    let (output, _, stderr) = run(sbtctl(&daemon.url).args(["-C", "/tmp", "compile"]));
    assert_eq!(output.status.code(), Some(125));
    assert!(stderr.contains("not loopback"), "{stderr}");
    assert!(stderr.contains("--daemon-url"), "{stderr}");
}
