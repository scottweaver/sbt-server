# sbt-server: Development Handoff

## What this is

A local daemon (`sbtd`) that keeps one warm sbt server process per project and
routes commands to it, plus an agent-facing CLI (`sbtctl`) that talks to the
daemon over loopback HTTP. Rust workspace at `~/Projects/sbt-server`
(no git repo yet).

**Goal:** avoid sbt's slow cold start (JVM boot + build-definition load) on
every agent-driven `compile`/`test` by keeping sbt warm across calls, while
routing each call to the right project automatically.

## Hard constraint that shaped the whole design

Agents in this environment may **not** use custom MCP servers, and may
**not** use Unix sockets. So:

- `sbtctl` (what agents actually invoke, via their shell tool) talks to
  `sbtd` only over **loopback HTTP** (`127.0.0.1:8077` by default).
- `sbtd` internally drives each warm sbt instance via sbt's own JSON-RPC
  server protocol, which may itself be a Unix socket — that's fine, because
  it's strictly `sbtd`↔`sbt`, never exposed to the agent.

Any redesign must preserve: **agents only ever speak loopback HTTP to `sbtd`.**

## Repo layout

```
Cargo.toml                      workspace (3 members)
crates/
  core/   sbt-server-core (lib, no binary)
    src/instance.rs    owns one warm sbt process + its JSON-RPC connection
    src/jsonrpc.rs     transport-generic JSON-RPC 2.0 client (TCP or Unix socket)
    src/registry.rs    build-root -> warm Instance map, lazy-start, shutdown
    src/routing.rs     cwd -> build root resolution (walks up for build.sbt /
                       project/build.properties)
    src/protocol.rs    HTTP request/response DTOs shared by daemon+cli
    src/paths.rs       shared state dir (~/.config/sbt-server)
  daemon/ sbtd (bin)   hand-rolled hyper 1.x HTTP server wrapping Registry
  cli/    sbtctl (bin) agent-facing client; clap + reqwest
```

Build/test: `cargo build` / `cargo test` (offline-capable; no `axum`, uses
raw `hyper`+`hyper-util`+`http-body-util` because `axum` wasn't in the local
offline cargo cache when this was built — swap back to `axum` if that's ever
not a constraint).

## Feature set (current, working)

- `sbtctl compile`, `sbtctl test`, `sbtctl testOnly com.ex.FooSpec`, or any
  other raw sbt command line — passthrough via clap's external-subcommand.
