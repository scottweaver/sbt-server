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
- **`sbtd` cannot be told to bind anything but a loopback address:**
  `--bind` parses into a `LoopbackAddr` newtype (default `127.0.0.1:8077`,
  port 0 allowed), so the invariant above holds by construction rather than
  by configuration.

## Workspace layout

Rust workspace: three library crates, the one package that ships both
binaries, and a test double.

```
Cargo.toml                      workspace (5 members; one is released)
crates/
  core/   sbt-server-core (lib, no binary)
    src/instance.rs    owns one warm sbt process + its JSON-RPC connection
    src/instance/      descriptor.rs, lines.rs, events.rs, completion.rs (pure);
                       session.rs (sbt typestate), output.rs (the router task)
    src/jsonrpc.rs     transport-generic JSON-RPC 2.0 client (TCP or Unix socket)
    src/jsonrpc/       framing.rs (LSP framing codec) and message.rs (message
                       shapes): pure, unit-tested without IO
    src/registry.rs    build-root -> warm Instance map, lazy-start, shutdown
    src/routing.rs     cwd -> build root resolution (walks up for build.sbt /
                       project/build.properties)
    src/protocol.rs    HTTP request/response DTOs shared by daemon+cli
    src/paths.rs       shared state dir (~/.config/sbt-server)
  daemon/ sbt-server-daemon (lib)  axum 0.8 HTTP server wrapping Registry;
                       src/{lib,config,server}.rs; `run()` is the whole daemon
  cli/    sbt-server-cli (lib)     agent-facing client; clap + reqwest;
                       src/{lib,args,client,render,daemon_ctl}.rs; `run()`
  sbt-server/          the one released package: `sbtd` and `sbtctl` as thin
                       mains over the two libraries (src/bin/), plus the
                       integration tests that spawn them (tests/). cargo-dist
                       releases per package and cannot bundle binaries across
                       packages, which is why both binaries live here
  fake-sbt/            dev-only scripted stand-in for sbt's server (lib + bin,
                       publish = false, never shipped); plays frozen literal
                       frames from fixtures/*.jsonl so tests need no JVM
```

- **Language/runtime: Rust on tokio async**, chosen for a single static
  binary and strong async process supervision.
- **HTTP server is `axum` 0.8 on tokio.** The first build hand-rolled it on
  `hyper` 1.x only because `axum` was absent from that machine's offline
  cargo cache, and invited the swap once that was not a constraint; the
  rebuild (2026-10-29) took it. `/exec` streams through
  `Body::from_stream`. Build/test is `cargo build` / `cargo test`.
- **`protocol.rs` DTOs are the agent-facing contract** shared by daemon and
  CLI.

## sbt driver protocol

