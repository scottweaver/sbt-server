# Contributing to sbt-server

sbt-server is a small Rust workspace with one maintainer. Contributions are
welcome; this page says how to get one in without friction.

## Reporting a bug

Open an issue at <https://github.com/scottweaver/sbt-server/issues> with:

- the version (`sbtctl --version`), your platform, and the sbt version the
  project pins in `project/build.properties`;
- the exact `sbtctl` command, what happened, and what you expected;
- the relevant lines of `~/.config/sbt-server/sbtd.log` (the daemon's own
  output, including raw build output).

[docs/troubleshooting.md](docs/troubleshooting.md) explains what each error
means and may answer the question outright.

**Security problems do not go on the issue tracker.** `sbtd` runs arbitrary
code through sbt's build definition, so anything that lets a request reach
it from somewhere other than the local machine, or makes it trust a file it
should not, is reported privately as [SECURITY.md](SECURITY.md) describes.

## Proposing a change

1. **Open an issue first** for anything beyond a typo, with *Why*, *Change*
   and *Done when* sections as the existing issues have. The issue is the
   spec, and it saves writing code for a change that will not land. A change
   to anything `.claude/rules/ARCHITECTURE.md` records as a constraint is a
   design discussion before it is code.
2. **Branch from `main`**, one branch per issue. Do not stack a branch on
   another open branch: GitHub closes a pull request whose base branch is
   deleted.
3. **Open a pull request** against `main` with `Closes #<n>` in the body.
   Nobody pushes to `main` directly, the maintainer included: a ruleset
   requires a pull request and three green checks, `check (ubuntu-latest)`
   and `check (macos-latest)` (format, clippy, tests) and `analyze (rust)`
   (CodeQL).
4. **Keep the PR to its issue.** When a change alters what a document under
   `.claude/rules/` describes, the document update rides in the same PR.

## Building and testing

Rust 1.95 is pinned in `rust-toolchain.toml` and `rustup` picks it up. No
JVM is needed for the default suite: the tests drive sbt's protocol against
`crates/fake-sbt`, a stand-in that plays frames frozen from real sbt 1.11.7
sessions.

```bash
cargo build
cargo test --workspace
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo deny check
```

These are what CI runs (with `--locked`), so a PR that passes them locally
passes CI. The last needs cargo-deny (`brew install cargo-deny` or
`cargo install --locked cargo-deny`); it checks the dependency tree against
the RustSec advisories and the license, duplicate and source rules in
`deny.toml`.

Three more exercise a real sbt or need extra tooling, and are run by hand:

```bash
# The two #[ignore] live tests, against tests/fixtures/hello-sbt (needs sbt and a JVM on PATH)
SBT_SERVER_LIVE=1 cargo test -p sbt-server-core --test live -- --ignored --nocapture

# End to end: build, start sbtd on a spare port, cold and warm compile, shutdown
scripts/smoke.sh

# Fuzz one parser (needs a nightly toolchain at or above 1.95, and cargo-fuzz)
cd fuzz && cargo +nightly fuzz run framing
```

The fuzz targets are `host_header`, `framing`, `message`, `descriptor` and
`lines`, one per parser at a trust boundary; a crash becomes a regression
test next to the parser.

## Test policy

A change in behaviour comes with a test for the new behaviour, and a bug
fix comes with the test that failed before the fix. Anything sbt itself
does is tested through `fake-sbt`, so a fix for sbt's behaviour starts with
a frame captured from a real session in `crates/fake-sbt/fixtures/`.

## Code conventions

[`.claude/rules/RUST_BEST_PRACTICES.md`](.claude/rules/RUST_BEST_PRACTICES.md)
is the authority: types first, illegal states unrepresentable, exhaustive
matches, errors as values, no `unwrap` outside tests. `cargo fmt` and
clippy's `pedantic` group settle the rest.

## Releasing (maintainer)

1. In one PR, move the *Unreleased* section of `CHANGELOG.md` under the new
   version with today's date and bump `version` in the workspace
   `Cargo.toml`.
2. Merge it, tag the merge commit `vX.Y.Z` and push the tag. The release
   workflow builds the archives, the installer, the SBOM, the attestations
   and the Homebrew formula.
3. Verify a download as [docs/install.md](docs/install.md) describes, then
   `brew upgrade sbt-server`.