- `-C <dir>` to override cwd instead of using `$PWD`.
- `sbtctl status` — lists warm instances: build root, pid, busy, idle time.
- `sbtctl shutdown` / `sbtctl shutdown --all` — stop one or all warm
  instances (bounded timeout, see gotcha #6).
- `sbtctl daemon start|stop|restart|status` — manage `sbtd` itself; all three
  verbs poll the real `/health` endpoint to confirm the state transition
  actually happened rather than firing-and-forgetting a signal.
- Streaming output: `/exec` returns NDJSON over HTTP, one line per log
  event, so output appears live instead of buffering until the whole
  command finishes.
- Real exit codes propagate end-to-end: sbt's actual result -> `sbtctl`'s
  process exit code, so agents can branch on success/failure.
- Lazy start + warm reuse: first call for a project pays full JVM boot cost;
  every subsequent call against the same `sbtd` process reuses the warm
  instance from `Registry`'s in-memory map.
- Multiple projects run concurrently (different `Instance`s); same project
  serializes concurrent execs (sbt has one global build state per process).

## Gotchas found only by live testing (not from reading sbt's source alone)

Each of these produced a real, confusing symptom before being understood.
Listed in the order they were hit.

### 1. sbt's default transport is a Unix socket here, not TCP

**Symptom:** `could not parse sbt connection descriptor: unsupported server
uri: local:///Users/.../sock`.

sbt's own source (`CommandExchange.scala`) codes `ConnectionType.Tcp` as the
default, but on this machine sbt actually picked a Unix domain socket
(`local:///path/to/sock` in `active.json`).

**Fix:** `jsonrpc.rs`'s `JsonRpcClient` is transport-generic — a boxed
`dyn AsyncRead + AsyncWrite` trait object — with `connect_tcp` and
`connect_unix` constructors. `instance.rs::parse_server_uri` handles both
`tcp://host:port` and `local:///path` URIs from `active.json`.

### 2. Token auth is optional, not mandatory

**Symptom:** `could not parse sbt connection descriptor: active.json has no
tokenfilePath`.

Confirmed in sbt's source: *"Over the default Local transport
isAuthenticated is always true"* — sbt treats the connecting process as
already authenticated via Unix-socket filesystem permissions, and simply
omits `tokenfilePath` from `active.json` in that case. It's not a malformed
file; it's working as designed.

**Fix:** `read_connection_info` and the `initialize` handshake both treat
the token as `Option<String>` — only read/send one if `tokenfilePath` is
actually present.

### 3. Never trust a pre-existing `active.json`

**Symptom:** fixes appeared to do nothing — identical output across
restarts, no matter what changed in the code.

**Root cause:** the original design reused any already-running sbt server
it found described by `active.json`. That file can describe *any* process
with a live server for that build root — a manual `sbt` shell left open, an
editor's background instance, a stale crashed server — not necessarily one
`sbtd` controls. Silently trusting it meant every request was served by a
foreign process with no stdio access (no raw-output capture, see gotcha #5)
and no lifecycle control, with zero visible difference to the caller. Every
"fix" to the spawn path was invisible because the process actually answering
requests was never replaced.

**Fix:** `Instance::start` now *always* takes over. If `active.json`
exists: connect to whatever's there, send a best-effort bounded
`sbt/shutdown`, delete the descriptor regardless of whether that worked,
then *always* spawn a fresh `sbt` process. One-time cold-boot cost per
daemon restart per build root — correctness over speed.

### 4. `sbt/shutdown` can hang forever with no response

**Symptom:** `sbtctl shutdown --all` just hangs indefinitely.

sbt's server dispatches requests through a handler chain with no catch-all;
a request it doesn't recognize (wrong method/params shape) is silently
dropped — no response, no error. The original shutdown code awaited the RPC
with no timeout.

**Fix:** wrap the `sbt/shutdown` request in `tokio::time::timeout` (5s), then
proceed to reap/kill the child process regardless of whether the RPC
succeeded, timed out, or errored.

### 5. Output that bypasses sbt's own logger needs separate capture

**Symptom:** the user expected to see a `gcloud` credential-refresh warning
(from a build plugin shelling out to `gcloud`) in the streamed output; it
never appeared.

sbt's JSON-RPC notifications (`build/logMessage`, `sbt/systemOut/Err`) only
cover sbt's *own* structured logger. A subprocess (like `gcloud`) invoked by
a build plugin writes straight to the JVM process's own stdout/stderr,
bypassing that logger entirely — so it never becomes a JSON-RPC notification
at all.

**Fix:** `spawn_sbt` pipes the child's real stdout/stderr and fans every raw
line into a `broadcast` channel (`raw_output`). `Instance::exec` subscribes
to this alongside the JSON-RPC notification stream and forwards both to the
caller.

Caveat actually confirmed during testing: this particular `gcloud` warning
fires during **project loading**, before the JSON-RPC handshake completes
and before any `exec()` call/subscriber exists — so it is architecturally
impossible to attribute to a specific `compile` invocation. It's a
one-time-per-JVM-boot event, not a per-command one. Not a capture bug; a
timing fact of this particular case.

### 6. sbt never prints its own `[success] Total time: ...` banner to a client

**Symptom:** even with raw stdout/stderr capture working (confirmed via
`RUST_LOG=debug` daemon logs showing every other line), the final
`[success] Total time: ...` summary line a human sees in an interactive `sbt`
shell never appeared over any channel.

**Root cause, confirmed from sbt's own source** (`Aggregation.scala`):

```scala
val jobParams = StandardMain.exchange.handedOffToClient
...
if !isPaused && show.success && ... && !jobParams then
  printSuccess(start, stop, extracted, success, cacheSummary, log)
```

`handedOffToClient` is true for anything run via a JSON-RPC client (us).
sbt deliberately skips `printSuccess` in that case — it only ever prints for
sbt's own interactive shell loop. This is not a bug anywhere in our stack;
the banner structurally cannot arrive over any stream a client has access
to.

**Fix:** stop waiting for a line that will never come. `Instance::exec`
synthesizes an equivalent completion line itself (command, exit code,
elapsed time), e.g. `[success] 'compile' completed in 1.2s` or
`[error] 'compile' failed (exit code 1) in 0.8s`, appended after any real
completion message sbt's RPC response did include.

### 7. Nested Tokio runtime panic in `sbtctl daemon status`

**Symptom:** `thread 'main' panicked: Cannot start a runtime from within a
runtime.`

An early version of `run_daemon_action` tried to spin up a *second* Tokio
runtime via `tokio::runtime::Runtime::new().block_on(...)` from inside code
already running under `main`'s runtime.

**Fix:** made the whole daemon-action path properly `async fn` and awaited
in place, instead of block_on-ing a nested runtime.

### 8. Noisy, content-free "Processing sbt/exec" lines

**Symptom:** a no-op `compile` (nothing to recompile) printed two identical,
useless `Processing sbt/exec` lines and nothing else — looked like the
command silently did nothing, even on a genuinely successful run.

These are bare `ExecStatusEvent` progress notifications sbt pushes with no
`message` field, fired regardless of whether anything is actually happening.

**Fix:** `notification_to_exec_event` now only forwards `sbt/exec`
notifications that carry an actual non-empty `message`; bare status pings
with none are dropped.

## Design decisions made along the way (not bugs, but worth preserving)

- **Language/transport:** Rust (tokio async), chosen specifically for a
  single static binary and strong async process supervision. Agent surface
  is `sbtctl` CLI -> loopback HTTP, explicitly to satisfy the no-MCP /
  no-Unix-socket-for-agents constraint.
- **sbt driver:** sbt's own JSON-RPC server protocol (hand-rolled client,
  not an existing BSP/LSP crate) — **this is not real LSP or BSP.** It
  reuses LSP's framing (`Content-Length`-prefixed JSON-RPC) and borrows
  LSP's `initialize` handshake shape, but the actual commands (`sbt/exec`,
  `sbt/shutdown`, `sbt/systemOut`) are sbt's own proprietary protocol — the
  same one sbt's thin client (`sbtn`) uses. BSP (Build Server Protocol) is a
  separate real standard sbt also implements (for Metals/IntelliJ); this
  project doesn't use it at all.
