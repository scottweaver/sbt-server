# sbt-server

[![CI](https://github.com/scottweaver/sbt-server/actions/workflows/ci.yml/badge.svg)](https://github.com/scottweaver/sbt-server/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/scottweaver/sbt-server?display_name=tag)](https://github.com/scottweaver/sbt-server/releases/latest)
[![Coverage](https://codecov.io/gh/scottweaver/sbt-server/branch/main/graph/badge.svg)](https://codecov.io/gh/scottweaver/sbt-server)
[![License: Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)
[![OpenSSF Scorecard](https://api.scorecard.dev/projects/github.com/scottweaver/sbt-server/badge)](https://scorecard.dev/viewer/?uri=github.com/scottweaver/sbt-server)

**Keep sbt warm for the agents and people who call it a hundred times a day.**

`sbtd` keeps one running sbt server per project; `sbtctl` is what you, your
scripts and your coding agents run instead of `sbt`. The first call in a
project pays sbt's cold start once. Every call after that is answered by the
same warm JVM, in milliseconds for a no-op, with sbt's own output and sbt's
own exit code.

```
$ sbtctl compile
[sbtctl] starting sbt for /Users/you/code/api (cold start)
[info] compiling 12 Scala sources to /Users/you/code/api/target/scala-2.13/classes ...
[info] done compiling
[success] 'compile' completed in 4.1s

$ sbtctl compile
[success] 'compile' completed in 44ms

$ sbtctl test
MySuite:
  + example test that succeeds 0.004s
[info] Passed: Total 1, Failed 0, Errors 0, Passed 1
[success] 'test' completed in 656ms
```

## Why

- **Agents pay sbt's cold start on every command.** A JVM boot plus the
  build-definition load is 10 to 30 seconds on a real build; an agent
  iterating on a failing test pays it dozens of times. sbt-server pays it
  once per project.
- **It works inside agent sandboxes.** `sbtctl` is an ordinary command that
  speaks loopback HTTP to `sbtd`. No custom MCP server, no Unix socket on
  the agent's side, nothing to install in the agent. The socket sbt
  itself listens on stays private to the daemon.
- **It routes by directory.** Run `sbtctl` anywhere inside a project and
  the right instance answers; several projects stay warm side by side,
  and commands against one project run one at a time, the way sbt needs.
- **It tells the truth.** Real exit codes, sbt's own log lines, a
  synthesized completion line sbt never sends to clients, and structured
  compiler diagnostics on the HTTP API.

## Install

macOS, with Homebrew:

```bash
brew install scottweaver/tap/sbt-server
```

Or the installer script, which puts both binaries in `~/.cargo/bin`:

```bash
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/scottweaver/sbt-server/releases/latest/download/sbt-server-installer.sh | sh
```

You need a JVM and `sbt` on your `PATH`; sbt-server launches the sbt you
already use. Other methods, requirements and uninstall: [docs/install.md](docs/install.md).

## Sixty seconds

```bash
sbtctl daemon start            # once per login; detaches, logs to ~/.config/sbt-server/sbtd.log
cd ~/code/my-project
sbtctl compile                 # cold: boots sbt for this project
sbtctl compile                 # warm
sbtctl testOnly com.example.FooSpec   # any sbt command line passes through
sbtctl status                  # what is warm
sbtctl shutdown --all          # free the JVMs
sbtctl daemon stop
```

sbtctl's own flags (`-C <dir>`, `--verbose`, `--daemon-url`) go before the
sbt words. Exit codes are sbt's own, clamped to 1..=124; 125 means the tool
itself failed, so "my build is broken" and "the tool is broken" never look
alike.

Point an agent at it with one paragraph in your `CLAUDE.md` or
`AGENTS.md`: [docs/agents.md](docs/agents.md).

## How it works

```
 agent / shell            loopback HTTP               sbt's own JSON-RPC server
┌────────────┐   POST /exec (NDJSON back)   ┌──────────────┐   tcp:// or local://   ┌─────────────────┐
│  sbtctl    │ ───────────────────────────▶ │     sbtd     │ ─────────────────────▶ │ sbt  (project A)│
│            │ ◀─────────────────────────── │              │ ◀───────────────────── │ warm JVM        │
└────────────┘   started / log / output /   │ one Instance │                        └─────────────────┘
                 diagnostics / done         │ per build    │ ─────────────────────▶ ┌─────────────────┐
                                            │ root         │                        │ sbt  (project B)│
                                            └──────────────┘                        └─────────────────┘
```

`sbtd` resolves the build root from the request's directory, boots sbt for
it if needed (`sbt --server`, exactly the way sbt's own thin client does),
takes over any stale server it finds, and drives it through sbt's JSON-RPC
protocol. Every log line, every byte of raw output and every compiler
diagnostic is forwarded as a typed event; the command's completion comes
back as `done` with the exit code, or `aborted` if sbt died, never a fake.
Instances idle for two hours are shut down. The daemon only ever binds a
loopback address, by construction, and refuses requests that do not name
one or that carry an `Origin` header, so a web page cannot reach it even
through DNS rebinding.

## Documentation

| Page | What is in it |
|---|---|
| [Install](docs/install.md) | Homebrew, the installer script, from source, requirements, uninstall |
| [Quickstart](docs/quickstart.md) | Daemon up, cold and warm compile, status, shutdown |
| [CLI reference](docs/cli.md) | Every flag, environment variable and exit code of `sbtctl` and `sbtd` |
| [HTTP API](docs/http-api.md) | `/health`, `/status`, `/exec` NDJSON events, `/shutdown`, errors |
| [Agents](docs/agents.md) | Claude Code and Cursor setup, writing your own tool against the API |
| [Troubleshooting](docs/troubleshooting.md) | What each error means and what to do |
| [FAQ](docs/faq.md) | sbtn comparison, safety, memory, several daemons |

The design decisions and the sbt protocol facts the implementation rests on
are in [`.claude/rules/ARCHITECTURE.md`](.claude/rules/ARCHITECTURE.md).

## Development

```bash
cargo test --workspace                                                            # no JVM needed
SBT_SERVER_LIVE=1 cargo test -p sbt-server-core --test live -- --ignored --nocapture  # real sbt
scripts/smoke.sh                                                                  # end to end
cargo llvm-cov --workspace --ignore-filename-regex crates/fake-sbt/                # coverage, as CI measures it
cargo +nightly fuzz run framing                                                  # fuzz a parser (see fuzz/)
cargo deny check                                                                  # advisories, licenses, bans, sources
```

Tests run sbt's protocol against `crates/fake-sbt`, a stand-in that plays
frames frozen from real sbt 1.11.7 sessions, so the suite is fast and
deterministic; two ignored live tests and the smoke script exercise a real
sbt. Rust 1.95 is pinned in `rust-toolchain.toml`; `cargo fmt`,
`cargo clippy --all-targets -- -D warnings`, the tests and `cargo deny check`
(cargo-deny: `brew install cargo-deny` or `cargo install --locked cargo-deny`)
gate every PR; `coverage.yml` measures the same tests with `cargo-llvm-cov`
and reports to Codecov. `fuzz/` holds cargo-fuzz targets for the pure parsers (`Host` header,
JSON-RPC framing and messages, `active.json`, line assembly); they need a
nightly toolchain and are run by hand, never in CI.

A release is a tag: bump the workspace version, merge, push `vX.Y.Z`, and
cargo-dist builds the archives, the installer and the Homebrew formula.

Bug reports, the PR process and the test policy are in
[CONTRIBUTING.md](CONTRIBUTING.md); what each release changed is in
[CHANGELOG.md](CHANGELOG.md).

`HANDOFF.md` is the frozen development hand-off this rebuild started from.
Work follows the rules under `.claude/rules/`.

## Platforms

macOS (Apple silicon and Intel) is supported. Linux x86_64 archives ship
and the suite passes on Linux; it is not yet a supported platform. Windows
is out of scope.

## License

Apache-2.0. See [LICENSE](LICENSE).
