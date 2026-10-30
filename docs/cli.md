# CLI reference

## `sbtctl`

```
sbtctl [OPTIONS] <sbt command line>
sbtctl [OPTIONS] status
sbtctl [OPTIONS] shutdown [--all]
sbtctl [OPTIONS] daemon <start|stop|restart|status>
```

Any words that are not one of sbtctl's own subcommands are joined with
spaces and sent to sbt as one command line. **sbtctl's flags must come
before them**: `sbtctl -C proj --verbose testOnly com.ex.FooSpec`. Anything
after the first sbt word belongs to sbt.

### Options

| Option | Environment | Default | Meaning |
|---|---|---|---|
| `-C`, `--directory <DIR>` | | current directory | Run as if started in `DIR`. The project is the nearest ancestor holding `build.sbt` or `project/build.properties`. |
| `--daemon-url <URL>` | `SBTCTL_DAEMON_URL` | `http://127.0.0.1:8077` | Where sbtd listens. `daemon start` derives sbtd's `--bind` from it. |
| `--state-dir <DIR>` | `SBTD_STATE_DIR` | `$XDG_CONFIG_HOME/sbt-server`, else `~/.config/sbt-server` | Where sbtd keeps `sbtd.pid`, `sbtd.addr`, `sbtd.token` and `sbtd.log`; sbtctl reads the bearer token from here, so it must be the directory sbtd runs with. |
| `--verbose` | | off | Also show sbt's debug-level log lines (the "Processing sbt/exec" chatter). |
| `-V`, `--version` | | | Print the version. |

### Subcommands

**`status`** lists the warm instances: build root, sbt's pid, phase
(`booting`, `idle`, `running`), idle time and uptime. Prints
`no warm sbt instances` when there are none.

**`shutdown`** stops the warm sbt for the current project (or `-C`'s), with
a bounded `shutdown` request to sbt followed by a kill if it does not
exit. `--all` stops every instance. Stopping a project that has no warm
instance is not an error.

A bare `start`, `stop` or `restart` as the whole command line is refused
with a pointer at `sbtctl daemon <verb>`, since that is almost always what
was meant; `sbtctl -- start` sends the word to sbt for a build that really
defines such a command.

**`daemon start`** spawns `sbtd` detached in its own process group, with
stdout and stderr appended to `sbtd.log`, and waits up to five seconds for
`/health` to answer. It looks for `sbtd` next to its own executable, then
on `PATH`. Fails with "already running" if a daemon answers.

**`daemon stop`** reads `sbtd.pid`, checks that `/health` reports that same
pid, sends SIGTERM, and waits up to ten seconds for `/health` to stop
answering. The daemon shuts every instance down before it exits. Stopping
a daemon that is not running succeeds and says so.

**`daemon restart`** is stop (tolerating not running) then start.

**`daemon status`** prints pid, version and uptime, or "not running" with
exit code 1.

### Output

sbtctl renders what sbt's own shell would have shown:

- `[info]`, `[warn]`, `[error]` lines on stdout, as sbt logged them.
- Raw output on the stream it used: what tests print, what a build plugin's
  subprocess prints, forwarded line by line.
- A cold start announced on stderr as
  `[sbtctl] starting sbt for <root> (cold start)`.
- One synthesized line last, because sbt never sends its own banner to a
  client: `[success] 'compile' completed in 1.2s` or
  `[error] 'compile' failed (exit code 1) in 0.8s: <sbt's reason>`.

Compiler diagnostics are not printed a second time; sbt already logs each
one as an `[error]` or `[warn]` line. The structured form is on the
[HTTP API](http-api.md).

### Exit codes

| Code | Meaning |
|---|---|
| `0` | sbt ran the command and it succeeded. |
| `1` to `124` | sbt ran the command and it failed; this is sbt's exit code, clamped to that range. A compile error or a failed test is `1`. |
| `125` | The tool failed, not the build: sbtd unreachable, the request refused (no build root, project failed to load), the exec aborted (sbt died, daemon shutting down), or a protocol error. The message on stderr says which, and `sbtd.log` has the rest. |
| `2` | A usage mistake, such as `sbtctl start` without `daemon`; the message says what to run instead. |
| `1` from `daemon status` | sbtd is not running. |

## `sbtd`

Normally started by `sbtctl daemon start`; run it by hand for a foreground
daemon or a second one on another port.

| Option | Environment | Default | Meaning |
|---|---|---|---|
| `--bind <ADDR>` | `SBTD_BIND` | `127.0.0.1:8077` | Loopback address to listen on; port `0` picks a free port, written to `sbtd.addr`. A non-loopback address is refused outright. |
| `--idle-timeout-secs <N>` | `SBTD_IDLE_TIMEOUT_SECS` | `7200` | Shut down instances idle this long; `0` keeps them warm forever. A running instance is never reaped. |
| `--sbt-program <PATH>` | `SBTD_SBT_PROGRAM` | `sbt` | The sbt launcher to spawn. |
| `--boot-timeout-secs <N>` | `SBTD_BOOT_TIMEOUT_SECS` | `180` | How long a fresh sbt may take to publish its server before the start is abandoned. Raise it for large builds on slow machines. |
| `--state-dir <DIR>` | `SBTD_STATE_DIR` | XDG config dir | Where the pid, address and token files live. |

Logging uses `RUST_LOG` (`tracing` syntax): `RUST_LOG=debug` for daemon
decisions, `RUST_LOG=sbt_server_core=trace` to see every JSON-RPC frame
exchanged with sbt. When started by `sbtctl daemon start`, the output is
in `sbtd.log`.

sbtd exits cleanly on SIGTERM or Ctrl-C: it stops every instance first, so
a command in flight ends with an `aborted` event rather than hanging, then
removes its pid and address files.
