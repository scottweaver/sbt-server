# Changelog

What changed in each release of sbt-server, for the people who run `sbtd`
and `sbtctl`. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/)
and versions follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html);
until 1.0.0 the HTTP and CLI contracts may still change in a minor release.

## [Unreleased]

## [1.0.0] - 2026-10-30

The contract freeze. From this release the HTTP API (its four routes, the
`/exec` event shapes and the error codes) and the `sbtctl` command line
(its flags and exit codes) change only with a new major version; a minor
release may add routes, event types, fields and flags, never remove or
reinterpret one. One change since 0.1.2 is breaking, and only for tools
that call the HTTP API directly: the bearer token below. `sbtctl` users
change nothing.

### Security

- Every HTTP route but `GET /health` now requires a bearer token. `sbtd`
  mints one at each start and writes it to `sbtd.token` (0600) in the state
  directory; `sbtctl` reads it from `--state-dir` / `SBTD_STATE_DIR` on its
  own, so agent call sites do not change. Tools that call the API directly
  must send `Authorization: Bearer <token>`; a missing or wrong token
  answers `401 unauthorized`. This keeps other local users on a shared
  machine from running sbt as you. (#26)
- The dependency tree is checked on every change against the RustSec
  advisory database, a license allowlist, wildcard versions and unknown
  sources with cargo-deny. (#23)

### Fixed

- `sbtctl daemon start` with a `localhost` daemon URL
  (`--daemon-url http://localhost:<port>`) now starts the daemon; it used to
  hand `localhost` to `sbtd --bind`, which takes an IP literal, and report
  that the daemon never became healthy. (#64)

### Verified

- The 0.1.2 Intel macOS archive runs under Rosetta and the 0.1.2 Linux
  x86_64 archive runs in an Ubuntu 24.04 container (daemon start, status
  and stop); until now only the Apple silicon build had been run by hand.
  Linux remains unsupported until it has a real user.

## [0.1.2] - 2026-10-30

A hardening release. Nothing about how `sbtctl` is used changes; what
changes is who can reach the daemon, what it trusts on disk, and what a
download can prove about itself.

### Security

- `sbtd` refuses any request whose `Host` header is not a loopback address
  or `localhost`, and any request carrying an `Origin` header, with
  `403 forbidden`. Binding to loopback alone did not keep browsers out: a web
  page could reach `/exec` through DNS rebinding, and sbt runs arbitrary code
  through a build definition. `sbtctl` is unaffected. (#18)
- `active.json`, the file sbt writes to describe a running server, is
  validated before anything trusts it: a TCP endpoint must be a loopback
  address and a `tokenfilePath` must lie under `~/.sbt`. A cloned repository
  that ships a crafted `project/target/active.json` can no longer make
  `sbtd` connect to another host or read an arbitrary file. (#19)
- The state directory (`~/.config/sbt-server`) is created `0700` and its
  pid, address and log files `0600`; looser modes left by an earlier version
  are tightened on the next start. The sbt server token no longer appears in
  trace-level logs. (#20)

### Added

- Provenance for every download: each release archive and checksum carries
  a GitHub artifact attestation (`gh attestation verify <archive> --owner
  scottweaver`), the binaries embed their dependency list so `cargo audit
  bin` works on an installed copy, and a CycloneDX SBOM
  (`sbt-server.cdx.xml`) is attached to the release. (#25)
- Sigstore signatures: every release asset has a `.sigstore.json` bundle
  beside it, verifiable with `cosign verify-blob` as `docs/install.md`
  describes. (#52)
- A security policy, [SECURITY.md](SECURITY.md), with private vulnerability
  reporting and response times. (#42)
- A Codecov coverage report and an OpenSSF Scorecard badge in the README.
  (#36, #40)

### Changed

- The repository's own supply chain is hardened: CI runs with a read-only
  token and SHA-pinned actions, builds with the lockfile honoured, scans the
  code with CodeQL on every pull request, gets dependency updates from
  Dependabot, and the parsers at every trust boundary (the `Host` header,
  JSON-RPC framing and messages, `active.json`, line assembly) have
  cargo-fuzz targets. The default branch requires a pull request and green
  checks from everyone. (#21, #22, #24, #34, #43)

## [0.1.1] - 2026-10-29

### Fixed

- `sbtctl start`, `sbtctl stop` and `sbtctl restart` typed without `daemon`
  are refused with a usage error (exit 2) that names `sbtctl daemon <verb>`,
  instead of handing the word to sbt and printing its "Not a valid command"
  parade. `sbtctl -- start` still sends the word to sbt. (#13)

### Changed

- Every error `sbtctl` shows carries a hint: an unreachable daemon points at
  `sbtctl daemon start`, a directory outside any project at running inside
  one or using `-C <dir>`, and a server that answers but is not `sbtd` is
  reported as such with a pointer at `--daemon-url`, never echoed raw.
  `sbtd` answers a malformed request body with its own `bad_request` JSON
  instead of a plain-text 422. (#13)

### Added

- User documentation under `docs/` (install, quickstart, CLI reference, HTTP
  API, agent setup, troubleshooting, FAQ) and a README written for someone
  seeing the project for the first time. (#6, #7)

## [0.1.0] - 2026-10-29

The first release.

### Added

- `sbtd`, a local daemon that keeps one warm sbt server per project and
  routes each command to the right one by walking up from the caller's
  directory to the build root. Commands against one project run one at a
  time, as sbt needs; different projects run concurrently. A stale server
  found at start-up is taken over, and instances idle for two hours are shut
  down.
- `sbtctl`, the command-line client: any sbt command line passes through
  (`sbtctl compile`, `sbtctl testOnly com.example.FooSpec`), sbt's own exit
  code comes back (clamped to 1..=124, with 125 reserved for a failure of
  the tool itself), plus `status`, `shutdown [--all]` and
  `daemon start|stop|restart|status`.
- Output arrives live, as sbt produces it: sbt's log lines, raw stdout and
  stderr of the JVM, test-framework output (forwarded by attaching to sbt's
  channel the way sbt's own thin client does), structured compiler
  diagnostics on the HTTP API, and a synthesized completion line, since sbt
  never sends its `[success]` banner to clients. When sbt dies before it can
  serve, the lines that say why are reported in full. (#1, #2)
- `sbtctl` only ever speaks loopback HTTP to `sbtd`, and `sbtd` can only
  bind a loopback address, so the tool works inside agent sandboxes that
  allow neither custom MCP servers nor Unix sockets.
- Distribution: a Homebrew formula (`brew install scottweaver/tap/sbt-server`),
  a `curl | sh` installer and archives for Apple Silicon, Intel macOS and
  x86_64 Linux, under the Apache-2.0 license. (#5)

[Unreleased]: https://github.com/scottweaver/sbt-server/compare/v1.0.0...HEAD
[1.0.0]: https://github.com/scottweaver/sbt-server/compare/v0.1.2...v1.0.0
[0.1.2]: https://github.com/scottweaver/sbt-server/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/scottweaver/sbt-server/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/scottweaver/sbt-server/releases/tag/v0.1.0
