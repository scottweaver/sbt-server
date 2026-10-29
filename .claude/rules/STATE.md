# Project State

The rehydration document. Any agent starting a session reads this
first to learn where the project stands right now. It answers "where
are we" — never "how does this work" (that's ARCHITECTURE.md and the
code) and never "how should we work" (that's METHODOLOGIES.md).

Last updated: 2026-10-29

## Active workstream

Rebuilding sbt-server from the hand-off in five phases, one local branch
each, merged `--no-ff` into `main` (plan approved 2026-10-29; no remote and
no pushes until the user says so). Phases 1, 2 and 3 have landed. The core
crate holds the agent-facing `protocol.rs` DTOs, the JSON-RPC connection
actor, `routing.rs`, `paths.rs`, `registry.rs` (shared boots, eviction of
dead instances, idle reaper) and the whole instance layer:
descriptor parsing, line assembly, notification translation (including
sbt 1.x's `build/publishDiagnostics`), the synthesized completion line, the
`Unauthenticated` to `SbtSession` typestate, the output router that owns
the instance phase, and `Instance::{start, exec, shutdown}` with
takeover-on-start, a bounded `shutdown` exec and a child supervisor. The
spawn contract was settled live on sbt 1.11.7 and is recorded in
ARCHITECTURE.md: `--server` plus `SBT_TERMINAL_PROPS` so sbt adds no
console channel (the alternative flags all flooded the raw pipes with
prompt escapes or, for `--detach-stdio`, closed them). `crates/fake-sbt`
now plays frames frozen from real captures, and its own integration test
keeps its binary fresh under `cargo test --workspace`. `crates/daemon` is
`sbtd` on axum 0.8: `LoopbackAddr` bind, `/health`, `/status`, NDJSON
`/exec`, `/shutdown`, pid and addr files, SIGTERM drain; smoke-tested live
with curl against the fixture. 93 tests pass; the two `#[ignore]` live
tests pass with `SBT_SERVER_LIVE=1`; fmt, `clippy -D warnings` and `test`
are green.

## Branches in flight

| Branch | Purpose | Status |
|---|---|---|
| `main` | trunk | Phases 1–3 merged; Phase 4 next |

## Next up

1. **Phase 4 — `feat/cli`**: `sbtctl` (passthrough via clap's external
   subcommand, `-C`, `--daemon-url`/`SBTCTL_DAEMON_URL`, `--verbose`,
   `status`, `shutdown [--all]`, `daemon start|stop|restart|status`; `start`
   finds `sbtd` next to its own executable else on PATH and polls
   `/health`; `stop` reads `sbtd.pid`, checks `/health` reports that pid,
   sends SIGTERM through `nix` and polls until `/health` fails; exit codes:
   sbt's clamped to 1..=124, 125 for `aborted` or an unreachable daemon),
   cli tests against an in-test axum fake daemon, `scripts/smoke.sh`. Doc
   edits: `nix` kill, the 125 convention.
2. **Phase 5 — `chore/ci-and-state`**: `.github/workflows/ci.yml` (fmt,
   clippy, test), README usage, final STATE.md; then create the GitHub repo
   and push, only when the user says so.

## Most recent meaningful progress

- **2026-10-29 — Phase 3: registry and daemon landed** (branch
  `feat/daemon`, merged). Why: with the instance layer proven, the daemon
  is a thin async shell around it, and writing the HTTP contract down in
  ARCHITECTURE.md gives Phase 4's CLI a fixed target. Design points worth
  knowing: boots run as detached tasks shared through a `watch` channel,
  the reaper never touches a running instance, and SIGTERM drains every
  instance before axum stops so streaming clients get `aborted:
  daemon_shutdown`. Risk: `sbtd` has no auth beyond loopback (unchanged
  from the hand-off), and the reaper interval is a one-minute constant.
- **2026-10-29 — Phase 2: instance layer landed** (branch `feat/instance`,
  merged). Why: this is the part the hand-off had no tests for, and the
  part whose behaviour only live testing could settle; both are now
  covered (13 fake-child integration tests, 2 live tests). Three facts the
  sources did not show came from the live runs and are in ARCHITECTURE.md:
  diagnostics arrive as `build/publishDiagnostics`, a failed command is a
  JSON-RPC error with code -33000 or -32603 (the latter with the failure
  text), and `shutdown` is answered by a `shutdown` notification and a
  closed socket, never a response. Risk: `SBT_TERMINAL_PROPS` is sbtn's
  private handshake, not documented; if a future sbt renames it the
  console channel returns and the live test's zero-escape assertion is
  what will catch it. Command-parse errors ("Not a valid command") never
  reach a network client; agents get only the synthesized `[error]` line.
- **2026-10-29 — Phase 1: workspace and JSON-RPC client landed** (branch
  `feat/workspace-and-jsonrpc`, merged). Why: every later phase sits on
  the framing, message shapes and connection actor, and the fake lets all
  of it be tested without a JVM. Risk: resolved in Phase 2 (fixtures are
  now frozen from captures).
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
