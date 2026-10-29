//! A scripted stand-in for sbt's JSON-RPC server, for tests only.
//!
//! Every frame it writes is a literal string frozen under `fixtures/`, never
//! produced by sbt-server-core, so a symmetric framing bug in the client has
//! nowhere to hide. Request handling mirrors sbt: a configured token must
//! arrive in `initialize` before anything else is accepted, unknown methods
//! are dropped without a reply, and `shutdown` is an ordinary `sbt/exec`.

use std::fmt;
use std::io;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, UnixListener};
use tokio::sync::watch;
use tokio::task::JoinHandle;

const INITIALIZE: &str = include_str!("../fixtures/initialize.jsonl");
const HAPPY: &str = include_str!("../fixtures/happy.jsonl");
const FAILS: &str = include_str!("../fixtures/fails.jsonl");
const SHUTDOWN: &str = include_str!("../fixtures/shutdown.jsonl");

/// Environment variables the `fake-sbt` binary reads.
pub const SCENARIO_ENV: &str = "FAKE_SBT_SCENARIO";
pub const TRANSPORT_ENV: &str = "FAKE_SBT_TRANSPORT";
pub const TOKEN_ENV: &str = "FAKE_SBT_TOKEN";
pub const BOOT_DELAY_MS_ENV: &str = "FAKE_SBT_BOOT_DELAY_MS";
pub const BOOT_FAIL_ENV: &str = "FAKE_SBT_BOOT_FAIL";
/// When set, the process prints `fake-sbt: exec <command>` to its own stdout
/// before answering each exec, so raw-pipe capture can be tested.
pub const PRINT_ON_EXEC_ENV: &str = "FAKE_SBT_PRINT_ON_EXEC";

/// What the fake plays for each `sbt/exec` other than `shutdown`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecScript {
    Happy,
    FailsWith(i64),
    /// Never answers, like sbt faced with a request it does not recognise.
    Mute,
}

/// How the fake reacts to an `sbt/exec` of `shutdown`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShutdownBehaviour {
    /// Like sbt: announce `shutdown`, delete `active.json`, stop listening
    /// and close the connection without answering the request.
    Cooperative,
    Mute,
    /// Answer and close, but keep listening and keep `active.json` until the
    /// delay elapses, like a JVM whose exit hook is still running.
    DelayedExit(Duration),
}

/// Text form: `exec=happy|fails:<code>|mute;delay=<ms>;shutdown=cooperative|mute|delayed:<ms>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scenario {
    pub exec: ExecScript,
    /// Pause before answering an exec, so tests can observe serialization.
    pub exec_delay: Duration,
    pub shutdown: ShutdownBehaviour,
}

