# Architecture constraints

Decided, binding architecture facts for sbt-server. STATE.md answers "what
is in flight"; this file answers "what must remain true." Every constraint
here is either a hard external fact (sbt's own protocol behaviour, confirmed
in its source or by live testing) or a design decision made during the first
build — a change to any of them is a design decision requiring a PR that
updates this file in the same change (see METHODOLOGIES.md "Refactors that
change documented architecture").

Established 2026-10-29 (converted from `HANDOFF.md`, the verbatim
development hand-off kept at the project root as the frozen historical
source; no separate design dialog — every constraint here was decided
during the first build and confirmed by its live testing). Gotcha numbers in
parentheses refer to that document's "Gotchas found only by live testing"
section.

## Purpose

- **`sbtd` is a local daemon that keeps one warm sbt server process per
  project and routes commands to it.** `sbtctl` is the agent-facing CLI
  that talks to the daemon. The goal is to avoid sbt's slow cold start
  (JVM boot + build-definition load) on every agent-driven `compile` /
  `test` by keeping sbt warm across calls, while routing each call to the
  right project automatically.

## Agent transport (hard constraint)

- **Agents in this environment may not use custom MCP servers and may not
  use Unix sockets.** This single constraint shaped the whole design.
- **`sbtctl` talks to `sbtd` only over loopback HTTP** (`127.0.0.1:8077`
  by default). `sbtctl` is what agents actually invoke, via their shell
  tool.
- **`sbtd` drives each warm sbt instance via sbt's own JSON-RPC server
  protocol**, which may itself be a Unix socket. That is fine because it is
  strictly `sbtd`↔`sbt`, never exposed to the agent.
- **Invariant any redesign must preserve: agents only ever speak loopback
  HTTP to `sbtd`.**
- **No auth on `sbtd`'s HTTP API beyond binding strictly to loopback.**
  Accepted for a localhost dev tool; not revisited since.

## Workspace layout

Rust workspace, three members:

```
Cargo.toml                      workspace (3 shipped members + a test double)
crates/
  core/   sbt-server-core (lib, no binary)
    src/instance.rs    owns one warm sbt process + its JSON-RPC connection
    src/jsonrpc.rs     transport-generic JSON-RPC 2.0 client (TCP or Unix socket)
    src/jsonrpc/       framing.rs (LSP framing codec) and message.rs (message
                       shapes): pure, unit-tested without IO
    src/registry.rs    build-root -> warm Instance map, lazy-start, shutdown
    src/routing.rs     cwd -> build root resolution (walks up for build.sbt /
                       project/build.properties)
    src/protocol.rs    HTTP request/response DTOs shared by daemon+cli
    src/paths.rs       shared state dir (~/.config/sbt-server)
  daemon/ sbtd (bin)   hand-rolled hyper 1.x HTTP server wrapping Registry
  cli/    sbtctl (bin) agent-facing client; clap + reqwest
  fake-sbt/            dev-only scripted stand-in for sbt's server (lib + bin,
                       publish = false, never shipped); plays frozen literal
                       frames from fixtures/*.jsonl so tests need no JVM
```

- **Language/runtime: Rust on tokio async**, chosen for a single static
  binary and strong async process supervision.
- **HTTP server is hand-rolled on `hyper` 1.x + `hyper-util` +
  `http-body-util`, not `axum`.** `axum` was absent from the offline cargo
  cache when this was built; swap back to `axum` if that is ever not a
  constraint. Build/test is `cargo build` / `cargo test`, offline-capable.
- **`protocol.rs` DTOs are the agent-facing contract** shared by daemon and
  CLI.

## sbt driver protocol

- **The sbt driver is a hand-rolled client for sbt's own JSON-RPC server
  protocol — this is not real LSP or BSP.** It reuses LSP's framing
  (`Content-Length`-prefixed JSON-RPC) and borrows LSP's `initialize`
  handshake shape, but the actual commands (`sbt/exec`, `sbt/shutdown`,
  `sbt/systemOut`) are sbt's own proprietary protocol — the same one sbt's
  thin client (`sbtn`) uses. BSP (Build Server Protocol) is a separate real
  standard sbt also implements (for Metals/IntelliJ); this project does not
  use it at all.
- **The JSON-RPC client is transport-generic** (gotcha 1): a boxed
  `dyn AsyncRead + AsyncWrite` trait object with `connect_tcp` and
  `connect_unix` constructors. `instance.rs::parse_server_uri` handles
  both `tcp://host:port` and `local:///path` URIs from `active.json`.
  sbt's source (`CommandExchange.scala`) codes `ConnectionType.Tcp` as the
  default, but on the original development machine sbt actually picked a
  Unix domain socket.
- **Token auth is optional, not mandatory** (gotcha 2). Over the default
  Local transport sbt treats the connecting process as already
  authenticated via Unix-socket filesystem permissions and omits
  `tokenfilePath` from `active.json`. `read_connection_info` and the
  `initialize` handshake treat the token as `Option<String>` and only
  read/send one when `tokenfilePath` is present.

## Instance lifecycle

- **Never trust a pre-existing `active.json`** (gotcha 3). That file can
  describe *any* live server for the build root — a manual `sbt` shell, an
  editor's background instance, a stale crashed server — and trusting it
  silently hands every request to a foreign process with no stdio access
  and no lifecycle control. `Instance::start` therefore *always* takes
  over: if `active.json` exists, connect to whatever is there, send a
  best-effort bounded `sbt/shutdown`, delete the descriptor regardless of
  whether that worked, then always spawn a fresh `sbt` process. One-time
  cold-boot cost per daemon restart per build root — correctness over
  speed.
- **`sbt/shutdown` is bounded** (gotcha 4). sbt's server dispatches through
  a handler chain with no catch-all; an unrecognized request is silently
  dropped with no response and no error. The shutdown request is wrapped
  in `tokio::time::timeout` (5s), then the child is reaped/killed
  regardless of whether the RPC succeeded, timed out, or errored.
- **Routing: cwd → build root by walking upward for `build.sbt` or
  `project/build.properties`.** Instances lazy-start on first use and
  stay warm until an explicit shutdown or daemon restart; there is no
  idle-timeout/auto-reap.
- **Per-instance serialization.** sbt has one global build state per
  process, so concurrent `exec` calls against the *same* instance are
  serialized via a `Mutex<()>` guard; different build roots run fully
  concurrently.
- **The daemon-action path is `async fn` end to end** (gotcha 7). Never
  spin up a second Tokio runtime with `Runtime::new().block_on(...)` from
  code already running under `main`'s runtime; it panics with "Cannot
  start a runtime from within a runtime".

## Output and completion semantics

- **`/exec` streams NDJSON over HTTP, one line per log event**, so output
  appears live instead of buffering until the whole command finishes.
- **Two output channels are captured and both are forwarded to the
  caller** (gotcha 5). sbt's JSON-RPC notifications (`build/logMessage`,
  `sbt/systemOut/Err`) only cover sbt's *own* structured logger; a
  subprocess invoked by a build plugin (like `gcloud`) writes straight to
  the JVM's stdout/stderr and never becomes a notification. `spawn_sbt`
  pipes the child's real stdout/stderr into a `broadcast` channel
  (`raw_output`), and `Instance::exec` subscribes to it alongside the
  notification stream.
