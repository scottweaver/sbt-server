//! The output router: one task per instance that owns the instance's phase
//! and turns everything that arrives on the instance's single FIFO channel
//! (RPC notifications, child pipe bytes, exec control, child death) into
//! [`ExecEvent`]s for whichever exec is running.
//!
//! Because notifications, child output and exec control share one channel,
//! every RPC-derived event of an exec is delivered before that exec's
//! terminal event. The JVM's pipes carry no such guarantee.

use std::collections::VecDeque;
use std::process::ExitStatus;
use std::time::{Duration, Instant};

use tokio::sync::{mpsc, oneshot, watch};

use super::Pid;
use super::completion;
use super::events::{self, SbtStream, Translated};
use super::lines::LineAssembler;
use super::session::{CommandLine, ExecOutcome};
use crate::jsonrpc::Notification;
use crate::protocol::{AbortReason, ExecEvent, LogLevel, OutputStream};
use crate::routing::BuildRoot;

const BOOT_RING_LINES: usize = 200;

/// Where an instance is in its life, as published to observers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Phase {
    Booting,
    Idle {
        since: Instant,
    },
    Running {
        command: CommandLine,
        since: Instant,
    },
    Dead(DeathCause),
}

impl Phase {
    #[must_use]
    pub fn is_booting(&self) -> bool {
        matches!(self, Self::Booting)
    }

    #[must_use]
    pub fn is_dead(&self) -> bool {
        match self {
            Self::Dead(_) => true,
            Self::Booting | Self::Idle { .. } | Self::Running { .. } => false,
        }
    }

    #[must_use]
    pub fn death_cause(&self) -> Option<&DeathCause> {
        match self {
            Self::Dead(cause) => Some(cause),
            Self::Booting | Self::Idle { .. } | Self::Running { .. } => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeathCause {
    Exited {
        code: i32,
    },
    Signalled,
    /// `wait` itself failed; the process is gone but its status is unknown.
    Unknown,
}

impl DeathCause {
    fn from_status(status: Option<ExitStatus>) -> Self {
        match status.map(|s| s.code()) {
            Some(Some(code)) => Self::Exited { code },
            Some(None) => Self::Signalled,
            None => Self::Unknown,
        }
    }
}

impl std::fmt::Display for DeathCause {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Exited { code } => write!(f, "exited with status {code}"),
            Self::Signalled => f.write_str("killed by a signal"),
            Self::Unknown => f.write_str("exited with an unknown status"),
        }
    }
}

/// How an exec ended, as reported by the task that ran it.
#[derive(Debug)]
pub enum ExecEnd {
    Answered {
        outcome: ExecOutcome,
        elapsed: Duration,
    },
    ConnectionClosed,
}

/// Everything the router consumes.
#[derive(Debug)]
pub enum Input {
    Notification(Notification),
    Stdout(Vec<u8>),
    Stderr(Vec<u8>),
    ChildExited(Option<ExitStatus>),
    /// The handshake completed; boot is over.
    Booted,
    /// Hand back the boot output ring buffer.
    BootOutput(oneshot::Sender<Vec<String>>),
    BeginExec {
        command: CommandLine,
        verbose: bool,
        sink: mpsc::Sender<ExecEvent>,
    },
    EndExec(ExecEnd),
    /// A shutdown is in progress; a lost connection is expected.
    ShutdownRequested,
}

impl From<Notification> for Input {
    fn from(notification: Notification) -> Self {
        Self::Notification(notification)
    }
}

struct ActiveExec {
    command: CommandLine,
    verbose: bool,
    sink: mpsc::Sender<ExecEvent>,
}

/// The router's own state; the sink exists only while an exec is open.
enum Mode {
    Booting,
    Idle {
        since: Instant,
    },
    Running(ActiveExec),
    Dead {
        cause: DeathCause,
        exec: Option<ActiveExec>,
    },
}

impl Mode {
    fn phase(&self, running_since: Instant) -> Phase {
        match self {
            Self::Booting => Phase::Booting,
            Self::Idle { since } => Phase::Idle { since: *since },
            Self::Running(exec) => Phase::Running {
                command: exec.command.clone(),
                since: running_since,
            },
            Self::Dead { cause, .. } => Phase::Dead(cause.clone()),
        }
    }
}

pub(super) struct Router {
    root: BuildRoot,
    pid: Pid,
    inbox: mpsc::Receiver<Input>,
    phase: watch::Sender<Phase>,
    mode: Mode,
    running_since: Instant,
    boot_ring: VecDeque<String>,
    jvm_out: LineAssembler,
    jvm_err: LineAssembler,
    sbt_out: LineAssembler,
    sbt_err: LineAssembler,
    execs_completed: u64,
    shutdown_requested: bool,
}

impl Router {
    pub(super) fn new(
        root: BuildRoot,
        pid: Pid,
        inbox: mpsc::Receiver<Input>,
        phase: watch::Sender<Phase>,
    ) -> Self {
        Self {
            root,
            pid,
            inbox,
            phase,
            mode: Mode::Booting,
            running_since: Instant::now(),
            boot_ring: VecDeque::with_capacity(BOOT_RING_LINES),
            jvm_out: LineAssembler::new(),
            jvm_err: LineAssembler::new(),
            sbt_out: LineAssembler::new(),
            sbt_err: LineAssembler::new(),
            execs_completed: 0,
            shutdown_requested: false,
        }
    }