impl Default for Scenario {
    fn default() -> Self {
        Self {
            exec: ExecScript::Happy,
            exec_delay: Duration::ZERO,
            shutdown: ShutdownBehaviour::Cooperative,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigError(String);

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ConfigError {}

impl FromStr for Scenario {
    type Err = ConfigError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let mut scenario = Self::default();
        for pair in text.split(';').map(str::trim).filter(|p| !p.is_empty()) {
            let (key, value) = pair
                .split_once('=')
                .ok_or_else(|| ConfigError(format!("expected key=value, got {pair:?}")))?;
            let value = value.trim();
            match key.trim() {
                "exec" => scenario.exec = parse_exec(value)?,
                "delay" => scenario.exec_delay = Duration::from_millis(parse_millis(value)?),
                "shutdown" => scenario.shutdown = parse_shutdown(value)?,
                other => return Err(ConfigError(format!("unknown scenario key {other:?}"))),
            }
        }
        Ok(scenario)
    }
}

impl fmt::Display for Scenario {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.exec {
            ExecScript::Happy => f.write_str("exec=happy")?,
            ExecScript::FailsWith(code) => write!(f, "exec=fails:{code}")?,
            ExecScript::Mute => f.write_str("exec=mute")?,
        }
        write!(f, ";delay={}", self.exec_delay.as_millis())?;
        match self.shutdown {
            ShutdownBehaviour::Cooperative => f.write_str(";shutdown=cooperative"),
            ShutdownBehaviour::Mute => f.write_str(";shutdown=mute"),
            ShutdownBehaviour::DelayedExit(d) => write!(f, ";shutdown=delayed:{}", d.as_millis()),
        }
    }
}

fn parse_exec(value: &str) -> Result<ExecScript, ConfigError> {
    match value {
        "happy" => Ok(ExecScript::Happy),
        "mute" => Ok(ExecScript::Mute),
        other => other
            .strip_prefix("fails:")
            .and_then(|code| code.parse().ok())
            .map(ExecScript::FailsWith)
            .ok_or_else(|| ConfigError(format!("bad exec script {other:?}"))),
    }
}

fn parse_shutdown(value: &str) -> Result<ShutdownBehaviour, ConfigError> {
    match value {
        "cooperative" => Ok(ShutdownBehaviour::Cooperative),
        "mute" => Ok(ShutdownBehaviour::Mute),
        other => other
            .strip_prefix("delayed:")
            .map(parse_millis)
            .transpose()?
            .map(|ms| ShutdownBehaviour::DelayedExit(Duration::from_millis(ms)))
            .ok_or_else(|| ConfigError(format!("bad shutdown behaviour {other:?}"))),
    }
}

fn parse_millis(value: &str) -> Result<u64, ConfigError> {
    value
        .parse()
        .map_err(|_| ConfigError(format!("expected milliseconds, got {value:?}")))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Transport {
    Tcp,
    Local(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    pub transport: Transport,
    pub token: Option<String>,
    pub scenario: Scenario,
    /// Print a line to the process stdout before answering each exec.
    pub print_on_exec: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            transport: Transport::Tcp,
            token: None,
            scenario: Scenario::default(),
            print_on_exec: false,
        }
    }
}

impl Options {
    /// Read [`SCENARIO_ENV`], [`TRANSPORT_ENV`] (`tcp` or `local:<path>`) and
    /// [`TOKEN_ENV`]; anything unset keeps its default.
    ///
    /// # Errors
    ///
    /// A variable is set but unparseable.
    pub fn from_env() -> Result<Self, ConfigError> {
        let scenario = match std::env::var(SCENARIO_ENV) {
            Ok(text) => text.parse()?,
            Err(_) => Scenario::default(),
        };
        let transport = match std::env::var(TRANSPORT_ENV) {
            Err(_) => Transport::Tcp,
            Ok(text) if text == "tcp" => Transport::Tcp,
            Ok(text) => text
                .strip_prefix("local:")
                .map(|path| Transport::Local(PathBuf::from(path)))
                .ok_or_else(|| {
                    ConfigError(format!(
                        "{TRANSPORT_ENV} must be `tcp` or `local:<path>`, got {text:?}"
                    ))
                })?,
        };
        Ok(Self {
            transport,
            token: std::env::var(TOKEN_ENV).ok(),
            scenario,
            print_on_exec: std::env::var_os(PRINT_ON_EXEC_ENV).is_some(),
        })
    }
}

/// Where a started fake listens, in the two forms sbt publishes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Endpoint {
    Tcp(SocketAddr),
    Local(PathBuf),
}

impl Endpoint {
    /// The `uri` field sbt would write to `active.json`.
    #[must_use]
    pub fn uri(&self) -> String {
        match self {
            Self::Tcp(addr) => format!("tcp://{addr}"),
            Self::Local(path) => format!("local://{}", path.display()),
        }
    }
}

/// One message the fake received, as sent.
#[derive(Debug, Clone)]
pub struct Recorded {
    pub received_at: Instant,
    pub id: Option<Value>,
    pub method: Option<String>,
    pub params: Value,
    pub result: Option<Value>,
    pub error: Option<Value>,
}

struct Shared {
    token: Option<String>,
    scenario: Scenario,
    print_on_exec: bool,
    recorded: Mutex<Vec<Recorded>>,
    active_json: PathBuf,
    token_file: Option<PathBuf>,
    socket_path: Option<PathBuf>,
    finished: watch::Sender<bool>,
    connections: Mutex<Vec<JoinHandle<()>>>,
}

impl Shared {
    fn record(&self, message: &Value) {
        let entry = Recorded {
            received_at: Instant::now(),
            id: message.get("id").cloned(),
            method: message
                .get("method")
                .and_then(Value::as_str)
                .map(str::to_owned),
            params: message.get("params").cloned().unwrap_or(Value::Null),
            result: message.get("result").cloned(),
            error: message.get("error").cloned(),
        };
        self.recorded
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(entry);
    }