- **The sbt driver is a hand-rolled client for sbt's own JSON-RPC server
  protocol — this is not real LSP or BSP.** It reuses LSP's framing
  (`Content-Length`-prefixed JSON-RPC) and borrows LSP's `initialize`
  handshake shape, but the actual commands (`sbt/exec`, `sbt/systemOut`,
  and `sbt/exec` of sbt's own `shutdown` command to stop a server) are
  sbt's own proprietary protocol — the same one sbt's
  thin client (`sbtn`) uses. BSP (Build Server Protocol) is a separate real
  standard sbt also implements (for Metals/IntelliJ); this project does not
  use it at all.
- **The JSON-RPC client is transport-generic** (gotcha 1): a boxed
  `dyn AsyncRead + AsyncWrite` object (`jsonrpc::BoxedIo`) opened by
  `connect_tcp` or `connect_unix`. One actor task per connection owns the
  socket and the pending-request map, so a reply slot is registered before
  its request is written and every notification is forwarded before any
  later response resolves. `instance/descriptor.rs` parses both
  `tcp://host:port` and `local:///path` URIs from `active.json`. Request
  ids are sent as JSON strings because sbt stores and echoes them as
  strings. sbt's source (`CommandExchange.scala`) codes `ConnectionType.Tcp`
  as the default, but on the original development machine sbt actually
  picked a Unix domain socket, and so does this one (sbt 1.11.7).
- **The handshake is `initialize` then `sbt/attach` with
  `{"interactive": false}`**, exactly what sbtn does for a batch command.
  Until a client attaches, the channel's terminal is sbt's `NullTerminal`
  and everything written to it is lost: a test framework's per-test lines,
  for one (found live on 2026-10-29: `sbtctl test` showed only the
  `[info] Passed:` summary). Attaching installs a terminal whose output
  arrives as `sbt/systemOut`/`sbt/systemErr`. Right after the attach sbt
  queries the terminal (`sbt/terminalPropertiesQuery`, and on demand
  `sbt/terminalCapabilities`, `sbt/terminalGetSize`, the attribute, echo
  and raw-mode calls); `instance/terminal.rs` answers each as a dumb
  80x24 terminal, because sbt blocks until an answer arrives (five seconds
  for the properties query, unbounded for capabilities). Any other
  server-initiated request is declined with "method not found".
- **Token auth is optional, not mandatory** (gotcha 2). Over the default
  Local transport sbt treats the connecting process as already
  authenticated via Unix-socket filesystem permissions and omits
  `tokenfilePath` from `active.json`. `read_connection_info` and the
  `initialize` handshake treat the token as `Option<String>` and only
  read/send one when `tokenfilePath` is present.

## Instance lifecycle

- **sbt is spawned the way its own thin client spawns a server, minus
  `--detach-stdio`:** `sbt --server -Dsbt.supershell=false
  -Dsbt.color=false -Dsbt.log.noformat=true` with
  `SBT_TERMINAL_PROPS=80,24,false,false,false` in the environment, stdin
  `/dev/null`, stdout and stderr piped. The variable makes sbt treat
  itself as started by a remote client, so it adds no console channel.
  Without it the console shell redraws its prompt after every log line
  and floods the raw stdout pipe with thousands of `ESC[?2004h>....`
  sequences (observed live, with stdin open or closed, with `-Dsbt.ci=true`
  and with JLine's dumb-terminal property alike); `--detach-stdio` cures
  that only by closing the JVM's stdio, which loses the raw output gotcha 5
  exists to capture. Confirmed live on sbt 1.11.7: boot about 2.3 s, no
  escape sequences, subprocess output captured from both pipes.
- **Never trust a pre-existing `active.json`** (gotcha 3). That file can
  describe *any* live server for the build root — a manual `sbt` shell, an
  editor's background instance, a stale crashed server — and trusting it
  silently hands every request to a foreign process with no stdio access
  and no lifecycle control. `Instance::start` therefore *always* takes
  over: if `active.json` exists, connect to whatever is there, send a
  best-effort bounded `shutdown` exec, delete the descriptor regardless of
  whether that worked, then always spawn a fresh `sbt` process. One-time
  cold-boot cost per daemon restart per build root — correctness over
  speed. After the bounded shutdown and the delete, `start` waits (bounded
  by the takeover timeout) until the old endpoint refuses connections
  before spawning: sbt's exit hook deletes `active.json` on the way out
  and would otherwise remove the replacement's descriptor. A server that
  never stops listening is logged and spawned over anyway.
- **Shutdown is `sbt/exec` of sbt's `shutdown` command, bounded**
  (gotcha 4). There is no `sbt/shutdown` method: sbt's handler chain ends
  in a fallback that logs an unrecognized request at debug level and never
  answers it, which is exactly why the hand-off's `sbt/shutdown` hung
  forever (confirmed in sbt 1.13.x `ServerHandler.fallback`; sbtn sends the
  `shutdown` command the same way). The `shutdown` exec is wrapped in
  `tokio::time::timeout` (5s). sbt answers it with a `shutdown`
  notification and closes the socket without a response (captured live),
  so a closed connection counts as cooperation; in every case the process
  is then killed and reaped if it has not exited on its own, regardless of
  whether the RPC succeeded, timed out, or errored. An exec whose
  connection drops ends with an `aborted` event: reason `daemon_shutdown`
  when a shutdown was requested, `sbt_died` when the child is known dead,
  otherwise `connection_lost` — never a fake exit code.
- **Routing: cwd → build root by walking upward for `build.sbt` or
  `project/build.properties`.** Instances lazy-start on first use and
  stay warm until an explicit shutdown or daemon restart; there is no
  idle-timeout/auto-reap.
- **Per-instance serialization.** sbt has one global build state per
  process, so concurrent `exec` calls against the *same* instance are
  serialized via an owned `tokio::sync::Mutex<()>` guard held by the task
  that runs the exec; different build roots run fully concurrently. That
  task runs to sbt's terminal reply even when the caller's output receiver
  is dropped, so a disconnecting HTTP client can never leave sbt busy with
  the lock free.
- **One boot per build root, shared by concurrent callers.** `Registry`
  keeps `BuildRoot → Slot::{Starting(watch), Ready(Arc<Instance>)}`. The
  first caller spawns the boot as a detached task, so a caller that
  disconnects mid-boot can never drop a half-spawned child, and every
  caller arriving during it waits on the same channel and shares the same
  outcome. A failed boot clears the slot so the next call retries. A dead
  instance (`Phase::Dead`) is omitted from `/status` and replaced on the
  next call for its build root.
- **Idle instances are reaped.** A reaper task (every minute) shuts down
  instances idle longer than `--idle-timeout-secs` (default 7200; `0`
  disables) and evicts dead ones; a running instance is never reaped.
  Added in the rebuild; the hand-off had left it open.
- **`sbtd` drains on SIGTERM or SIGINT.** It shuts every instance down
  (bounded) before letting axum stop, so in-flight `/exec` streams end with
  `aborted: daemon_shutdown` instead of hanging. It writes `sbtd.pid` and
  `sbtd.addr` (the address actually bound) into the state dir on start and
  removes them on exit.
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
  pipes the child's real stdout/stderr, as bytes, into the instance's
  single bounded mpsc channel. The same channel carries sbt's JSON-RPC
  notifications and exec control, so every RPC-derived event of an exec
  precedes its terminal event (FIFO); raw pipe lines carry no ordering
  guarantee relative to it. No `broadcast` channel is used anywhere: lag
  would mean silent output loss, so backpressure is applied instead.
  `sbt/systemOut` and `sbt/systemErr` arrive as arrays of signed bytes in
  arbitrary chunks and are reassembled into whole lines
  (`instance/lines.rs`), as are the raw pipes. They flow only to an
  attached client (see the handshake rule under sbt driver protocol).
- **Project-loading output cannot be attributed to a command.** Anything
  that fires during project loading (the `gcloud` credential-refresh
  warning, for instance) happens before the JSON-RPC handshake completes
  and before any `exec()` subscriber exists. It is a one-time-per-JVM-boot
  event, not a capture bug. Boot output is kept in a bounded ring buffer
  and returned in `project_load_failed` when sbt dies before publishing
  its server; the supervisor reports the exit only after both pipes are
  drained (bounded), so a fast-dying sbt's last lines, the ones that say
  why, are never lost to the race between the pipes and `wait` (PR #3).
- **sbt never sends its `[success] Total time: ...` banner to a client**
  (gotcha 6, confirmed in `Aggregation.scala`: `handedOffToClient` is true
  for JSON-RPC clients and `printSuccess` is skipped). `Instance::exec`
  synthesizes an equivalent completion line itself — e.g.
  `[success] 'compile' completed in 1.2s` or
  `[error] 'compile' failed (exit code 1) in 0.8s` — appended after any
  real completion message sbt's RPC response did include.
- **Once attached, the terminal stream is the source of log lines and
  `build/logMessage` is dropped except at debug level** (gotcha 8, and
  what sbtn does). sbt's console appender renders every log event onto the
  channel terminal as `[info] …`/`[warn] …`/`[error] …` and the same event
  also arrives as a `build/logMessage` notification, so forwarding both
  would print everything twice. `sbt_stdout` lines carrying sbt's own
  `[level] ` prefix become `log` events (`instance/events.rs::log_line`),
  other lines on it (a test framework's output) stay `output`, and
  `build/logMessage` is used only for level 4 (log/debug), which the
  appender never renders: the content-free "Processing sbt/exec" lines,
  forwarded only when the request sets `verbose`.
  `build/taskStart|taskProgress|taskFinish` and `sbt/exec` status
  notifications are ignored; completion is the response to the `sbt/exec`
  request.
- **Completion is the response to the `sbt/exec` request.** Success is a
  result (`status: Done`, `exitCode` 0 or absent). A failure arrives as a
  JSON-RPC *error* response: its `code` is the exit code when it is in
  1..=255, and sbt's generic `UnknownError` (-33000, sent for any failed
  command, message empty, captured live) means exit code 1; any other code
  is an internal sbt failure, also reported as exit code 1. The text of a
  command-parse error ("Not a valid command") reaches only sbt's own
  console, never a network client.
- **Compiler diagnostics are a captured channel.** sbt 1.x sends them as
  `build/publishDiagnostics` (BSP-shaped: `textDocument.uri`,
  `diagnostics`, `reset`; captured live), and the LSP
  `textDocument/publishDiagnostics` form is accepted too. They become
  `diagnostics` events with an absolute file path and 1-based line/column;
  an empty item list is forwarded too, since it means the file is clean
  now.
- **Real exit codes propagate end to end.** sbt's actual result becomes
  `sbtctl`'s process exit code so agents can branch on success/failure
  (clamped to 1..=124; 125 is reserved for tool failure, see CLI surface).

## HTTP contract

Served by `sbtd` on the loopback address; the DTOs are `protocol.rs`.
Written down in the rebuild as the contract the hand-off's `/exec`,
`/health` and status/shutdown verbs implied.

| Route | Body | Response |
|---|---|---|
| `GET /health` | – | `{ status: "ok", pid, version, uptime_secs }` |
| `GET /status` | – | `{ instances: [{ build_root, pid?, phase: booting\|idle\|running, idle_secs?, uptime_secs }] }` |
| `POST /exec` | `{ cwd, command_line, verbose? }` | `application/x-ndjson`, one event per line |
| `POST /shutdown` | `{ target: "one", cwd }` or `{ target: "all" }` | `{ stopped: [build_root] }` |

Errors are JSON `{ code, message, output? }`: `no_build_root` and
`bad_request` are 400, `project_load_failed` and `start_failed` are 500;
`output` carries the captured boot lines when the project failed to load.

`/exec` events are tagged by `type`, keep free text in `text`, and end
with exactly one terminal event, always last:

- `started { build_root, pid, cold_start }`, always first.
- `log { level: debug|info|warning|error, text }`, sbt's own logger;
  `debug` only when `verbose`.
- `output { stream: stdout|stderr|sbt_stdout|sbt_stderr, text }`;
  `stdout`/`stderr` are the JVM's raw pipes, `sbt_*` sbt's forwarded
  streams.
- `diagnostics { file, items: [{ line, column, severity, text }] }`,
  1-based; an empty `items` means the file is clean now.
- `done { outcome: success|failure, exit_code, elapsed_ms, summary }`.
- `aborted { reason: sbt_died|connection_lost|daemon_shutdown, summary }`.

RPC-derived events precede `done` in order; raw pipe lines carry no such
guarantee.

## CLI surface

- **Any sbt command line passes through**: `sbtctl compile`, `sbtctl test`,
  `sbtctl testOnly com.ex.FooSpec`, or any other raw command via clap's
  external-subcommand, joined with spaces into one command line. sbtctl's
  own flags must precede it: `-C <dir>` overrides cwd instead of using
  `$PWD`; `--daemon-url` (`SBTCTL_DAEMON_URL`, default
  `http://127.0.0.1:8077`) picks the daemon; `--state-dir`
  (`SBTD_STATE_DIR`) its pid and log directory; `--verbose` keeps sbt's
  debug-level lines.
- **Rendering mirrors sbt's own shell**: `log` events print as
  `[info]`/`[warn]`/`[error]` lines on stdout, raw and forwarded output
  on the stream it came from, the synthesized summary last; a cold start
  is announced on stderr as `[sbtctl] starting sbt for <root> (cold
  start)`. `diagnostics` events are not printed: sbt already logs every
  compiler message, and the structured form is for HTTP consumers.
- **Exit codes**: sbt's own code passes through, clamped to 1..=124 when
  nonzero; **125** means the tool failed rather than the build (an
  unreachable daemon, a refused request, an `aborted` exec, a protocol
  error), so the two can never collide. `daemon status` exits 1 when sbtd
  is not running.
- **`sbtctl status`** lists warm instances: build root, pid, phase
  (booting, idle, running), idle time, uptime. **`sbtctl shutdown` /
  `sbtctl shutdown --all`** stop one or all warm instances (bounded
  timeout, see Instance lifecycle).
- **`sbtctl daemon start|stop|restart|status`** manage `sbtd` itself; all
  three transition verbs poll the real `/health` endpoint to confirm the
  transition actually happened rather than firing-and-forgetting a signal.
  `start` finds `sbtd` next to its own executable, else on `PATH`, spawns
  it in its own process group with stdio appended to `sbtd.log`, and
  passes `--bind` derived from the daemon URL. `stop` reads `sbtd.pid`,
  refuses to act unless `/health` reports that same pid, then sends
  SIGTERM; stopping a daemon that is not running succeeds.
- **Signals go through `nix` (`sbtctl daemon stop`) and detachment through
  `CommandExt::process_group` (`daemon start`)**: a standard POSIX
  environment is assumed; untested on Windows.

## Distribution

- **One released package, `sbt-server`, two binaries.** cargo-dist
  (`dist-workspace.toml`, `.github/workflows/release.yml`, regenerated by
  `dist generate`) builds `sbtd` and `sbtctl` for `aarch64-apple-darwin`,
  `x86_64-apple-darwin` and `x86_64-unknown-linux-gnu` on every `v*` tag,
  publishes a GitHub Release with the archives and a `curl | sh` installer,
  and pushes a formula to `scottweaver/homebrew-tap`, so macOS users run
  `brew install scottweaver/tap/sbt-server`. No Windows target, ever; the
  Linux archive ships because CI already proves it, but macOS is the
  supported platform and Linux support comes later.
- **The Homebrew publish job needs the `HOMEBREW_TAP_TOKEN` secret** (a
  personal access token with the `repo` scope) in this repository; without
  it the release still publishes, only the formula push fails.
- **Versions are unified**: one workspace version, one tag (`v0.1.0`), one
  release; `fake-sbt` (`publish = false`) and the library crates are never
  released. Licensed Apache-2.0.

## Verified scope and open items

- **Only sbt 1.x server-protocol behaviour has been exercised** (project
  pinned at 1.11.7). The system's `sbt` launcher is 2.0.9, but the launcher
  downloads/runs whatever `project/build.properties` pins per project.
- **`instance.rs`'s sbt-facing logic is covered by integration tests**
  against the `fake-sbt` child process (handshake over local and
  TCP+token, takeover of cooperative, mute and slow-exit servers, exec
  event streams, child death mid-exec, bounded shutdown, boot failure and
  boot timeout), playing frames frozen from sbt 1.11.7. Two `#[ignore]`
  live tests against `tests/fixtures/hello-sbt` run with
  `SBT_SERVER_LIVE=1`.
- **Idle reaping exists** (see Instance lifecycle); its interval is fixed
  at one minute and only the timeout is configurable.
- **No auth** on the HTTP API beyond loopback binding.
- **Platforms: macOS is supported; Linux builds and passes CI and will be
  supported eventually; Windows is out of scope for good** (decided
  2026-10-29). Signals via `nix`, process groups and Unix sockets for
  sbt's local transport are all POSIX, and nothing Windows-shaped is
  planned.

## Audit triggers

Files whose changes warrant re-checking this doc during wrap-up:

- `crates/core/src/instance.rs`, `crates/core/src/instance/**`
  (takeover-on-start, bounded shutdown, output fan-in and ordering,
  synthesized completion line, descriptor parsing, event translation)
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
- `crates/sbt-server/**` and the cargo-dist configuration (what ships, for
  which targets, through which installers)
- `Cargo.toml` / `crates/*/Cargo.toml` (HTTP stack, async runtime, or any
  new transport dependency)

## Structural criteria

Structural (this doc must change in the same PR): any transport other than
loopback HTTP between `sbtctl` and `sbtd`, or anything that exposes sbt's
socket or JSON-RPC to agents; adding auth to the HTTP API; a change to the
takeover-on-start rule or the bounded-shutdown rule; a change to which
output channels are captured or to the synthesized completion-line format;
adopting a different sbt protocol (BSP, LSP) or framing; a change to
build-root resolution or to the per-instance serialization model; a change to the
idle-reaper rule or a new daemon lifecycle verb; swapping the HTTP
stack or the async runtime; dropping sbt 1.x support or adding sbt 2.x
support.

Not structural (no update needed): additional passthrough subcommands that
reuse `/exec` unchanged; log formatting; test changes; DTO field additions
that keep the existing endpoints' meaning.

## Maintenance

Update when a constraint above is deliberately renegotiated (design
dialog + PR updating this file), or when an open item is resolved
(auth, Linux support, sbt 2.x).
Never for in-flight status — that's STATE.md. Keep constraints
falsifiable and dated. `HANDOFF.md` at the project root is the frozen
2026-10-29 transcript this doc was converted from; do not edit it.