    pub(super) async fn run(mut self) {
        while let Some(input) = self.inbox.recv().await {
            match input {
                Input::Notification(notification) => self.on_notification(&notification).await,
                Input::Stdout(bytes) => {
                    let lines = self.jvm_out.push(&bytes);
                    self.emit_lines(OutputStream::Stdout, lines).await;
                }
                Input::Stderr(bytes) => {
                    let lines = self.jvm_err.push(&bytes);
                    self.emit_lines(OutputStream::Stderr, lines).await;
                }
                Input::ChildExited(status) => self.on_child_exited(status),
                Input::Booted => self.on_booted(),
                Input::BootOutput(reply) => {
                    drop(reply.send(self.boot_ring.iter().cloned().collect()));
                }
                Input::BeginExec {
                    command,
                    verbose,
                    sink,
                } => self.on_begin(command, verbose, sink).await,
                Input::EndExec(end) => self.on_end(end).await,
                Input::ShutdownRequested => self.shutdown_requested = true,
            }
        }
        tracing::debug!(root = %self.root, "output router stopped");
    }

    async fn on_notification(&mut self, notification: &Notification) {
        match events::translate(notification) {
            Translated::Log {
                level: LogLevel::Debug,
                text,
            } => {
                self.emit(ExecEvent::Log {
                    level: LogLevel::Debug,
                    text,
                })
                .await;
            }
            Translated::Log { level, text } => {
                tracing::trace!(?level, %text, "logMessage dropped; the terminal stream carries it");
            }
            Translated::Bytes { stream, bytes } => {
                let lines = self.sbt_assembler(stream).push(&bytes);
                self.emit_lines(output_stream(stream), lines).await;
            }
            Translated::Flush { stream } => {
                if let Some(line) = self.sbt_assembler(stream).flush() {
                    self.emit_lines(output_stream(stream), vec![line]).await;
                }
            }
            Translated::Diagnostics { file, items } => {
                self.emit(ExecEvent::Diagnostics { file, items }).await;
            }
            Translated::ServerShutdown => {
                tracing::info!(root = %self.root, "sbt announced its shutdown");
            }
            Translated::Ignored => {}
        }
    }

    fn sbt_assembler(&mut self, stream: SbtStream) -> &mut LineAssembler {
        match stream {
            SbtStream::Out => &mut self.sbt_out,
            SbtStream::Err => &mut self.sbt_err,
        }
    }

    fn on_booted(&mut self) {
        if matches!(self.mode, Mode::Booting) {
            self.mode = Mode::Idle {
                since: Instant::now(),
            };
            self.publish();
        }
    }

    fn on_child_exited(&mut self, status: Option<ExitStatus>) {
        let cause = DeathCause::from_status(status);
        tracing::info!(root = %self.root, pid = self.pid.get(), %cause, "sbt process ended");
        self.mode = match std::mem::replace(&mut self.mode, Mode::Booting) {
            Mode::Running(exec) => Mode::Dead {
                cause,
                exec: Some(exec),
            },
            Mode::Dead { cause: first, exec } => Mode::Dead { cause: first, exec },
            Mode::Booting | Mode::Idle { .. } => Mode::Dead { cause, exec: None },
        };
        self.publish();
    }

    async fn on_begin(
        &mut self,
        command: CommandLine,
        verbose: bool,
        sink: mpsc::Sender<ExecEvent>,
    ) {
        let exec = ActiveExec {
            command,
            verbose,
            sink,
        };
        self.running_since = Instant::now();
        self.mode = match std::mem::replace(&mut self.mode, Mode::Booting) {
            Mode::Booting | Mode::Idle { .. } => Mode::Running(exec),
            Mode::Running(previous) => {
                tracing::error!(root = %self.root, "exec began while another was open");
                drop(previous);
                Mode::Running(exec)
            }
            Mode::Dead {
                cause,
                exec: previous,
            } => {
                drop(previous);
                Mode::Dead {
                    cause,
                    exec: Some(exec),
                }
            }
        };
        self.publish();
        let started = ExecEvent::Started {
            build_root: self.root.as_path().to_path_buf(),
            pid: self.pid.get(),
            cold_start: self.execs_completed == 0,
        };
        self.emit(started).await;
    }