    /// What sbt's exit hook does: remove the descriptor, then go away.
    async fn finish(&self) {
        for path in [
            Some(&self.active_json),
            self.token_file.as_ref(),
            self.socket_path.as_ref(),
        ]
        .into_iter()
        .flatten()
        {
            drop(tokio::fs::remove_file(path).await);
        }
        self.finished.send_replace(true);
    }

    fn abort_connections(&self) {
        for handle in self
            .connections
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .drain(..)
        {
            handle.abort();
        }
    }
}

pub struct FakeSbtServer {
    endpoint: Endpoint,
    shared: Arc<Shared>,
    accept: JoinHandle<()>,
}

impl FakeSbtServer {
    /// Bind, write `project/target/active.json` (and a token file when a
    /// token is configured) under `build_root`, and start serving.
    ///
    /// # Errors
    ///
    /// The listener could not bind or the descriptor could not be written.
    pub async fn start(build_root: &Path, options: Options) -> io::Result<Self> {
        let target = build_root.join("project").join("target");
        tokio::fs::create_dir_all(&target).await?;
        let (listener, endpoint) = bind(&options.transport).await?;
        let active_json = target.join("active.json");
        let token_file = options.token.as_ref().map(|_| target.join("token.json"));
        let descriptor = match (&options.token, &token_file) {
            (Some(token), Some(path)) => {
                let token_json = json!({ "uri": endpoint.uri(), "token": token });
                tokio::fs::write(path, token_json.to_string()).await?;
                json!({
                    "uri": endpoint.uri(),
                    "tokenfilePath": path,
                    "tokenfileUri": format!("file://{}", path.display()),
                })
            }
            _ => json!({ "uri": endpoint.uri() }),
        };
        tokio::fs::write(&active_json, descriptor.to_string()).await?;
        let (finished, _) = watch::channel(false);
        let shared = Arc::new(Shared {
            token: options.token,
            scenario: options.scenario,
            print_on_exec: options.print_on_exec,
            recorded: Mutex::new(Vec::new()),
            active_json,
            token_file,
            socket_path: match &endpoint {
                Endpoint::Local(path) => Some(path.clone()),
                Endpoint::Tcp(_) => None,
            },
            finished,
            connections: Mutex::new(Vec::new()),
        });
        let accept = tokio::spawn(accept_loop(listener, Arc::clone(&shared)));
        Ok(Self {
            endpoint,
            shared,
            accept,
        })
    }

    #[must_use]
    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    #[must_use]
    pub fn active_json(&self) -> &Path {
        &self.shared.active_json
    }

    /// Every message received so far, in arrival order.
    #[must_use]
    pub fn requests(&self) -> Vec<Recorded> {
        self.shared
            .recorded
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Only the `sbt/exec` requests.
    #[must_use]
    pub fn exec_requests(&self) -> Vec<Recorded> {
        self.requests()
            .into_iter()
            .filter(|r| r.method.as_deref() == Some("sbt/exec"))
            .collect()
    }

    /// Resolves once a cooperative or delayed shutdown has run its exit hook.
    pub fn finished(&self) -> impl Future<Output = ()> + Send + 'static {
        let mut rx = self.shared.finished.subscribe();
        async move {
            drop(rx.wait_for(|done| *done).await);
        }
    }

    #[must_use]
    pub fn is_finished(&self) -> bool {
        *self.shared.finished.borrow()
    }

    /// Close every connection and stop listening, like the JVM exiting.
    pub async fn stop(self) {
        self.accept.abort();
        self.shared.abort_connections();
        self.shared.finish().await;
    }
}

impl Drop for FakeSbtServer {
    fn drop(&mut self) {
        self.accept.abort();
        self.shared.abort_connections();
    }
}

/// Path of the `fake-sbt` binary next to the running test binary's target
/// directory.
///
/// # Errors
///
/// The binary has not been built; `cargo build -p fake-sbt` (or
/// `cargo test --workspace`) builds it.
pub fn bin_path() -> Result<PathBuf, BinNotBuilt> {
    let exe = std::env::current_exe().map_err(|_| BinNotBuilt {
        searched_from: PathBuf::from("<unknown current_exe>"),
    })?;
    exe.ancestors()
        .skip(1)
        .map(|dir| dir.join("fake-sbt"))
        .find(|candidate| candidate.is_file())
        .ok_or(BinNotBuilt { searched_from: exe })
}

#[derive(Debug)]
pub struct BinNotBuilt {
    searched_from: PathBuf,
}

