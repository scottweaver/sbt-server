# Project State

The rehydration document. Any agent starting a session reads this
first to learn where the project stands right now. It answers "where
are we" — never "how does this work" (that's ARCHITECTURE.md and the
code) and never "how should we work" (that's METHODOLOGIES.md).

Last updated: 2026-10-29

## Active workstream

Rebuilding sbt-server from the hand-off in five phases, one local branch
each, merged `--no-ff` into `main` (plan approved 2026-10-29; no remote and
no pushes until the user says so). Phase 1 has landed: the Cargo workspace
(edition 2024, clippy pedantic, toolchain 1.95.0), `sbt-server-core` with
the agent-facing `protocol.rs` DTOs, the LSP-framed JSON-RPC connection
actor (`jsonrpc.rs` plus `jsonrpc/{framing,message}.rs`), `routing.rs`,
`paths.rs`, and the dev-only `crates/fake-sbt` double that plays frozen sbt
frames. 40 tests pass; fmt, `clippy -D warnings` and `test` are green.
Decisions carried into the next phases, all verified against sbt 1.13.x
source: sbt has no `sbt/shutdown` method, shutdown is `sbt/exec` with
`commandLine: "shutdown"`, and unknown methods get no reply (gotcha 4's
hang); the "Processing sbt/exec" noise is a debug-level `build/logMessage`,
so debug is dropped by default; request ids go out as JSON strings; no
`broadcast` channels anywhere (one bounded mpsc per instance); axum 0.8 for
`sbtd`; `nix` for signals; diagnostics forwarding and idle-timeout reaping
are in scope. Live fixture: `tests/fixtures/hello-sbt` (sbt 1.11.7).

## Branches in flight

| Branch | Purpose | Status |
|---|---|---|
| `main` | trunk | Phase 1 merged; Phase 2 next |

## Next up

1. **Phase 2 — `feat/instance`**: `instance.rs` and children (`descriptor`,
   `lines`, `events`, `completion`, `session` typestate, `output` router,
   child supervisor). Takeover connects, initializes and execs `shutdown`
   bounded, deletes `active.json`, then waits for the foreign endpoint to
   refuse connections before spawning. The exec task holds the owned lock
   and runs to sbt's terminal reply even if the consumer disconnects; a dead
   child yields `aborted: sbt_died`, never a fake exit code. Fake-child
   integration tests, an `#[ignore]` live test gated by `SBT_SERVER_LIVE=1`,
   spawn flags confirmed (`sbt --server -Dsbt.supershell=false
   -Dsbt.color=false -Dsbt.log.noformat=true`, stdio piped), real frames
   captured into `crates/fake-sbt/fixtures`. ARCHITECTURE.md edits in the
   same branch: shutdown mechanism, gotcha-8 root cause, diagnostics as a
   captured channel, mpsc wording, the takeover wait step.
2. **Phase 3 — `feat/daemon`**: `registry.rs` (slot enum over `watch`,
   detached boot task, `InstanceId` eviction, idle reaper), `sbtd` on axum
   (`LoopbackAddr` newtype, NDJSON `/exec`, pid file, SIGTERM drain), daemon
   integration tests via `CARGO_BIN_EXE_sbtd` and fake-sbt. Doc edits: axum
   replaces hand-rolled hyper, idle reaping, HTTP contract.
3. **Phase 4 — `feat/cli`**: `sbtctl` (passthrough, `-C`, `status`,
   `shutdown [--all]`, `daemon start|stop|restart|status` polling
   `/health`; exit codes: sbt's clamped to 1..=124, 125 for aborted or an
   unreachable daemon), cli tests, `scripts/smoke.sh`. Doc edits: `nix`
   kill, the 125 convention.
4. **Phase 5 — `chore/ci-and-state`**: `.github/workflows/ci.yml` (fmt,
   clippy, `cargo build -p fake-sbt`, test), README usage, final STATE.md;
   then create the GitHub repo and push, only when the user says so.

## Most recent meaningful progress

- **2026-10-29 — Phase 1: workspace and JSON-RPC client landed** (branch
  `feat/workspace-and-jsonrpc`, merged). Why: every later phase sits on
  the framing, message shapes and connection actor, and the fake lets all
  of it be tested without a JVM. Risk: the fake's frames are hand-written
  from reading sbt's source, not captured; Phase 2's live run replaces
  them. Open until that run: whether `--server` alone keeps sbt alive with
  piped stdin, whether `-Dsbt.log.noformat=true` strips ANSI from the raw
  pipes, whether `sbt/systemOut` arrives on a `--server` channel, and the
  first-boot wait ceiling (default 180 s).
- **2026-10-29 — Rules layer bootstrapped.** HANDOFF.md transcribed from
  five Drive photos (255 gutter lines, checked line for line), converted
  into ARCHITECTURE.md, and the standard agent-rules layer installed:
  CLAUDE.md/AGENTS.md, METHODOLOGIES.md, RUST_BEST_PRACTICES.md, PROJECT.md
  bound to GitHub Issues with ARCHITECTURE.md as the wrap-up audit doc.
  Why: the project arrives as prose only; the rules layer makes every
  constraint from the first build binding before any code is rewritten.
  Risk: no source exists in this checkout, so ARCHITECTURE.md describes a
  build that must be reproduced faithfully — drift between the doc and the
  rebuilt code is the main hazard until tests exist.

## Blocked / waiting

- *(nothing)*

## Maintenance

- **Refresh trigger:** any merge or milestone that changes what an
  incoming agent needs to know: workstream shifts, a branch opens or
  closes, "Next up" changes, something lands. Wired into
  METHODOLOGIES.md's post-merge routine (the "refresh STATE.md"
  step).
- **Always update:** "Last updated"; "Branches in flight"; prepend a
  progress entry (what / why / risk voice — a judgment edit, not a
  paste of the PR description).
- **As applicable:** "Active workstream" paragraph, "Next up",
  "Blocked / waiting".
- **Trim policy:** progress log holds at most 10 entries — drop the
  oldest when adding. Anything stable graduates out of this file
  into the appropriate rules doc; this file stays small because
  every agent loads it every turn.
- **Edit policy:** STATE.md is authored on feature branches,
  propagates through merges, and is refreshed (not deleted) on new
  branches. Never edit it directly on `main`; docs-only diffs under
  `.claude/rules/` ride the METHODOLOGIES.md docs-only carve-out.
- **Keep entries short:** each progress entry is a pointer — date,
  PR #, ticket, a sentence or two of judgment. If you're tempted to
  write more, the detail belongs in the PR, commit, or ticket.