- **Project-loading output cannot be attributed to a command.** Anything
  that fires during project loading (the `gcloud` credential-refresh
  warning, for instance) happens before the JSON-RPC handshake completes
  and before any `exec()` subscriber exists. It is a one-time-per-JVM-boot
  event, not a capture bug.
- **sbt never sends its `[success] Total time: ...` banner to a client**
  (gotcha 6, confirmed in `Aggregation.scala`: `handedOffToClient` is true
  for JSON-RPC clients and `printSuccess` is skipped). `Instance::exec`
  synthesizes an equivalent completion line itself — e.g.
  `[success] 'compile' completed in 1.2s` or
  `[error] 'compile' failed (exit code 1) in 0.8s` — appended after any
  real completion message sbt's RPC response did include.
- **Bare status pings are dropped** (gotcha 8). `notification_to_exec_event`
  only forwards `sbt/exec` notifications carrying a non-empty `message`;
  `ExecStatusEvent` pings with no message are discarded so a no-op
  `compile` no longer prints two content-free "Processing sbt/exec" lines.
- **Real exit codes propagate end to end.** sbt's actual result becomes
  `sbtctl`'s process exit code so agents can branch on success/failure.

## CLI surface

- **Any sbt command line passes through**: `sbtctl compile`, `sbtctl test`,
  `sbtctl testOnly com.ex.FooSpec`, or any other raw command via clap's
  external-subcommand. `-C <dir>` overrides cwd instead of using `$PWD`.
