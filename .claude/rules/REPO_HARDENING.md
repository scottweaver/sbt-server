# Repository hardening

What a security review, a corporate intake form, or a careful adopter
asks of an open-source repository, and how each answer is wired in so
that it is a CI gate or a repository setting rather than a promise.
METHODOLOGIES.md says how work flows; this file says what the
repository must keep proving about itself. Every item below is either a
gate (a PR cannot merge without it) or a setting (checked at wrap-up).

Extracted from the sbt-server hardening of 2026-10-30 (issues #18 to
#27, #34, #36, #41, #44, #45 there), where the OpenSSF Scorecard went
from unmeasured to every fixable check green. The Rust column is what
that repository runs; the other columns are the nearest equivalents and
have not been exercised in a live repository, so verify before copying.

## How to use this file

- **A new repository** works the rubric top to bottom. Each row is one
  GitHub issue carrying *Why / Change / Done when* (the issue is the
  spec), one PR from the default branch, never stacked. Rows can run in
  parallel sessions except where "Order of adoption" says otherwise.
- **An existing repository** starts by reading its Scorecard
  (`https://api.securityscorecards.dev/projects/github.com/<owner>/<repo>`
  once the Scorecard workflow has published; the raw report names the
  file and line behind every warning) and maps each failing check to a
  row here. Rows the score cannot see (threat model, fuzzing scope,
  release signing) are checked by hand.
- **At wrap-up**, a merged PR that touches anything in "Audit triggers"
  re-checks its row: the gate still runs, the setting is still on, the
  pin is still a SHA.

## The rubric

### 1. Build gates

Every PR, on every supported operating system, with the lockfile
honoured (`--locked` or the ecosystem's equivalent) and the toolchain
pinned by a file in the repository, not by whatever the runner has.

| Gate | Rust | TypeScript | Scala |
|---|---|---|---|
| Format | `cargo fmt --all --check` | prettier `--check` | `scalafmtCheckAll` |
| Lint, warnings are errors | `cargo clippy --locked --workspace --all-targets -- -D warnings`, `pedantic` on at workspace level | eslint, `--max-warnings 0` | `-Xfatal-warnings` (`-Werror`) |
| Tests | `cargo test --locked --workspace` | the test runner | `test` |
| Toolchain pin | `rust-toolchain.toml` plus `rust-version` in `Cargo.toml` | `.nvmrc` or `packageManager` | `project/build.properties` |
| Unsafe code | `[workspace.lints.rust] unsafe_code = "forbid"` | n/a | n/a |

The lint and format rules themselves live in the language's
best-practices file; this row only says they are gates.

### 2. Workflow hygiene

The workflows are code that runs with a token, so they are reviewed as
code.

- **Least-privilege tokens.** Top-level `permissions: contents: read`
  in every workflow (or `read-all`); a job that must write (SARIF
  upload, release assets, attestations) declares exactly that
  permission at job level. The repository's default workflow token is
  set to read-only in Settings so a workflow that forgets inherits
  nothing.
- **Every action is pinned to a full commit SHA** with the version as a
  trailing comment (`uses: actions/checkout@<sha> # v4.4.0`), so the
  update tool can read the version and a retagged action cannot change
  what runs. A **generated workflow** (cargo-dist's `release.yml`, for
  one) is never hand-edited, because its generator's check fails on
  drift; its pins go through the generator's own table
  (`[dist.github-action-commits]`) and an update-tool PR that edits the
  generated file directly is closed and the bump made in the table.
- **No dangerous patterns.** No `pull_request_target` that checks out
  the PR head, no `${{ github.event.* }}` text interpolated into a
  `run:` line. Scorecard's Dangerous-Workflow check is the gate.
- `persist-credentials: false` on checkout in any workflow that does
  not push.

### 3. Dependencies

- **An update tool** files PRs for the package manager and for GitHub
  Actions on a weekly schedule, with minor and patch bumps grouped so
  one PR carries them. Dependabot for cargo and npm; Scala Steward for
  sbt, which Dependabot does not read. A `dependencies` label exists
  for its PRs.
- **Vulnerability alerts and automated security updates** are enabled
  in repository settings (both are off by default on a new repository;
  `gh api -X PUT repos/<o>/<r>/vulnerability-alerts` and
  `.../automated-security-fixes`).
- **A policy gate in CI** fails a PR on a known advisory, a license
  outside the allowlist, a wildcard version, or a source outside the
  registry: `cargo-deny` with a `deny.toml` (advisories, licenses,
  bans, sources) for Rust; `npm audit --audit-level=high` or
  `osv-scanner` for TypeScript; the OWASP `sbt-dependency-check` plugin
  or GitHub's `sbt-dependency-submission` action for Scala. The license
  allowlist is derived from the real tree, not guessed, and every
  `ignore` entry carries a written reason. A new advisory in an
  unrelated crate failing every PR is the intended behaviour; the fix
  is a bump or a reasoned ignore, never turning the gate off.

### 4. Static analysis

- **CodeQL, advanced setup** (the workflow file is in the repository,
  SHA-pinned like every other action; the default-setup toggle is not,
  and cannot be reviewed): the `security-and-quality` suite, on pull
  requests, pushes to the default branch and a weekly schedule, with
  `contents: read`, `security-events: write`, `actions: read`. Rust uses
  build mode `none`; TypeScript is `javascript-typescript`. CodeQL has no
  Scala analyzer, so Scala repositories use Semgrep as the SAST
  analogue.
- **Findings are triaged**, not accumulated: each alert is fixed or
  dismissed with a reason in the Security tab. An open alert older than
  the current sprint is a wrap-up finding.
- The check's name (`analyze (rust)` in a one-language repository) is
  recorded so the ruleset in section 6 can require it.

### 5. Beyond unit tests

- **Coverage is measured on every PR and reported**, not just
  computed: `cargo-llvm-cov` to lcov, uploaded to Codecov with OIDC
  (`use_oidc: true`, so no upload token exists to leak), test doubles
  excluded from the denominator (`--ignore-filename-regex`), the badge
  in the README. TypeScript: the runner's `--coverage` (c8 or istanbul)
  to lcov, same upload. Scala: `sbt-scoverage`. The number is a trend to
  watch, not a gate; a hard threshold invites tests that assert
  nothing.
- **Every parser at a trust boundary has a fuzz target.** Anything that
  turns bytes from outside the process into a typed value (wire frames,
  a descriptor file another program writes, an HTTP header, a line
  assembler) gets a target under `fuzz/fuzz_targets/` driven by
  `cargo-fuzz`; a pure parser needs no harness beyond feeding it the
  bytes. Scorecard's Fuzzing check detects the directory. Targets run
  locally (`cargo +nightly fuzz run <target>`) and, where the budget
  allows, as a nightly job; a crash becomes a regression test next to
  the parser. JVM code can use Jazzer; property tests (ScalaCheck,
  fast-check, proptest) are the nearest substitute where fuzzing does
  not fit.

### 6. Branch and repository settings

Settings have no PR, so they are applied by hand, recorded in one
issue (its body carries the exact commands), and closed by hand.

- **A ruleset on the default branch**: a pull request is required,
  the status checks are required *by name* (the build matrix jobs, the
  CodeQL analysis, the policy gate), force pushes and deletion are
  blocked, and no bypass actors are listed, administrators included.
  Review count is 0 on a one-maintainer repository, where nobody can
  approve their own PR; Scorecard's Code-Review check stays low there
  by construction and is not a task.
- **Secret scanning and push protection** on. (A separate secret-scan
  CI job is not needed when they are.)
- **Private vulnerability reporting** on, so SECURITY.md can point at
  it.
- **Auto-merge** on, so docs-only PRs under the carve-out in
  METHODOLOGIES.md can merge with `gh pr merge --auto`.

### 7. Policy files and the front page

- **`SECURITY.md`**: what is in scope, what is explicitly not, where
  to report privately, and the response timeline. Written in the
  project's own threat model, not a template; a local daemon that runs
  a build tool is "code execution from anything that can reach it",
  and the file should say so.
- **`LICENSE`**: one OSI license for the whole repository, declared in
  the package manifest too. Apache-2.0 by default; it is the permissive
  license corporate intake accepts most readily.
- **README badges** in this order: CI, release, coverage, license,
  OpenSSF Scorecard. A badge that reads "unknown" is a defect.
- **`CONTRIBUTING.md`** once a second contributor exists, and a
  **`CODEOWNERS`** file once a second maintainer does; before that they
  are decoration.

### 8. Releases

- **Tag-driven, one version**: a `v*` tag on the default branch runs
  the release workflow and nothing else does; the version is bumped in
  its own PR first. For Rust, cargo-dist generates the workflow, builds
  per target, attaches archives and checksums, and publishes installers
  (a `curl | sh` script, a Homebrew formula to a tap).
- **The binaries are auditable**: built with `cargo-auditable`, so
  `cargo audit bin` can check an installed copy against the advisory
  database without the source tree.
- **An SBOM is attached** to every release (CycloneDX,
  `cargo-cyclonedx = true` in cargo-dist). Intake forms ask for it by
  name.
- **Provenance is attested**: GitHub artifact attestations
  (`github-attestations = true` in cargo-dist; `actions/attest` is one
  more pinned action) tie each archive to the workflow and commit that
  built it, verifiable with `gh attestation verify <archive> --owner
  <o>`. npm has `npm publish --provenance`; Maven Central requires GPG
  signatures through `sbt-ci-release`. Scorecard's Signed-Releases
  check reads the latest releases only, so the score moves on the first
  release after the change.
- **Signatures are release assets too.** Scorecard's Signed-Releases
  reads only assets named `*.sigstore.json`, `*.sig`, `*.minisig`,
  `*.asc`, `*.intoto.jsonl` on the last five releases; it never reads the
  attestations API. So a second workflow signs each asset keyless with
  `cosign sign-blob --bundle` and uploads `<asset>.sigstore.json` beside
  it. It is triggered by `workflow_run` of the release workflow, because
  a release created with `GITHUB_TOKEN` never fires a `release:` event,
  and a generator's custom jobs rarely hold both `id-token: write` and
  `contents: write`. A `workflow_run` job runs from the default branch,
  so the certificate identity users verify against is
  `.../sign.yml@refs/heads/main`, not the tag; the verify command in the
  install docs pins that exact identity and the GitHub OIDC issuer
  (proven on sbt-server v0.1.2, 2026-10-30).
- **Code signing and notarization** (macOS) stay a decision, not a
  default: do it when a managed endpoint actually blocks the binary,
  because it needs a developer certificate and a secret in CI.
- A secret the release needs (a tap token, a signing key) is a
  fine-grained token scoped to the one target, and its expiry is
  written down where the next release will look.

### 9. The yardstick: OpenSSF Scorecard

- The `ossf/scorecard-action` workflow runs on pushes to the default
  branch and weekly, with `publish_results: true` (the public API and
  badge), uploading its SARIF to code scanning so warnings appear next
  to CodeQL's. `permissions: read-all` at top level, `security-events:
  write` and `id-token: write` on the job.
- **Checks that are capped by structure, not by work**, so they are
  read, not chased (each ceiling measured on sbt-server, 2026-10-30,
  Scorecard v5.6):
  - Code-Review and Contributors on a one-person project; Maintained
    for a repository under 90 days old; Packaging (`-1`, unscored) when
    the release is a GitHub Release rather than a registry publish.
  - Branch-Protection: tier 1 (no force push, no deletion) is 3; tier 2
    weighs a reviewer count double and requiring PRs, up-to-date
    branches and last-push approval once each, and the later tiers do
    not count until tier 2 is complete. Without a second human the
    ceiling is 4 (strict up-to-date checks, with the merge-main churn
    that implies). One required approval plus an admin bypass reads 8,
    and makes every merge a bypass; do not.
  - SAST: 7 the day CodeQL lands, then 10 only once all of the last 30
    merged PRs carry a CodeQL check run on their head commit. It climbs
    by itself; a PR opened before CodeQL existed never counts.
  - Pinned-Dependencies: every `curl | sh` a release generator emits
    (its own installer, rustup, an SBOM tool) counts as unpinned unless
    the URL is `raw.githubusercontent.com` at a commit SHA; a checksum
    check does not count. With cargo-dist the ceiling is 7, and dropping
    the SBOM to gain a point is the wrong trade.
  - Token-Permissions: a generated release workflow with top-level
    `contents: write` and no knob to narrow it reads 0.
  - CII-Best-Practices: 0 until the project is registered at
    bestpractices.dev; registered is 2, passing 5, silver 7, gold 10.
    Passing needs a CONTRIBUTING file (how to report, contribute, run
    the tests, the test policy) and human-written release notes or a
    CHANGELOG; the rest is form answers, about an hour.
  Every other check is expected at 10. A repository with those at 10
  and the structural ones explained is a top-tier repository by this
  rubric; on a solo repository released by a generator that is an
  overall score near 7.5, not 10.

### 10. The code itself

The rubric does not restate ARCHITECTURE.md, but it insists the threat
model is written down there and in SECURITY.md: what listens on what,
who is trusted (the loopback rule, the Host/Origin check, a private
state directory with private files), which inputs are untrusted even
though they come from disk (a descriptor file in a cloned repository,
a lockfile, a config another program writes), and which secrets exist
and how they are kept out of logs. Each of those is a constraint with a
date and a test, or it is not a constraint.

## Order of adoption

Learned the hard way; the sequence matters.

1. **Pins and token permissions first** (section 2), so every workflow
   added afterwards is born under the convention.
2. **The update tool next** (section 3), so the pins stay current.
3. **Scanners and gates in parallel** (sections 3 to 5, 9): each is its
   own workflow file or job, so the PRs do not conflict; they do all
   edit the README badges and ARCHITECTURE.md, so expect a merge from
   the default branch before the second one lands.
4. **The ruleset last** (section 6), because it requires checks by
   name and a check that does not yet exist on the default branch
   blocks every open PR that lacks it.
5. **Provenance rides the next release** (section 8); nothing proves it
   until a tag runs.

## Not adopted by default

Listed so nobody re-proposes them without a new reason.

- A gitleaks CI job: GitHub secret scanning plus push protection
  already cover it.
- `cargo-vet` or `cargo-crev` review trails: heavier than a small
  project's dependency tree warrants.
- Mutation testing (`cargo-mutants`): useful, slow, and a judgement
  call per project.
- A coverage threshold gate: see section 5.

## Audit triggers

A merged PR touching any of these re-checks its row at wrap-up:

- `.github/workflows/**`, `.github/dependabot.yml`, `CODEOWNERS`
- the release generator's config (`dist-workspace.toml`) and any
  generated workflow
- `deny.toml`, `fuzz/**`, coverage configuration
- `SECURITY.md`, `CONTRIBUTING.md`, `CHANGELOG.md`, `LICENSE`, the README
  badge block
- workspace-level lints in the package manifest (`[workspace.lints]`)
- repository settings (no file; the settings issue is the record)

## Maintenance

Update when a tool is swapped, a gate is added or deliberately
removed, or a "Not adopted" item is adopted; each change names the
repository and date it was proven in. Project status (which rows are
done here) lives in STATE.md, never in this file.
