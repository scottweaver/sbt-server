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
`[dist.github-action-commits]`), #21 (ruleset `protect main`,
vulnerability alerts, security updates, auto-merge; applied by hand) and
#52 (Sigstore bundles beside every release asset, `sign.yml`). #27 is
closed as not planned: no paid Apple membership, so no code signing.
`v0.1.2` shipped on 2026-10-30 with attestations, a CycloneDX SBOM, 13
`.sigstore.json` bundles and, after #57, six `.intoto.jsonl` provenance
files, all verified locally; the unsigned v0.1.0 and v0.1.1 releases were
deleted (tags kept), so v0.1.2 is the only release. The rubric all of it
amounts to is `.claude/rules/REPO_HARDENING.md`, installed from the global
rules layer. #23 (cargo-deny, PR #60) and #26 (bearer-token auth, PR #62)
landed on 2026-10-30; no issue is open. Every route but `/health` now
requires the token `sbtd` mints at start (`sbtd.token` in the state dir),
which `sbtctl` reads on its own; this is the one contract change since
v0.1.2. The next milestone is `v1.0.0`, cut once the Scorecard is at a
reasonable grade; 1.0 freezes the HTTP contract and the CLI surface.

## Branches in flight

| Branch | Purpose | Status |
|---|---|---|
| `main` | trunk | at `f136e9f` (#62 merged); `v0.1.2` is the only release |
| `docs/state-auth-landed` | this refresh | docs carve-out PR |

## Next up

1. **The road to v1.0.0.** Scorecard is 6.6 on `6129c46` (before #62):
   Signed-Releases 10, CII-Best-Practices 5 (bestpractices.dev/projects/
   16104, "passing" since 2026-10-30, Baseline Level 1 at 100%), SAST 7
   climbing by itself to 10 once all of the last 30 merged PRs carry a
   CodeQL check run (about 25 more merges; #48 never counts),
   Pinned-Dependencies 7 (cargo-dist's three `curl | sh` lines, no knob),
   Branch-Protection 3 (the user chose not to turn on strict up-to-date
   checks for the 4th point); Token-Permissions, Code-Review,
   Contributors and Maintained are structural. The hardening is done;
   what remains before 1.0.0 is the freeze itself: fix or document the
   `localhost` daemon-URL bug and the unrendered diagnostics (below), try
   the Intel macOS and Linux archives by hand (only Apple Silicon has
   ever been run), write the 1.0.0 CHANGELOG section (the bearer token
   is the one breaking change since 0.1.2: direct API callers must send
   `Authorization: Bearer`) and a stability statement in the README, pass
   over `docs/agents.md`, then bump and tag. Coverage policy: Codecov's
   project status fails below 90% (`codecov.yml`, decided 2026-10-30);
   `main` is at 90.5%.
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

- **2026-10-30 — #26 bearer-token auth landed (PR #62); #23 cargo-deny
  (PR #60) and the Best Practices badge (PR #61) landed; no open
  issues.** Why: loopback plus Host/Origin admitted every local user,
  and 1.0 freezes the contract, so the auth decision had to come first.
  The design dialog settled: `/health` open, a new token per daemon start
  (`sbtd.token`, 0600, removed on exit), file only, no env override.
  `sbtctl` reads it from the state dir, so agents change nothing; direct
  API callers add one header. The ruleset now also requires the `deny`
  check. Codecov's project status gets a hard 90% floor (`codecov.yml`)
  after it went red on a 0.2-point dip. Lessons: a reusable-workflow job
  cannot hold more permissions than its caller grants, which is why
  signing could not live inside cargo-dist's run; the badge site's
  proposal URLs refuse about 8 KB. Risk: the smoke script and the
  integration tests cover the token path, but no one has run the
  released binaries with it yet; v1.0.0 will.
- **2026-10-30 — v0.1.2 released with provenance and Sigstore bundles;
  #51, #52 landed (PRs #51, #53); #27 closed; Scorecard re-reviewed.**
  Why: the release proves #25, and Signed-Releases needs assets, not the
  attestations API. The first tag hit GitHub's `macos-14` brownout (the
  image retires 2026-11-02; cargo-dist 0.32 still picked it), fixed by
  `[dist.github-custom-runners]` and a moved tag; `sign.yml` runs on
  `workflow_run` because a `GITHUB_TOKEN`-created release fires no
  `release:` event, and its certificate identity is `@refs/heads/main`,
  which the first draft of the docs got wrong. Four review agents fixed
  the ceilings now recorded in Next up and in REPO_HARDENING.md. Lessons:
  `gh pr merge --auto --delete-branch` does not delete the remote branch
  (ten merged branches linger; the repo setting "automatically delete
  head branches" would); `brew upgrade` needs `brew update` first to see
  a new formula. Risk: the Dependabot token may not let CodeQL upload,
  leaving SAST gaps; nothing verified the Linux or Intel archives beyond
  their checksums. Later the same day: #57 (PR #58) exports the
  attestation as `<asset>.intoto.jsonl` beside each archive and checksum
  (Scorecard scores a signature asset 8 and a provenance asset 10,
  averaged over the last five releases, and never reads the attestations
  API), `sign.yml` gained `workflow_dispatch` for an existing tag, and the
  unsigned v0.1.0 and v0.1.1 releases were deleted at the user's request
  so the average covers v0.1.2 alone. Score 6.3.
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

## Blocked / waiting

- *(nothing)*

## Session handoff

<!-- transient; owned by the checkpoint skill -->

- **Resume here:** the hardening workstream is closed. Next is the
  1.0.0 freeze list in Next up, each item a small PR from `main`: file
  the `localhost` daemon-URL bug and fix it, decide and document the
  diagnostics rendering, run the Intel macOS and Linux v0.1.2 archives
  by hand, write the 1.0.0 CHANGELOG section and README stability note,
  then bump to 1.0.0 and tag. Read the Scorecard after this PR's push
  (expected about 6.6; SAST climbs with each merge).
- Ruleset `protect main` (id 26239811) requires `check (ubuntu-latest)`,
  `check (macos-latest)`, `analyze (rust)` and `deny`; docs
  carve-out PRs merge with `gh pr merge --auto --squash --delete-branch`,
  but the remote branch survives (see below).
- Merged branches still on the remote: `ci/codeql`, `ci/scorecard`,
  `ci/sigstore-release-signing`, `claude/release-provenance-db862a`,
  `dependabot/github_actions/actions/checkout-7.0.1`,
  `docs/security-policy`, `docs/state-hardening`, `fix/macos-15-runner`,
  `test/fuzz-parsers`, plus whatever auto-merge left since. Delete once
  the user confirms, or turn on "automatically delete head branches".
- Releasing: `brew update` before `brew upgrade sbt-server`, or the tap
  index still shows the previous version. The release workflow is
  followed by `sign.yml` (about a minute); the bundles are not there
  until it finishes.
- Fuzzing needs a nightly at or above the workspace's `rust-version`
  1.95: `rustup toolchain install nightly`, then
  `cargo +nightly fuzz run <target>` from `fuzz/`; the older dated
  nightlies on this machine fail the version check. cargo-fuzz is
  installed locally.
- Scorecard's Signed-Releases reads the `.sigstore.json` assets; the
  attestations API is invisible to it. Verify a download with the
  command in `docs/install.md` (identity `sign.yml@refs/heads/main`).
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