impl fmt::Display for BinNotBuilt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "no fake-sbt binary in any ancestor of {}; run `cargo build -p fake-sbt` first",
            self.searched_from.display()
        )
    }
}

impl std::error::Error for BinNotBuilt {}

enum Listener {
    Tcp(TcpListener),
    Unix(UnixListener),
}

async fn bind(transport: &Transport) -> io::Result<(Listener, Endpoint)> {
    match transport {
        Transport::Tcp => {
            let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
            let addr = listener.local_addr()?;
            Ok((Listener::Tcp(listener), Endpoint::Tcp(addr)))
        }
        Transport::Local(path) => {
            drop(std::fs::remove_file(path));
            let listener = UnixListener::bind(path)?;
            Ok((Listener::Unix(listener), Endpoint::Local(path.clone())))
        }
    }
}

async fn accept_loop(listener: Listener, shared: Arc<Shared>) {
    let mut finished = shared.finished.subscribe();
    loop {
        tokio::select! {
            accepted = accept_one(&listener, &shared) => {
                if accepted.is_err() {
                    break;
                }
            }
            _ = finished.wait_for(|done| *done) => break,
        }
    }
}

async fn accept_one(listener: &Listener, shared: &Arc<Shared>) -> io::Result<()> {
    match listener {
        Listener::Tcp(listener) => {
            let (stream, _) = listener.accept().await?;
            spawn_serve(stream, shared);
        }
        Listener::Unix(listener) => {
            let (stream, _) = listener.accept().await?;
            spawn_serve(stream, shared);
        }
    }
    Ok(())
}

fn spawn_serve<S>(stream: S, shared: &Arc<Shared>)
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let for_task = Arc::clone(shared);
    let handle = tokio::spawn(async move {
        if let Err(error) = serve(stream, for_task).await {
            eprintln!("fake-sbt: connection ended: {error}");
        }
    });
    shared
        .connections
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(handle);
}

