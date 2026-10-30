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
`scottweaver/sbt-server` exists and `main` is pushed. 137 tests pass; the two
`#[ignore]` live tests pass with `SBT_SERVER_LIVE=1`; `scripts/smoke.sh`
passes end to end against real sbt; fmt, `clippy -D warnings` and `test`
are green.

The active workstream since 2026-10-30 is security hardening: issues under
the `security` label (#18 to #27, plus #34 for CodeQL), each its own PR from
`main`, never stacked, worked in parallel sessions, one per issue. Landed:
#18 (sbtd refuses a non-loopback `Host` or any `Origin`, 403 `forbidden`,
`crates/daemon/src/guard.rs`), #22 (ci.yml: read-only token, SHA-pinned
actions, `--locked`) and #24 (Dependabot for cargo and GitHub Actions).
Also landed: #20 (state dir 0700, `sbtd.pid`/`sbtd.addr`/`sbtd.log`
0600, sbt token redacted from trace frames) and #19 (`active.json` is
trusted only when it names a loopback TCP address or a local socket and a
token file under `~/.sbt`, checked before connecting). Nothing from the
first batch is open; #23 and #34 are next.

## Branches in flight

| Branch | Purpose | Status |
|---|---|---|
| `main` | trunk | at `ab8d62f` (#41 merged); `v0.1.1` is the last release |
| `docs/security-policy` | #42: SECURITY.md pointing at GitHub private vulnerability reporting | PR #44, CI green, awaiting merge |
| `test/fuzz-parsers` | #43: cargo-fuzz targets over the five pure parsers, `fuzz/` outside the workspace | PR #45, 60 s seed runs clean locally, awaiting CI and merge |

## Next up

1. **Security hardening, one PR per issue, never stacked.** #23
   (cargo-deny: adds a job to ci.yml) and #34 (CodeQL: its own workflow)
   can run in parallel now, each in its own session from `main`. #21
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
   block. Scorecard scored `main` 4/10 on 2026-10-30 and maps onto the
   open issues: Security-Policy is #42, Fuzzing is #43, SAST is #34,
   Branch-Protection is #21, Pinned-Dependencies and Signed-Releases are
   #25. #21's recipe is a ruleset on `main`: block force pushes and
   deletion, require a pull request with 1 approval and stale-approval
   dismissal, require `check (ubuntu-latest)`, `check (macos-latest)` and
   `coverage` up to date, with Repository admin in the bypass list for
   pull requests only; that reaches Scorecard tier 3 (8/10) for a solo
   maintainer. Token-Permissions stays 0 because cargo-dist writes
   `contents: write` at the top of `release.yml` with no knob to move it;
   only an upstream change fixes it. Code-Review, Maintained, Contributors
   and CII-Best-Practices are accepted as is. The CodeQL badge is added by
   #34's own PR (the line is a comment on #34).
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

- **2026-10-30 — Badges, coverage and Scorecard landed (#36 via PR #37,
  #40 via PR #41); #42 and #43 opened with PRs #44 and #45.** Why: a
  team evaluating the tool reads the badge row first. `coverage.yml` runs
  cargo-llvm-cov on ubuntu with the spawned `sbtd`/`sbtctl`/`fake-sbt`
  children instrumented and `fake-sbt` excluded from the number, uploads
  to Codecov over OIDC (no secret; the user activated the repo by
  installing the Codecov app), and `main` reads 91%. `scorecard.yml`
  published the first score, 4/10, which the triage in Next up turned
  into work. Lessons: a Codecov upload fails with "Repository not found"
  until the repo is activated; Scorecard's Packaging check does not
  recognise cargo-dist's Homebrew job (unscored, harmless); two sessions
  sharing one checkout cost a stray commit on `ci/coverage`, so side
  sessions now work in their own worktrees. Risk: #44 and the fuzz PR
  merged on green CI without a second reader; the fuzz targets only run
  by hand.
- **2026-10-30 — #19 and #20 landed (PRs #33, #32).** Why: the two
  local-trust gaps from the assessment. #20 makes the state dir 0700 and
  its files 0600 and keeps the sbt token out of trace frames; #19 refuses
  an `active.json` naming a non-loopback TCP host or a token file outside
  `~/.sbt` before connecting to anything. Both needed `main` merged in
  before they could land, because every PR of the batch edits
  ARCHITECTURE.md and the daemon; expect the same for any two PRs of this
  workstream open at once. Risk: both merged on green CI without a human
  read of the diff. The coverage PR (#37) is the first to exercise OIDC
  upload to Codecov and is blocked on activating the repo there.
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

## Blocked / waiting

- PRs #44 and #45 wait on a merge by hand; both are green and docs or
  test-only.

## Session handoff

<!-- transient; owned by the checkpoint skill -->

- **Resume here:** merge #44 and #45, run wrap-up for each, then do
  #21 with the ruleset recipe in Next up, and start #23 and #34 as
  parallel sessions from `main`. The next Scorecard run on `main` (weekly,
  or on the merge push) should show Security-Policy and Fuzzing at 10.
- Private vulnerability reporting was enabled on the repo on 2026-10-30;
  vulnerability alerts, Dependabot security updates and auto-merge are
  still off (#21). Until #21 lands, docs carve-out PRs merge by hand with
  `--squash --delete-branch`.
- Fuzzing needs a nightly at or above the workspace's `rust-version`
  1.95: `rustup toolchain install nightly`, then
  `cargo +nightly fuzz run <target>` from `fuzz/`; the older dated
  nightlies on this machine fail the version check. cargo-fuzz is
  installed locally.
- Signed-Releases caveat for #25: Scorecard looks for `*.intoto.jsonl` or
  `*.sigstore.json` release assets, while GitHub attestations live in the
  attestations API, so #25 may lift Pinned-Dependencies without lifting
  Signed-Releases.
- `sbtctl daemon start` cannot start a daemon for
  `--daemon-url http://localhost:<port>`: `bind_from_url` hands
  `localhost:<port>` to `--bind`, which only parses IP literals. Found
  while writing #18's CLI test; not filed yet.
- Deliberately not filed: a gitleaks CI job (secret scanning and push
  protection are on), and the local pre-commit scrub for names that must
  never appear in the repo; its word list stays outside the repo.
- cargo-dist 0.32 keys verified for #25: `cargo-auditable`,
  `cargo-cyclonedx`, `github-attestations` (experimental),
  `github-action-commits` (survives `dist generate`). No macOS signing key
  exists, so #27 would be a custom codesign plus notarytool job.

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
