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
the `security` label (#18 to #27 and #34, plus #36, #40, #42 and #43 for
coverage, Scorecard, the security policy and fuzzing), each its own PR
from `main`, never stacked, worked in parallel sessions, one per issue.
Landed: #18 (Host/Origin guard), #19 (`active.json` validated before it is
trusted), #20 (private state dir, token redacted), #22 (read-only token,
SHA-pinned actions, `--locked`), #24 (Dependabot), #36 (coverage to
Codecov), #40 (Scorecard), #42 (SECURITY.md), #43 (cargo-fuzz targets),
#34 (CodeQL, check `analyze (rust)`), #25 (cargo-auditable, CycloneDX
SBOM, GitHub attestations, release.yml pinned through
`[dist.github-action-commits]`) and #21 (ruleset `protect main`,
vulnerability alerts, security updates, auto-merge; applied by hand). The
rubric all of it amounts to is `.claude/rules/REPO_HARDENING.md`,
installed from the global rules layer. Open: #23 (cargo-deny), #26
(bearer-token design dialog), #27 (signing decision).

## Branches in flight

| Branch | Purpose | Status |
|---|---|---|
| `main` | trunk | at `ffc45c0` (#48 merged); `v0.1.1` is the last release |
| `release/v0.1.2` | version bump; the release that proves #25 | PR #49, awaiting CI and merge; tag after |
| `docs/state-hardening` | this refresh, REPO_HARDENING.md and its CLAUDE.md row | docs carve-out PR |

## Next up

1. **Finish the hardening batch.** Merge PR #49, tag `v0.1.2`, and check
   the release carries `sbt-server.cdx.xml` and that
   `gh attestation verify <archive> --owner scottweaver` passes; that is
   what can lift Scorecard's Signed-Releases (caveat in Session handoff).
   #23 (cargo-deny) is one PR adding a job to ci.yml and a `deny.toml`;
   afterwards add its check name to ruleset `protect main` (id
   26239811), which requires `check (ubuntu-latest)`,
   `check (macos-latest)` and `analyze (rust)`, so every PR branch must
   contain `codeql.yml` (a branch cut before #47 needs `main` merged in).
   #26 is a design dialog and structural per ARCHITECTURE.md; #27 stays
   open until a team reports an endpoint policy block. Scorecard scored
   `main` 5/10 on 2026-10-30 before #34, #25 and #21 landed; expected
   after the next run: SAST, Branch-Protection and Pinned-Dependencies
   up; Token-Permissions still 0 (cargo-dist writes top-level
   `contents: write` with no knob in 0.32); Code-Review, Maintained,
   Contributors and CII-Best-Practices accepted as is; Packaging
   unscored.
2. **Releasing** is always: bump the workspace version in a PR, merge,
   push a `v*` tag, and the workflow does the rest; then
   `brew upgrade sbt-server`. The `HOMEBREW_TAP_TOKEN` secret is a
   fine-grained PAT scoped to the tap with Contents read/write; when it
   expires only the formula push fails.
3. **Socialising it**: `docs/agents.md` is the page to hand to a team.
   #25 now produces the SBOM and provenance a security review asks for;
   #23 adds the license report and #27 a signature. REPO_HARDENING.md is
   the rubric to show a reviewer.
4. **Still open from the hand-off**: auth on sbtd beyond loopback (now
   #26), Linux as a supported platform, sbt 2.x (only 1.x's protocol has
   been exercised), a launchd recipe for running sbtd at login. Each is a
   design dialog plus an ARCHITECTURE.md change, not a quick fix.
5. **Daily use will surface the rest**: the cold-start boot time is not in
   the `[success]` summary; `sbtctl` does not render diagnostics; the
   reaper interval is a one-minute constant.

## Most recent meaningful progress

- **2026-10-30 — #34, #25 and #21 landed (PRs #47, #48; #21 by hand);
  v0.1.2 bump open (PR #49); REPO_HARDENING.md extracted.** Why: the
  three Scorecard gaps that were work rather than structure. CodeQL
  (advanced setup, `security-and-quality`, build mode none) found nothing
  on the current tree; #25 turns on cargo-auditable, CycloneDX and GitHub
  attestations in `dist-workspace.toml` and pins every action in the
  generated release.yml through `[dist.github-action-commits]`; #21 is
  ruleset `protect main` plus vulnerability alerts, security updates and
  auto-merge. The rubric those issues add up to now lives in the global
  rules layer (`my-dev-environment` PR #32) and is installed here.
  Lessons: the ruleset requires `analyze (rust)` by name, so a PR branch
  without `codeql.yml` can never merge; the auto-mode classifier refuses
  to merge PRs, so merges are the user's. Risk: nothing proves the
  attestations until `v0.1.2` runs; #47 and #48 merged on green CI
  without a second reader.
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

## Blocked / waiting

- The `v0.1.2` tag waits on PR #49 merging and the user's go; the release
  workflow is the only test of #25.

## Session handoff

<!-- transient; owned by the checkpoint skill -->

- **Resume here:** merge PR #49, tag `v0.1.2` on its merge commit, watch
  the release workflow (first run of the attest and cyclonedx jobs),
  verify with `gh attestation verify` and `brew upgrade sbt-server`; the
  push to `main` reruns Scorecard, read the new score. Then #23 in its own
  session from `main`, and add its check to the ruleset.
- Ruleset `protect main` (id 26239811) is active since 2026-10-30; docs
  carve-out PRs now merge with `gh pr merge --auto --squash
  --delete-branch`, first exercised by this refresh's own PR.
- Merged branches still on the remote because their PRs were merged from
  the web UI: `ci/codeql`, `ci/scorecard`,
  `claude/release-provenance-db862a`, `docs/security-policy`,
  `test/fuzz-parsers`, `dependabot/github_actions/actions/checkout-7.0.1`.
  Delete once the user confirms.
- Fuzzing needs a nightly at or above the workspace's `rust-version`
  1.95: `rustup toolchain install nightly`, then
  `cargo +nightly fuzz run <target>` from `fuzz/`; the older dated
  nightlies on this machine fail the version check. cargo-fuzz is
  installed locally.
- Signed-Releases caveat: Scorecard looks for `*.intoto.jsonl` or
  `*.sigstore.json` release assets, while GitHub attestations live in the
  attestations API, so `v0.1.2` may lift Pinned-Dependencies without
  lifting Signed-Releases. If so, the fix is an attestation bundle
  attached as a release asset, a cargo-dist question.
- `sbtctl daemon start` cannot start a daemon for
  `--daemon-url http://localhost:<port>`: `bind_from_url` hands
  `localhost:<port>` to `--bind`, which only parses IP literals. Found
  while writing #18's CLI test; not filed yet.
- Deliberately not filed: a gitleaks CI job (secret scanning and push
  protection are on), and the local pre-commit scrub for names that must
  never appear in the repo; its word list stays outside the repo.
- No macOS signing key exists, so #27 would be a custom codesign plus
  notarytool job.

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
