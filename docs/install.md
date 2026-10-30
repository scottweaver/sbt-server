# Install

## Requirements

- **macOS**, Apple silicon or Intel. A Linux x86_64 archive ships with each
  release and the test suite passes on Linux, but macOS is the supported
  platform for now. Windows is out of scope.
- **A JVM and `sbt` on your `PATH`.** sbt-server launches the `sbt` command
  you already use; it does not bundle one. sdkman, Coursier and Homebrew's
  `sbt` all work. Developed and verified against sbt 1.11.7 on Java 21;
  the sbt launcher runs whatever version each project pins in
  `project/build.properties`, and only the 1.x server protocol has been
  exercised.

## Homebrew (recommended)

```bash
brew install scottweaver/tap/sbt-server
```

That installs `sbtd` and `sbtctl` together under Homebrew's prefix
(`/opt/homebrew/bin` on Apple silicon). Upgrade with
`brew upgrade sbt-server`.

## Installer script

For machines without Homebrew. It detects your platform, fetches the
matching archive from the latest GitHub release, verifies it, and installs
both binaries to `$CARGO_HOME/bin` (usually `~/.cargo/bin`), adding that
directory to your shell profile if it is not already on `PATH`:

```bash
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/scottweaver/sbt-server/releases/latest/download/sbt-server-installer.sh | sh
```

Set `SBT_SERVER_NO_MODIFY_PATH=1` in the environment to leave your shell
profile alone, or `SBT_SERVER_INSTALL_DIR=/some/dir` to install elsewhere.
Every release page also carries the archives and SHA-256 sums if you would
rather download by hand.

## Verify a download

Every release since the one after v0.1.1 ships with provenance. Each
platform archive carries a GitHub artifact attestation that ties it to this
repository's release workflow at the tagged commit; with the GitHub CLI:

```bash
gh attestation verify sbt-server-aarch64-apple-darwin.tar.xz --owner scottweaver
```

The binaries embed their dependency list (built with cargo-auditable), so
an installed copy can be checked against the RustSec advisory database
without the source tree, by `cargo-audit` installed with its
`binary-scanning` feature:

```bash
cargo audit bin $(which sbtctl) $(which sbtd)
```

Every asset also has a Sigstore signature beside it, `<asset>.sigstore.json`,
made keyless by the repository's `sign.yml` workflow right after the release
is published. The signing job runs from `main`, which is why the identity
below names `refs/heads/main` rather than the tag. It needs only
[cosign](https://github.com/sigstore/cosign):

```bash
cosign verify-blob --bundle sbt-server-aarch64-apple-darwin.tar.xz.sigstore.json --certificate-identity https://github.com/scottweaver/sbt-server/.github/workflows/sign.yml@refs/heads/main --certificate-oidc-issuer https://token.actions.githubusercontent.com sbt-server-aarch64-apple-darwin.tar.xz
```

The same workflow exports the build provenance beside each archive and
checksum as `<asset>.intoto.jsonl`, the SLSA statement from the attestation
above in a file, so it can be verified offline:

```bash
gh attestation verify sbt-server-aarch64-apple-darwin.tar.xz --bundle sbt-server-aarch64-apple-darwin.tar.xz.intoto.jsonl --repo scottweaver/sbt-server
```

Each release page also carries a CycloneDX SBOM, `sbt-server.cdx.xml`, for
security intake forms. The attestation covers the archives and their
checksums; the Sigstore bundles cover every asset, the installer script and
the SBOM included. The binaries are not code-signed or notarized: that needs
a paid Apple developer membership, and the installer, Homebrew and `curl`
paths set no quarantine flag, so Gatekeeper does not prompt.

## From source

With a Rust toolchain (1.95 or newer):

```bash
cargo install --git https://github.com/scottweaver/sbt-server sbt-server
```

or, from a clone, `cargo install --path crates/sbt-server`. Both binaries
come from the `sbt-server` package, so one command installs both.

Whichever method you use, keep `sbtd` and `sbtctl` next to each other:
`sbtctl daemon start` looks for `sbtd` beside its own executable first, and
only then on `PATH`. Do not mix methods; two copies on `PATH` is the usual
cause of a puzzling version mismatch.

## What gets written where

| Path | What |
|---|---|
| `~/.config/sbt-server/` (or `$XDG_CONFIG_HOME/sbt-server/`, or `--state-dir`) | `sbtd.pid`, `sbtd.addr` (the address the daemon bound) and `sbtd.log` (its output when started detached, including raw build output); the directory is created 0700 and the files 0600, and looser modes left by an older version are tightened on start |
| `<project>/project/target/active.json` | written by sbt itself, as for any sbt server; sbt-server reads and, on takeover, deletes it |
| `~/.sbt/1.0/server/` | sbt's own socket and token files, untouched by sbt-server |

Nothing is written into your project beyond what sbt writes on its own.

## Uninstall

```bash
sbtctl daemon stop
brew uninstall sbt-server
brew untap scottweaver/tap
rm -rf ~/.config/sbt-server
```

For the installer script or `cargo install`, delete `sbtd` and `sbtctl`
from the directory they were installed to instead of the `brew` lines.