async fn serve<S>(mut io: S, shared: Arc<Shared>) -> io::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut authenticated = shared.token.is_none();
    let mut buf = Vec::new();
    while let Some(body) = read_frame(&mut io, &mut buf).await? {
        let Ok(message) = serde_json::from_slice::<Value>(&body) else {
            continue;
        };
        shared.record(&message);
        let method = message.get("method").and_then(Value::as_str);
        let id = message.get("id").cloned();
        let (Some(method), Some(id)) = (method, id) else {
            continue;
        };
        match method {
            "initialize" => {
                authenticated = handle_initialize(&mut io, &shared, &message, &id).await?;
            }
            "sbt/exec" if !authenticated => {
                let text = "'sbt/exec' is not allowed before authentication.";
                write_error(&mut io, &id, -32600, text).await?;
            }
            "sbt/exec" => {
                let command = message
                    .pointer("/params/commandLine")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if command == "shutdown" {
                    if handle_shutdown(&mut io, &shared, &id).await? {
                        return Ok(());
                    }
                } else {
                    handle_exec(&mut io, &shared, command, &id).await?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}

async fn handle_initialize<S: AsyncWrite + Unpin>(
    io: &mut S,
    shared: &Shared,
    message: &Value,
    id: &Value,
) -> io::Result<bool> {
    if let Some(expected) = &shared.token {
        let offered = message
            .pointer("/params/initializationOptions/token")
            .and_then(Value::as_str);
        if offered != Some(expected.as_str()) {
            write_error(io, id, -32600, "invalid token").await?;
            return Ok(false);
        }
    }
    play(io, INITIALIZE, id).await?;
    Ok(true)
}

async fn handle_exec<S: AsyncWrite + Unpin>(
    io: &mut S,
    shared: &Shared,
    command: &str,
    id: &Value,
) -> io::Result<()> {
    if shared.print_on_exec {
        println!("fake-sbt: exec {command}");
    }
    tokio::time::sleep(shared.scenario.exec_delay).await;
    match shared.scenario.exec {
        ExecScript::Happy => play(io, HAPPY, id).await,
        ExecScript::FailsWith(code) => {
            play(io, &FAILS.replace("{{CODE}}", &code.to_string()), id).await
        }
        ExecScript::Mute => Ok(()),
    }
}

/// Returns whether the connection should close.
async fn handle_shutdown<S: AsyncWrite + Unpin>(
    io: &mut S,
    shared: &Arc<Shared>,
    id: &Value,
) -> io::Result<bool> {
    match shared.scenario.shutdown {
        ShutdownBehaviour::Cooperative => {
            play(io, SHUTDOWN, id).await?;
            shared.finish().await;
            Ok(true)
        }
        ShutdownBehaviour::DelayedExit(delay) => {
            play(io, SHUTDOWN, id).await?;
            let shared = Arc::clone(shared);
            tokio::spawn(async move {
                tokio::time::sleep(delay).await;
                shared.finish().await;
            });
            Ok(true)
        }
        ShutdownBehaviour::Mute => Ok(false),
    }
}

async fn play<S: AsyncWrite + Unpin>(io: &mut S, fixture: &str, id: &Value) -> io::Result<()> {
    let id = id_text(id);
    for line in fixture
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
    {
        write_frame(io, &line.replace("{{ID}}", &id)).await?;
    }
    Ok(())
}

async fn write_error<S: AsyncWrite + Unpin>(
    io: &mut S,
    id: &Value,
    code: i64,
    message: &str,
) -> io::Result<()> {
    let body = json!({
        "jsonrpc": "2.0",
        "id": id_text(id),
        "error": { "code": code, "message": message },
    });
    write_frame(io, &body.to_string()).await
}

/// sbt echoes ids as strings whatever the client sent.
fn id_text(id: &Value) -> String {
    match id {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// sbt-style: both headers, CRLF separators, and a stray newline after the
/// body that the client must skip.
async fn write_frame<S: AsyncWrite + Unpin>(io: &mut S, body: &str) -> io::Result<()> {
    let frame = format!(
        "Content-Length: {}\r\nContent-Type: application/vscode-jsonrpc; charset=utf-8\r\n\r\n{body}\n",
        body.len()
    );
    io.write_all(frame.as_bytes()).await
}

async fn read_frame<S: AsyncRead + Unpin>(
    io: &mut S,
    buf: &mut Vec<u8>,
) -> io::Result<Option<Vec<u8>>> {
    let mut chunk = [0u8; 4096];
    loop {
        if let Some(frame) = take_frame(buf) {
            return Ok(Some(frame));
        }
        let n = io.read(&mut chunk).await?;
        if n == 0 {
            return Ok(None);
        }
        buf.extend_from_slice(&chunk[..n]);
    }
}

fn take_frame(buf: &mut Vec<u8>) -> Option<Vec<u8>> {
    let header_end = buf.windows(4).position(|w| w == b"\r\n\r\n")?;
    let headers = String::from_utf8_lossy(&buf[..header_end]).into_owned();
    let length: usize = headers
        .lines()
        .find_map(|line| line.strip_prefix("Content-Length:"))
        .and_then(|value| value.trim().parse().ok())?;
    let body_start = header_end + 4;
    let body_end = body_start + length;
    if buf.len() < body_end {
        return None;
    }
    let body = buf[body_start..body_end].to_vec();
    buf.drain(..body_end);
    Some(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scenario_text_round_trips() {
        let scenario: Scenario = "exec=fails:3; delay=250; shutdown=delayed:500"
            .parse()
            .unwrap();
        assert_eq!(scenario.exec, ExecScript::FailsWith(3));
        assert_eq!(scenario.exec_delay, Duration::from_millis(250));
        assert_eq!(
            scenario.shutdown,
            ShutdownBehaviour::DelayedExit(Duration::from_millis(500))
        );
        assert_eq!(scenario.to_string().parse::<Scenario>().unwrap(), scenario);
        assert_eq!("".parse::<Scenario>().unwrap(), Scenario::default());
    }

    #[test]
    fn bad_scenario_text_is_rejected() {
        assert!("exec=nope".parse::<Scenario>().is_err());
        assert!("shutdown=delayed:soon".parse::<Scenario>().is_err());
        assert!("exec".parse::<Scenario>().is_err());
        assert!("colour=red".parse::<Scenario>().is_err());
    }

    #[test]
    fn frames_are_cut_at_content_length_and_ids_echo_as_strings() {
        let mut buf = b"Content-Length: 2\r\n\r\n{}Content-Length: 1".to_vec();
        assert_eq!(take_frame(&mut buf), Some(b"{}".to_vec()));
        assert_eq!(take_frame(&mut buf), None);
        assert_eq!(id_text(&json!(7)), "7");
        assert_eq!(id_text(&json!("7")), "7");
    }
}