    async fn on_end(&mut self, end: ExecEnd) {
        let (exec, next) = match std::mem::replace(&mut self.mode, Mode::Booting) {
            Mode::Running(exec) => (
                exec,
                Mode::Idle {
                    since: Instant::now(),
                },
            ),
            Mode::Dead {
                cause,
                exec: Some(exec),
            } => (exec, Mode::Dead { cause, exec: None }),
            other => {
                tracing::error!(root = %self.root, "exec ended but none was open");
                self.mode = other;
                return;
            }
        };
        for (stream, assembler) in [
            (OutputStream::Stdout, &mut self.jvm_out),
            (OutputStream::Stderr, &mut self.jvm_err),
            (OutputStream::SbtStdout, &mut self.sbt_out),
            (OutputStream::SbtStderr, &mut self.sbt_err),
        ] {
            if let Some(text) = assembler.flush() {
                drop(exec.sink.send(ExecEvent::Output { stream, text }).await);
            }
        }
        let terminal = match end {
            ExecEnd::Answered { outcome, elapsed } => ExecEvent::done(
                outcome.exit_code(),
                u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX),
                completion::render(&exec.command, &outcome, elapsed),
            ),
            ExecEnd::ConnectionClosed => {
                let reason = if self.shutdown_requested {
                    AbortReason::DaemonShutdown
                } else if matches!(next, Mode::Dead { .. }) {
                    AbortReason::SbtDied
                } else {
                    AbortReason::ConnectionLost
                };
                ExecEvent::Aborted {
                    summary: format!("'{}' aborted: {}", exec.command, abort_text(reason)),
                    reason,
                }
            }
        };
        drop(exec.sink.send(terminal).await);
        self.execs_completed += 1;
        self.mode = next;
        self.publish();
    }

    /// The attached terminal stream carries sbt's console rendering, so a
    /// line with sbt's own `[level] ` prefix is a log line; everything else
    /// on it (a test framework's output, say) is plain output.
    async fn emit_lines(&mut self, stream: OutputStream, lines: Vec<String>) {
        for text in lines {
            let event = match stream {
                OutputStream::SbtStdout => match events::log_line(&text) {
                    Some((level, rest)) => ExecEvent::Log {
                        level,
                        text: rest.to_owned(),
                    },
                    None => ExecEvent::Output { stream, text },
                },
                OutputStream::Stdout | OutputStream::Stderr | OutputStream::SbtStderr => {
                    ExecEvent::Output { stream, text }
                }
            };
            self.emit(event).await;
        }
    }

    async fn emit(&mut self, event: ExecEvent) {
        match &self.mode {
            Mode::Booting => {
                if let Some(text) = boot_text(&event) {
                    if self.boot_ring.len() == BOOT_RING_LINES {
                        self.boot_ring.pop_front();
                    }
                    self.boot_ring.push_back(text);
                }
            }
            Mode::Idle { .. } | Mode::Dead { exec: None, .. } => {
                tracing::debug!(root = %self.root, ?event, "output outside any exec");
            }
            Mode::Running(exec)
            | Mode::Dead {
                exec: Some(exec), ..
            } => {
                if !exec.verbose && is_debug_log(&event) {
                    return;
                }
                drop(exec.sink.send(event).await);
            }
        }
    }

    fn publish(&self) {
        self.phase.send_replace(self.mode.phase(self.running_since));
    }
}

fn output_stream(stream: SbtStream) -> OutputStream {
    match stream {
        SbtStream::Out => OutputStream::SbtStdout,
        SbtStream::Err => OutputStream::SbtStderr,
    }
}

fn boot_text(event: &ExecEvent) -> Option<String> {
    match event {
        ExecEvent::Log { level, text } if *level != LogLevel::Debug => Some(text.clone()),
        ExecEvent::Output { text, .. } => Some(text.clone()),
        ExecEvent::Log { .. }
        | ExecEvent::Started { .. }
        | ExecEvent::Diagnostics { .. }
        | ExecEvent::Done { .. }
        | ExecEvent::Aborted { .. } => None,
    }
}

fn is_debug_log(event: &ExecEvent) -> bool {
    matches!(
        event,
        ExecEvent::Log {
            level: LogLevel::Debug,
            ..
        }
    )
}

fn abort_text(reason: AbortReason) -> &'static str {
    match reason {
        AbortReason::SbtDied => "the sbt process died",
        AbortReason::ConnectionLost => "the connection to sbt was lost",
        AbortReason::DaemonShutdown => "the daemon is shutting down",
    }
}