- **`sbtctl status`** lists warm instances: build root, pid, busy, idle
  time. **`sbtctl shutdown` / `sbtctl shutdown --all`** stop one or all
  warm instances (bounded timeout, see Instance lifecycle).
- **`sbtctl daemon start|stop|restart|status`** manage `sbtd` itself; all
  three transition verbs poll the real `/health` endpoint to confirm the
  transition actually happened rather than firing-and-forgetting a signal.
- **`sbtctl daemon stop` uses `kill <pid>`**, which assumes a standard POSIX
  environment; untested on Windows.

## Verified scope and open items

- **Only sbt 1.x server-protocol behaviour has been exercised** (project
  pinned at 1.11.7). The system's `sbt` launcher is 2.0.9, but the launcher
  downloads/runs whatever `project/build.properties` pins per project.
- **No automated test coverage for `instance.rs`'s sbt-facing logic**
  (handshake, exec, shutdown); only `routing.rs`'s pure cwd-resolution
  logic has unit tests. Everything else was verified by live manual
  testing against real sbt projects, not captured as repeatable tests.
- **No idle-timeout/auto-reap** for warm instances (mentioned as a future
  possibility, not implemented).
- **No auth** on the HTTP API beyond loopback binding.
- **Windows untested** (`kill <pid>` in `sbtctl daemon stop`).

## Audit triggers

Files whose changes warrant re-checking this doc during wrap-up:

- `crates/core/src/instance.rs` (takeover-on-start, bounded shutdown,
  output fan-in, synthesized completion line, `parse_server_uri`)
- `crates/core/src/jsonrpc.rs`, `crates/core/src/jsonrpc/**` (transport
  abstraction, framing, `initialize` handshake, token handling)
- `crates/core/src/registry.rs` (instance map, lazy-start, per-instance
  serialization guard)
- `crates/core/src/routing.rs` (build-root resolution)
- `crates/core/src/protocol.rs` (HTTP DTOs — the agent-facing contract)
- `crates/daemon/**` (bind address, HTTP server stack, any auth or
  lifecycle endpoint)
- `crates/cli/**` (subcommand surface, daemon lifecycle verbs, `kill`
  usage, `/health` polling)
- `Cargo.toml` / `crates/*/Cargo.toml` (HTTP stack, async runtime, or any
  new transport dependency)

## Structural criteria

Structural (this doc must change in the same PR): any transport other than
loopback HTTP between `sbtctl` and `sbtd`, or anything that exposes sbt's
socket or JSON-RPC to agents; adding auth to the HTTP API; a change to the
takeover-on-start rule or the bounded-shutdown rule; a change to which
output channels are captured or to the synthesized completion-line format;
adopting a different sbt protocol (BSP, LSP) or framing; a change to
build-root resolution or to the per-instance serialization model; adding
idle-timeout/auto-reap or a new daemon lifecycle verb; swapping the HTTP
stack or the async runtime; dropping sbt 1.x support or adding sbt 2.x
support.

Not structural (no update needed): additional passthrough subcommands that
reuse `/exec` unchanged; log formatting; test changes; DTO field additions
that keep the existing endpoints' meaning.

## Maintenance

Update when a constraint above is deliberately renegotiated (design
dialog + PR updating this file), or when an open item is resolved
(instance-level tests, idle reaping, auth, Windows support, sbt 2.x).
Never for in-flight status — that's STATE.md. Keep constraints
falsifiable and dated. `HANDOFF.md` at the project root is the frozen
2026-10-29 transcript this doc was converted from; do not edit it.
