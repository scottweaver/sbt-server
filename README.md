# sbt-server

`sbtd` keeps one warm sbt server process per build root; `sbtctl` is the
command agents (and people) run instead of `sbt`. Each call is routed to
the right project automatically and skips sbt's cold start (JVM boot plus
build-definition load) after the first one.

Agents only ever speak loopback HTTP to `sbtd`. `sbtd` drives sbt through
sbt's own JSON-RPC server protocol, which may be a Unix socket; that leg is
never exposed. Every binding constraint and the HTTP contract live in
[`.claude/rules/ARCHITECTURE.md`](.claude/rules/ARCHITECTURE.md).

## Requirements

- Rust 1.95 (pinned in `rust-toolchain.toml`)
- `sbt` and a JVM on `PATH` (developed against sbt 1.11.7 on Java 21)
- macOS or Linux

## Build and install

```bash
cargo build --release
cp target/release/sbtd target/release/sbtctl ~/.local/bin/
```

`sbtctl daemon start` looks for `sbtd` next to its own executable first,
then on `PATH`; keep the two binaries together.

## Usage

```bash
sbtctl daemon start                  # detaches sbtd and waits for /health
sbtctl compile                       # from inside any sbt project
sbtctl -C path/to/project test       # or name the project
sbtctl testOnly com.example.FooSpec  # any sbt command line passes through
sbtctl status                        # warm instances
sbtctl shutdown                      # stop this project's sbt; --all for every one
sbtctl daemon stop
```

sbtctl's own flags (`-C`, `--verbose`, `--daemon-url`, `--state-dir`) must
come before the sbt command line.

**Exit codes.** sbt's own exit code (clamped to 1..=124) when sbt
answered; 125 when the tool failed rather than the build (sbtd
unreachable, request refused, exec aborted).

**Output** mirrors sbt's shell: `[info]`/`[warn]`/`[error]` lines, raw
subprocess output on the stream it used, and a synthesized
`[success] 'compile' completed in 1.2s` or
`[error] 'compile' failed (exit code 1) in 0.8s` last, because sbt never
sends its own banner to clients.

The first call for a project pays the full JVM boot, announced on stderr
as `[sbtctl] starting sbt for <root> (cold start)`; later calls reuse the
warm instance. Instances idle for two hours are shut down
(`sbtd --idle-timeout-secs`, `0` disables).

**Configuration.** `SBTCTL_DAEMON_URL` (default `http://127.0.0.1:8077`),
`SBTD_STATE_DIR` (pid, address and log files; default
`~/.config/sbt-server`), `SBTD_BIND` (loopback only, by construction).

## HTTP API

`GET /health`, `GET /status`, `POST /exec` (streams NDJSON events) and
`POST /shutdown`, documented under "HTTP contract" in
[`.claude/rules/ARCHITECTURE.md`](.claude/rules/ARCHITECTURE.md). Anything
that can issue loopback HTTP can use it directly:

```bash
curl -sN -X POST localhost:8077/exec -H 'content-type: application/json' \
  -d '{"cwd":"/path/to/project","command_line":"compile"}'
```

## Development

```bash
cargo test --workspace                                                            # no JVM needed
SBT_SERVER_LIVE=1 cargo test -p sbt-server-core --test live -- --ignored --nocapture  # real sbt
scripts/smoke.sh                                                                  # end to end
```

Tests exercise sbt's protocol against `crates/fake-sbt`, a stand-in that
plays frames frozen from real sbt 1.11.7 captures. `HANDOFF.md` is the
frozen development hand-off this rebuild started from; the rules agents
and humans work by are under `.claude/rules/`.
