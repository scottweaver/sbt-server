# Project State

The rehydration document. Any agent starting a session reads this
first to learn where the project stands right now. It answers "where
are we" — never "how does this work" (that's ARCHITECTURE.md and the
code) and never "how should we work" (that's METHODOLOGIES.md).

Last updated: 2026-10-30

## Active workstream

The rebuild of sbt-server from the hand-off is complete: five phases, one
local branch each, merged `--no-ff` into `main` on 2026-10-29. The core
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
`scottweaver/sbt-server` exists and `main` is pushed. 121 tests pass; the two
`#[ignore]` live tests pass with `SBT_SERVER_LIVE=1`; `scripts/smoke.sh`
passes end to end against real sbt; fmt, `clippy -D warnings` and `test`
are green.

The active workstream since 2026-10-30 is security hardening: issues under
the `security` label (#18 to #27, plus #34 for CodeQL), each its own PR from
`main`, never stacked, worked in parallel sessions, one per issue. Landed:
#18 (sbtd refuses a non-loopback `Host` or any `Origin`, 403 `forbidden`,
`crates/daemon/src/guard.rs`), #22 (ci.yml: read-only token, SHA-pinned
actions, `--locked`) and #24 (Dependabot for cargo and GitHub Actions).
Open and green: #20 (PR #32) and #19 (PR #33).

## Branches in flight

| Branch | Purpose | Status |
|---|---|---|
| `main` | trunk | at `edeb306` (#31 merged); `v0.1.1` is the last release |
| `claude/state-dir-permissions-26f4b8` | #20: state dir 0700, files 0600, token redacted from traces | PR #32, green after a merge from `main`, awaiting review |
| `fix/validate-active-json` | #19: `active.json` must name loopback TCP and a token file under `~/.sbt` | PR #33, green, awaiting review |
| *(worker session)* | README badges: release, CI, coverage, security | design dialog + issue first; no branch yet |

## Next up

1. **Security hardening, one PR per issue, never stacked.** Merge #32
   (#20) and #33 (#19), wrap each up. Then #23 (cargo-deny: adds a job to
   ci.yml) and #34 (CodeQL: its own workflow) can run in parallel. #21
   (ruleset on `main`, vulnerability alerts, auto-merge; settings only,
   closed by hand) once the required check names are settled: `check
   (ubuntu-latest)`, `check (macos-latest)`, plus whatever #23 and #34
   add. #25 (cargo-auditable, CycloneDX SBOM, GitHub attestations, pinned
   release actions) rides the next release and now also owns the
   release.yml pinning rule: `release.yml` is cargo-dist-generated and
   `dist plan` fails on any hand edit, so its action pins go through the
   `github-action-commits` table, and Dependabot PRs that touch it are
   closed by hand (#35 was the first). #26 (bearer-token auth on sbtd) is
   a design dialog and structural per ARCHITECTURE.md; #27 (macOS signing
   and notarization) stays open until a team reports an endpoint policy
   block.
2. **Releasing** is always: bump the workspace version in a PR, merge,
   push a `v*` tag, and the workflow does the rest; then
   `brew upgrade sbt-server`. The `HOMEBREW_TAP_TOKEN` secret is a
   fine-grained PAT scoped to the tap with Contents read/write; when it
   expires only the formula push fails.
3. **Socialising it**: `docs/agents.md` is the page to hand to a team.
   What a security review will ask for is what #23, #25 and #27 produce
   (license report, SBOM, provenance, a signature).
4. **Still open from the hand-off**: auth on sbtd beyond loopback (now
   #26), Linux as a supported platform, sbt 2.x (only 1.x's protocol has
   been exercised), a launchd recipe for running sbtd at login. Each is a
   design dialog plus an ARCHITECTURE.md change, not a quick fix.
5. **Daily use will surface the rest**: the cold-start boot time is not in
   the `[success]` summary; `sbtctl` does not render diagnostics; the
   reaper interval is a one-minute constant.

