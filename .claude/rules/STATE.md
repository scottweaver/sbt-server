# Project State

The rehydration document. Any agent starting a session reads this
first to learn where the project stands right now. It answers "where
are we" — never "how does this work" (that's ARCHITECTURE.md and the
code) and never "how should we work" (that's METHODOLOGIES.md).

Last updated: 2026-10-29

## Active workstream

The rebuild of sbt-server from the hand-off is complete: five phases, one
local branch each, merged `--no-ff` into `main` on 2026-10-29. There is no
remote and nothing has been pushed; that is the next step and it waits
for the user. The core
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
with curl against the fixture. `crates/cli` is `sbtctl`: passthrough,
`-C`, `--daemon-url`, `--state-dir`, `--verbose`, `status`, `shutdown
[--all]`, `daemon start|stop|restart|status` confirmed through `/health`,
sbt's exit code clamped to 1..=124 and 125 reserved for tool failure.
`.github/workflows/ci.yml` runs fmt, clippy and the tests on ubuntu and
macos; `README.md` covers install and usage. The remote
`scottweaver/sbt-server` exists and `main` is pushed. 107 tests pass; the two
`#[ignore]` live tests pass with `SBT_SERVER_LIVE=1`; `scripts/smoke.sh`
passes end to end against real sbt; fmt, `clippy -D warnings` and `test`
are green.

## Branches in flight

| Branch | Purpose | Status |
|---|---|---|
| `main` | trunk | docs (#11), README (#15) and the forgot-daemon fix (#14) merged after `v0.1.0`; 0.1.1 release in flight |

## Next up

1. **Release 0.1.1**: this bump merges, then `git tag -a v0.1.1 && git
   push origin v0.1.1`; after the workflow, `brew upgrade sbt-server` and
   check `sbtctl start` gives the usage error. Releasing is always: bump
   the workspace version, merge, push a `v*` tag. The `HOMEBREW_TAP_TOKEN`
   secret is a fine-grained PAT scoped to the tap with Contents
   read/write; when it expires only the formula push fails.
2. **Socialising it**: `docs/agents.md` is the page to hand to a team. If
   an endpoint policy rejects unsigned binaries, code signing and
   notarization become the next ticket; nothing in the pipeline does them.
3. **Still open from the hand-off**: auth on sbtd beyond loopback, Linux as
   a supported platform, sbt 2.x, a launchd recipe for running sbtd at
   login.
2. **Open items the hand-off left, still open**: auth on `sbtd` beyond
   loopback; Windows (signals, process groups, named pipes); sbt 2.x
   (only 1.x's protocol has been exercised). Each is a design dialog plus
   an ARCHITECTURE.md change, not a quick fix.
3. **Daily use will surface the rest**: the cold-start boot time is not in
   the `[success]` summary; `sbtctl` does not render diagnostics; the
   reaper interval is a one-minute constant.

## Most recent meaningful progress

- **2026-10-29 — Docs, README and the forgot-daemon fix landed** (PRs
  #11, #15, #14; issues #6, #7, #13). Why: the user's first real-project
  run surfaced `sbtctl start` as the common slip with feedback that never
  named the daemon; the fix refuses bare daemon verbs (exit 2) with the
  right command, adds hints to every error, names non-sbtd answers, and
  makes sbtd reject bad bodies in its own shape. The docs and README are
  what other teams will see first. Lesson: GitHub closes a PR whose base
  branch is deleted, so the stacked README PR (#12) had to be reopened
  as #15 from `main`; do not stack PRs, or merge the top one first. Risk:
  Homebrew users on 0.1.0 lack the fix until 0.1.1 ships.
- **2026-10-29 — v0.1.0 released.** Tag `v0.1.0` on `66a6c08`; the
  release workflow passed every job, the release carries the three
  archives, checksums, `sbt-server-installer.sh` and the formula, and the
  formula landed in `scottweaver/homebrew-tap`. Verified on this Mac:
  `brew install scottweaver/tap/sbt-server` put 0.1.0 under
  `/opt/homebrew/bin`, and `sbtctl test` on a real munit project ran
  through it. Risk: only macOS arm64 was exercised by hand; the x86_64
  macOS and Linux archives are built and checksummed but untried.
- **2026-10-29 — Release pipeline landed (issue #5, PR #8, merged at
  `abf46d4`, CI green on both platforms with the release plan job
  passing).** Apache-2.0 chosen as the single license
  (the permissive one companies accept most readily). cargo-dist releases
  per package and cannot bundle binaries across packages, so the daemon
  and CLI became library crates and a new `crates/sbt-server` package ships
  both binaries; the binary-spawning integration tests moved with them.
  `dist plan` shows one app with both binaries for the three targets, a
  shell installer and a Homebrew formula. The tap repository
  `scottweaver/homebrew-tap` exists. Risk: the `HOMEBREW_TAP_TOKEN`
  secret is still missing and nothing has been tagged, so the release
  path is unexercised in CI.
- **2026-10-29 — Fix: test-framework output was lost** (issues #1 and #2,
  PR #3, merged at `289384b`, CI green on ubuntu and macos). Found by the user on a real munit
  project right after the first install: `sbtctl test` printed only the
  `[info] Passed:` summary. Cause: a channel's terminal is sbt's
  `NullTerminal` until the client sends `sbt/attach`, and the test
  framework writes its lines there. Fix: the handshake now attaches as a
  non-interactive client (as sbtn does for batch commands), the client
  answers sbt's terminal queries as a dumb 80x24 terminal, and, as sbtn
  does, `build/logMessage` is dropped except at debug level because the
  attached terminal stream carries the same lines rendered; lines with
  sbt's `[level] ` prefix are parsed back into `log` events. Fixture
  refrozen from an attached capture. The first CI run on `main` (ubuntu)
  then exposed a second defect, fixed in the same PR: the child's exit was
  reported before its pipes were drained, so a fast-dying sbt's last lines
  (the ones that explain a failed project load) could be missing from
  `project_load_failed` (issue #2). Risk: sbt's terminal queries are
  answered generically; an sbt that asks something new would get "method
  not found", and its `getCapability` has no timeout.
- **2026-10-29 — Phase 5: CI, README, rebuild complete** (branch
  `chore/ci-and-state`, merged). Why: the workflow makes the three gates
  the repo already passes locally binding once a remote exists, and the
  README is what someone installing the tool reads first. Risk: CI has
  never run (no remote); the first push will tell.
- **2026-10-29 — Phase 4: sbtctl landed** (branch `feat/cli`, merged).
  Why: the agent-facing surface is the point of the project; it is a thin
  client over the HTTP contract plus the daemon lifecycle verbs, and the
  smoke script proves the whole chain on real sbt. Decisions: diagnostics
  are not rendered by the CLI (sbt logs each compiler message already),
  sbtctl's flags must precede the sbt command line, and `daemon stop`
  refuses to signal unless the pid file and `/health` agree. Risk: the
  cold-start boot time is not inside the `[success]` summary's elapsed
  figure; the `[sbtctl] starting sbt` line on stderr is the only hint.
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
