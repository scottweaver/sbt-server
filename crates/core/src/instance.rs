//! One warm sbt process per build root: spawn it, take over any server that
//! was already there, run commands against it, shut it down. The pure parts
//! live in the child modules; this file is the async shell that wires the
//! child process, the JSON-RPC connection and the output router together.

pub mod completion;
pub mod descriptor;
pub mod events;
pub mod lines;
pub mod output;
pub mod session;
pub mod terminal;

use std::ffi::OsString;
use std::io;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::{Child, Command};
use tokio::sync::{Mutex, Notify, mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tokio::time::timeout;

pub use self::descriptor::{ConnectionInfo, DescriptorError, ServerUri, Token};
pub use self::output::{DeathCause, Phase};
use self::output::{ExecEnd, Input, Router};
pub use self::session::{
    CommandLine, EmptyCommandLine, ExecOutcome, ExitCode, HandshakeError, SbtSession,
    Unauthenticated,
};
use crate::jsonrpc::{self, BoxedIo, Connection, Notification};
use crate::protocol::ExecEvent;
use crate::routing::BuildRoot;

const INPUT_QUEUE: usize = 1024;
const EXEC_QUEUE: usize = 256;
const POLL_INTERVAL: Duration = Duration::from_millis(100);
const REAP_TIMEOUT: Duration = Duration::from_secs(10);
/// After sbt exits, how long to wait for its pipes to reach EOF before
/// reporting the exit anyway (a grandchild it left behind may hold them).
const PIPE_DRAIN_GRACE: Duration = Duration::from_secs(2);
/// A closed socket usually precedes the kernel's exit report by a moment;
/// wait this long before calling a lost connection anything but a death.
const DEATH_GRACE: Duration = Duration::from_secs(2);

static NEXT_INSTANCE_ID: AtomicU64 = AtomicU64::new(1);

/// Distinguishes one spawn from the next for the same build root, so a stale
/// handle can never evict a fresh replacement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct InstanceId(u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Pid(u32);

impl Pid {
    #[must_use]
    pub fn get(self) -> u32 {
        self.0
    }
}

/// Tells sbt a remote client started it, so it adds no console channel.
/// Without it the console shell redraws its prompt after every log line and
/// floods the raw stdout pipe with escape sequences; `--detach-stdio` cures
/// that only by closing the JVM's stdio, which loses the raw output gotcha 5
/// exists to capture. Confirmed live on sbt 1.11.7.
pub const TERMINAL_PROPS_ENV: &str = "SBT_TERMINAL_PROPS";
/// `width,height,ansi,color,supershell`, as sbt's thin client sends them.
pub const TERMINAL_PROPS_VALUE: &str = "80,24,false,false,false";

/// How to start sbt. The defaults reproduce what sbt's own thin client does
/// minus `--detach-stdio`: a server with no console channel, plain
/// uncoloured output, stdin `/dev/null`, stdout and stderr piped.
#[derive(Debug, Clone)]
pub struct Launcher {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub env: Vec<(OsString, OsString)>,
}

impl Default for Launcher {
    fn default() -> Self {
        Self {
            program: PathBuf::from("sbt"),
            args: [
                "--server",
                "-Dsbt.supershell=false",
                "-Dsbt.color=false",
                "-Dsbt.log.noformat=true",
            ]
            .into_iter()
            .map(OsString::from)
            .collect(),
            env: vec![(
                OsString::from(TERMINAL_PROPS_ENV),
                OsString::from(TERMINAL_PROPS_VALUE),
            )],
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Timeouts {
    /// Opening the socket named in `active.json`.
    pub connect: Duration,
    /// The `initialize` round trip.
    pub handshake: Duration,
    /// From spawn until `active.json` is readable (JVM boot + project load).
    pub boot: Duration,
    /// Waiting for a taken-over server to stop listening.
    pub takeover: Duration,
    /// The `shutdown` exec round trip (gotcha 4).
    pub shutdown: Duration,
}

impl Default for Timeouts {
    fn default() -> Self {
        Self {
            connect: Duration::from_secs(5),
            handshake: Duration::from_secs(15),
            boot: Duration::from_mins(3),
            takeover: Duration::from_secs(10),
            shutdown: Duration::from_secs(5),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct SbtConfig {
    pub launcher: Launcher,
    pub timeouts: Timeouts,
}

#[derive(Debug, Error)]
pub enum StartError {
    #[error("cannot spawn {program}: {source}")]
    Spawn {
        program: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("sbt {cause} before publishing its server; last output:\n{}", output.join("\n"))]
    ProjectLoadFailed {
        cause: DeathCause,
        output: Vec<String>,
    },
    #[error("sbt did not publish active.json within {waited:?}; last output:\n{}", output.join("\n"))]
    BootTimeout {
        waited: Duration,
        output: Vec<String>,
    },
    #[error("cannot connect to sbt at {uri}: {source}")]
    Connect {
        uri: ServerUri,
        #[source]
        source: io::Error,
    },
    #[error("connecting to sbt at {uri} timed out after {waited:?}")]
    ConnectTimeout { uri: ServerUri, waited: Duration },
    #[error("sbt handshake failed: {0}")]
    Handshake(#[from] HandshakeError),
    #[error("sbt handshake timed out after {waited:?}")]
    HandshakeTimeout { waited: Duration },
    #[error("the sbt start task ended without reporting an outcome")]
    Vanished,
}

impl StartError {
    /// Boot output captured before the failure, when there is any.
    #[must_use]
    pub fn output(&self) -> &[String] {
        match self {
            Self::ProjectLoadFailed { output, .. } | Self::BootTimeout { output, .. } => output,
            Self::Spawn { .. }
            | Self::Connect { .. }
            | Self::ConnectTimeout { .. }
            | Self::Handshake(_)
            | Self::HandshakeTimeout { .. }
            | Self::Vanished => &[],
        }
    }
}

/// The `shutdown` exec's fate; the child is reaped either way. Real sbt
/// announces `shutdown` and closes the socket without answering, so
/// `ConnectionClosed` is the normal cooperative outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShutdownRpc {
    Acked,
    TimedOut,
    ConnectionClosed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShutdownReport {
    pub rpc: ShutdownRpc,
    /// Whether the process was observed to exit within the reap timeout.
    pub reaped: bool,
}

/// Events of one exec, in order, ending with exactly one terminal event.
#[derive(Debug)]
pub struct ExecOutput(mpsc::Receiver<ExecEvent>);

impl ExecOutput {
    pub async fn next(&mut self) -> Option<ExecEvent> {
        self.0.recv().await
    }

    /// Drain until the terminal event (or the stream closes).
    pub async fn collect(mut self) -> Vec<ExecEvent> {
        let mut events = Vec::new();
        while let Some(event) = self.next().await {
            let terminal = event.is_terminal();
            events.push(event);
            if terminal {
                break;
            }
        }
        events
    }

    #[must_use]
    pub fn into_receiver(self) -> mpsc::Receiver<ExecEvent> {
        self.0
    }
}

/// A warm sbt server this process spawned and controls.
#[derive(Debug)]
pub struct Instance {
    id: InstanceId,
    pid: Pid,
    root: BuildRoot,
    session: SbtSession,
    inputs: mpsc::Sender<Input>,
    exec_lock: Arc<Mutex<()>>,
    phase: watch::Receiver<Phase>,
    kill: Arc<Notify>,
    started_at: Instant,
    timeouts: Timeouts,
}

impl Instance {
    /// Take over any server already described by `active.json`, spawn a
    /// fresh sbt, wait for its descriptor, connect and initialize.
    ///
    /// # Errors
    ///
    /// Spawn failure, the process dying or stalling during project load
    /// (with its captured output), or a connection/handshake failure. The
    /// child is killed on every error path.
    pub async fn start(config: &SbtConfig, root: BuildRoot) -> Result<Self, StartError> {
        take_over_existing_server(&root, &config.timeouts).await;
        let mut child = spawn(&config.launcher, &root)?;
        let pid = Pid(child.id().unwrap_or_default());
        let (inputs, inbox) = mpsc::channel(INPUT_QUEUE);
        let (phase_tx, phase) = watch::channel(Phase::Booting);
        tokio::spawn(Router::new(root.clone(), pid, inbox, phase_tx).run());
        let pumps = [
            child
                .stdout
                .take()
                .map(|stdout| tokio::spawn(pump(stdout, inputs.clone(), Input::Stdout))),
            child
                .stderr
                .take()
                .map(|stderr| tokio::spawn(pump(stderr, inputs.clone(), Input::Stderr))),
        ];
        let kill = Arc::new(Notify::new());
        tokio::spawn(supervise(child, Arc::clone(&kill), inputs.clone(), pumps));
        let guard = KillOnDrop(Arc::clone(&kill));

        let boot = Boot {
            root: &root,
            inputs: &inputs,
            phase: &phase,
            timeouts: &config.timeouts,
        };
        let info = boot.wait_for_descriptor().await?;
        let session = boot.connect_and_initialize(&info).await?;
        drop(inputs.send(Input::Booted).await);
        let mut booted = phase.clone();
        drop(booted.wait_for(|p| !p.is_booting()).await);
        guard.disarm();
        tracing::info!(%root, pid = pid.get(), uri = %info.uri, "sbt server ready");
        Ok(Self {
            id: InstanceId(NEXT_INSTANCE_ID.fetch_add(1, Ordering::Relaxed)),
            pid,
            root,
            session,
            inputs,
            exec_lock: Arc::new(Mutex::new(())),
            phase,
            kill,
            started_at: Instant::now(),
            timeouts: config.timeouts,
        })
    }

    #[must_use]
    pub fn id(&self) -> InstanceId {
        self.id
    }

    #[must_use]
    pub fn pid(&self) -> Pid {
        self.pid
    }

    #[must_use]
    pub fn root(&self) -> &BuildRoot {
        &self.root
    }

    #[must_use]
    pub fn started_at(&self) -> Instant {
        self.started_at
    }

    /// A snapshot of the current phase.
    #[must_use]
    pub fn phase(&self) -> Phase {
        self.phase.borrow().clone()
    }

    /// A receiver that observes every phase change.
    #[must_use]
    pub fn phase_watch(&self) -> watch::Receiver<Phase> {
        self.phase.clone()
    }

    /// Run one command. Execs on one instance are serialized; the exec runs
    /// to sbt's terminal reply even if the returned output is dropped, so a
    /// caller that goes away can never leave sbt busy with the lock free.
    #[must_use]
    pub fn exec(&self, command: CommandLine, verbose: bool) -> ExecOutput {
        let (sink, events) = mpsc::channel(EXEC_QUEUE);
        let lock = Arc::clone(&self.exec_lock);
        let session = self.session.clone();
        let inputs = self.inputs.clone();
        let mut phase = self.phase.clone();
        tokio::spawn(async move {
            let _guard = lock.lock_owned().await;
            let begin = Input::BeginExec {
                command: command.clone(),
                verbose,
                sink,
            };
            if inputs.send(begin).await.is_err() {
                return;
            }
            let started = Instant::now();
            let end = if let Ok(outcome) = session.exec(&command).await {
                ExecEnd::Answered {
                    outcome,
                    elapsed: started.elapsed(),
                }
            } else {
                drop(timeout(DEATH_GRACE, phase.wait_for(Phase::is_dead)).await);
                ExecEnd::ConnectionClosed
            };
            drop(inputs.send(Input::EndExec(end)).await);
        });
        ExecOutput(events)
    }

    /// Ask sbt to shut down, bounded, then make sure the process is gone.
    pub async fn shutdown(&self) -> ShutdownReport {
        drop(self.inputs.send(Input::ShutdownRequested).await);
        let rpc = match timeout(
            self.timeouts.shutdown,
            self.session.exec(&CommandLine::shutdown()),
        )
        .await
        {
            Ok(Ok(_)) => ShutdownRpc::Acked,
            Ok(Err(session::ConnectionClosed)) => ShutdownRpc::ConnectionClosed,
            Err(_) => ShutdownRpc::TimedOut,
        };
        tracing::info!(root = %self.root, pid = self.pid.get(), ?rpc, "sbt shutdown requested");
        if rpc != ShutdownRpc::TimedOut && self.wait_for_death(self.timeouts.shutdown).await {
            return ShutdownReport { rpc, reaped: true };
        }
        self.kill.notify_one();
        let reaped = self.wait_for_death(REAP_TIMEOUT).await;
        ShutdownReport { rpc, reaped }
    }

    async fn wait_for_death(&self, limit: Duration) -> bool {
        let mut phase = self.phase.clone();
        matches!(
            timeout(limit, phase.wait_for(Phase::is_dead)).await,
            Ok(Ok(_))
        )
    }
}

struct KillOnDrop(Arc<Notify>);

impl KillOnDrop {
    fn disarm(self) {
        std::mem::forget(self);
    }
}

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        self.0.notify_one();
    }
}

fn spawn(launcher: &Launcher, root: &BuildRoot) -> Result<Child, StartError> {
    tracing::info!(%root, program = %launcher.program.display(), "spawning sbt");
    Command::new(&launcher.program)
        .args(&launcher.args)
        .envs(launcher.env.iter().map(|(k, v)| (k, v)))
        .current_dir(root.as_path())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|source| StartError::Spawn {
            program: launcher.program.clone(),
            source,
        })
}

async fn pump<R: AsyncRead + Unpin>(
    mut reader: R,
    inputs: mpsc::Sender<Input>,
    wrap: fn(Vec<u8>) -> Input,
) {
    let mut chunk = [0u8; 4096];
    loop {
        match reader.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if inputs.send(wrap(chunk[..n].to_vec())).await.is_err() {
                    break;
                }
            }
        }
    }
}

/// Sole owner of the child: reports its exit, and kills it on request. The
/// exit is reported only after both pipe pumps have forwarded everything
/// (bounded by `PIPE_DRAIN_GRACE`), so the last lines a dying sbt wrote,
/// the ones that explain a failed project load, precede `ChildExited` on
/// the instance channel instead of racing it.
async fn supervise(
    mut child: Child,
    kill: Arc<Notify>,
    inputs: mpsc::Sender<Input>,
    pumps: [Option<JoinHandle<()>>; 2],
) {
    let status = tokio::select! {
        status = child.wait() => status,
        () = kill.notified() => {
            drop(child.start_kill());
            child.wait().await
        }
    };
    for pump in pumps.into_iter().flatten() {
        if timeout(PIPE_DRAIN_GRACE, pump).await.is_err() {
            tracing::debug!("a pipe is still open after sbt exited; reporting the exit anyway");
        }
    }
    drop(inputs.send(Input::ChildExited(status.ok())).await);
}

struct Boot<'a> {
    root: &'a BuildRoot,
    inputs: &'a mpsc::Sender<Input>,
    phase: &'a watch::Receiver<Phase>,
    timeouts: &'a Timeouts,
}

impl Boot<'_> {
    async fn wait_for_descriptor(&self) -> Result<ConnectionInfo, StartError> {
        let path = self.root.active_json();
        let deadline = Instant::now() + self.timeouts.boot;
        loop {
            let died = self.phase.borrow().death_cause().cloned();
            if let Some(cause) = died {
                return Err(StartError::ProjectLoadFailed {
                    cause,
                    output: self.boot_output().await,
                });
            }
            match descriptor::read(&path).await {
                Ok(info) => return Ok(info),
                Err(error) if error.is_not_found() => {}
                Err(error) => tracing::debug!(%error, "descriptor not readable yet"),
            }
            if Instant::now() >= deadline {
                return Err(StartError::BootTimeout {
                    waited: self.timeouts.boot,
                    output: self.boot_output().await,
                });
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    async fn boot_output(&self) -> Vec<String> {
        let (reply, output) = oneshot::channel();
        if self.inputs.send(Input::BootOutput(reply)).await.is_err() {
            return Vec::new();
        }
        output.await.unwrap_or_default()
    }

    async fn connect_and_initialize(
        &self,
        info: &ConnectionInfo,
    ) -> Result<SbtSession, StartError> {
        let io = timeout(self.timeouts.connect, connect(&info.uri))
            .await
            .map_err(|_| StartError::ConnectTimeout {
                uri: info.uri.clone(),
                waited: self.timeouts.connect,
            })?
            .map_err(|source| StartError::Connect {
                uri: info.uri.clone(),
                source,
            })?;
        let connection = Connection::open(io, self.inputs.clone(), terminal::answer);
        let handshake = Unauthenticated::new(connection).initialize(info.token.as_ref());
        timeout(self.timeouts.handshake, handshake)
            .await
            .map_err(|_| StartError::HandshakeTimeout {
                waited: self.timeouts.handshake,
            })?
            .map_err(StartError::Handshake)
    }
}

async fn connect(uri: &ServerUri) -> io::Result<BoxedIo> {
    match uri {
        ServerUri::Tcp(addr) => jsonrpc::connect_tcp(*addr).await,
        ServerUri::Local(path) => jsonrpc::connect_unix(path).await,
    }
}

/// Gotcha 3: never trust a pre-existing `active.json`. Ask whatever it
/// describes to shut down (bounded), delete it, wait for the endpoint to
/// stop listening so its exit hook cannot delete the replacement's
/// descriptor, then let the caller spawn fresh.
async fn take_over_existing_server(root: &BuildRoot, timeouts: &Timeouts) {
    let path = root.active_json();
    let info = match descriptor::read(&path).await {
        Ok(info) => info,
        Err(error) if error.is_not_found() => return,
        Err(error) => {
            tracing::warn!(%root, %error, "removing an unreadable server descriptor");
            drop(tokio::fs::remove_file(&path).await);
            return;
        }
    };
    tracing::info!(%root, uri = %info.uri, "taking over a pre-existing sbt server");
    let rpc = request_foreign_shutdown(&info, timeouts).await;
    tracing::info!(%root, ?rpc, "foreign server shutdown requested");
    drop(tokio::fs::remove_file(&path).await);
    wait_until_refused(&info.uri, timeouts.takeover).await;
}

async fn request_foreign_shutdown(info: &ConnectionInfo, timeouts: &Timeouts) -> ShutdownRpc {
    let Ok(Ok(io)) = timeout(timeouts.connect, connect(&info.uri)).await else {
        return ShutdownRpc::ConnectionClosed;
    };
    let (sink, mut drain) = mpsc::channel::<Notification>(64);
    tokio::spawn(async move { while drain.recv().await.is_some() {} });
    let handshake = Unauthenticated::new(Connection::open(io, sink, terminal::answer))
        .initialize(info.token.as_ref());
    let Ok(Ok(session)) = timeout(timeouts.handshake, handshake).await else {
        return ShutdownRpc::ConnectionClosed;
    };
    match timeout(timeouts.shutdown, session.exec(&CommandLine::shutdown())).await {
        Ok(Ok(_)) => ShutdownRpc::Acked,
        Ok(Err(session::ConnectionClosed)) => ShutdownRpc::ConnectionClosed,
        Err(_) => ShutdownRpc::TimedOut,
    }
}

async fn wait_until_refused(uri: &ServerUri, limit: Duration) {
    let deadline = Instant::now() + limit;
    loop {
        if connect(uri).await.is_err() {
            return;
        }
        if Instant::now() >= deadline {
            tracing::warn!(%uri, ?limit, "foreign sbt server still listening; spawning anyway");
            return;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}