## Most recent meaningful progress

- **2026-10-30 — #18, #22, #24 landed (PRs #31, #29, #30); #19 and #20
  open (PRs #33, #32); #34 filed.** Why: the first batch of the security
  workstream, run as five parallel sessions, one per issue, from `main`.
  #18 adds `guard.rs`, a `from_fn` layer that parses `Host` into a
  loopback literal or `localhost` and refuses anything else or any
  `Origin` with 403 `forbidden`; the accepted IP set is any loopback
  address, matching `--bind`, a deliberate widening of the issue. Lessons:
  five PRs editing ARCHITECTURE.md and the integration test files conflict
  once the first merges (PR #32 needed a merge from `main`); Dependabot's
  first PR (#35) failed `dist plan` because it edited the generated
  release.yml, now a constraint recorded on #25; the `dependencies` label
  Dependabot applies had to be created by hand. Risk: #32 and #33 are
  green but unreviewed; the parallel briefs told workers not to edit
  STATE.md, so this entry is the only record of the batch.
- **2026-10-30 — Security hardening scoped (issues #18 to #27).** Why: the
  real test of the tool is installing it on a managed laptop and handing
  it to other teams, and that means a security review. The assessment
  found one hole reachable from outside (no Host or Origin check, so DNS
  rebinding reaches `/exec` from a web page), two local-trust gaps
  (`active.json` trusted for any TCP host and any token-file path; a 0755
  state dir with a 0644 log of raw build output, and the sbt token in
  trace frames), and a supply chain with no pins, no advisory check and
  no provenance. Ten issues under a new `security` label, each carrying
  Why / Change / Done when, so the issue is the spec. Risk: cargo-deny
  will fail unrelated PRs on a new advisory, by design;
  `github-attestations` is experimental in cargo-dist 0.32.
- **2026-10-29 — v0.1.1 released (PR #16).** The release workflow passed
  every job; the formula in the tap moved to 0.1.1; `brew upgrade` on
  this Mac installed it, and `sbtctl start` now answers with the usage
  error, exit 2, while a real `sbtctl test` still runs through. Homebrew
  users get the fix with `brew upgrade sbt-server`.
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

## Blocked / waiting

- *(nothing)*

## Session handoff

<!-- transient; owned by the checkpoint skill -->

- **Resume here:** review and merge PR #32 (#20) and PR #33 (#19), run
  the wrap-up for each, then start #23 and #34 as two parallel sessions
  from `main`. The README-badges session is in a design dialog about the
  coverage pipeline (tool, upload service, token) and will file its issue
  before coding.
- `sbtctl daemon start` cannot start a daemon for
  `--daemon-url http://localhost:<port>`: `bind_from_url` hands
  `localhost:<port>` to `--bind`, which only parses IP literals. Found
  while writing #18's CLI test; not filed yet.
- The issues were drafted in chat and filed on 2026-10-30, all assigned,
  bodies cross-linked (#22 and #24 cite each other, #26 cites #18).
  Nothing about them is written down anywhere but the issues themselves.
- Deliberately not filed: a gitleaks CI job (GitHub secret scanning and
  push protection are already on for the repo), and the local pre-commit
  scrub for names that must never appear in the repo; its word list stays
  outside the repo.
- Repo settings observed 2026-10-30, recorded in #21: default workflow
  token read-only; secret scanning and push protection on; vulnerability
  alerts, Dependabot security updates and auto-merge off; `main`
  unprotected. Until #21 lands, docs carve-out PRs are merged by hand
  with `--squash --delete-branch`, not `--auto`. A `dependencies` label
  now exists for Dependabot's PRs.
- cargo-dist 0.32 keys verified against its docs for #25:
  `cargo-auditable`, `cargo-cyclonedx`, `github-attestations`
  (experimental), `github-action-commits` (a table that survives
  `dist generate`). There is no macOS signing key, so #27 would be a
  custom codesign plus notarytool job.

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