- **Routing:** cwd -> build root by walking upward for `build.sbt` or
  `project/build.properties`; lazy-start on first use, warm until an
  explicit shutdown.
- **Per-instance serialization:** sbt has one global build state per
  process, so concurrent `exec` calls against the *same* instance are
  serialized via a `Mutex<()>` guard; different build roots run fully
  concurrently.

## Known open items / not yet verified

- No automated test coverage for `instance.rs`'s sbt-facing logic (handshake,
  exec, shutdown) — only `routing.rs`'s pure cwd-resolution logic has unit
  tests. Everything else was verified by live manual testing against real
  sbt projects, not captured as repeatable tests.
- No idle-timeout/auto-reap for warm instances — they stay warm until an
  explicit `sbtctl shutdown` or daemon restart. Mentioned as a future
  possibility, not implemented.
- No auth on `sbtd`'s HTTP API beyond binding strictly to loopback — treated
  as acceptable for a localhost dev tool, not revisited since.
- Only tested with sbt 1.x server protocol behavior in practice (even though
  the system's `sbt` launcher is 2.0.9 — the launcher downloads/runs
  whatever version `project/build.properties` pins per-project, and the
  project actually tested against pinned 1.11.7).
- `sbtctl daemon stop`'s use of `kill <pid>` assumes a standard POSIX
  environment; untested on Windows.
