# Troubleshooting

Start with `sbtctl daemon status` and `sbtctl status`, then `sbtd.log` in
the state directory (`~/.config/sbt-server/` unless you moved it). For a
full trace of the daemon's conversation with sbt, run a daemon by hand with
`RUST_LOG=sbt_server_core=trace sbtd --bind 127.0.0.1:8099` and point
`sbtctl --daemon-url http://127.0.0.1:8099` at it.

## `` `start` is not an sbtctl command ``

You typed `sbtctl start` (or `stop`, `restart`) and meant the daemon:
`sbtctl daemon start`. sbtctl refuses the bare word rather than sending
it to sbt, which would only answer "Not a valid command". If your build
really defines an sbt command called `start`, `sbtctl -- start` sends it
through.

## `cannot reach sbtd at http://127.0.0.1:8077`

No daemon is listening there. `sbtctl daemon start`. If you started the
daemon on another port or machine-wide setting, pass `--daemon-url` or
set `SBTCTL_DAEMON_URL`.

## `sbtd is already running (pid N)`

A daemon already answers the URL. That is fine; use it. To replace it,
`sbtctl daemon restart`.

## `sbtd.pid says pid X but the daemon at ... reports pid Y; refusing to signal`

The pid file belongs to a daemon that is not the one answering, usually
because two daemons were started with different state directories or one
was started by hand. Decide which one you want, stop the other with
`kill <pid>`, and delete the stale `sbtd.pid`.

## `cannot listen on 127.0.0.1:8077`

The port is taken, by another sbtd or something else. Either stop it or run
on another port: `SBTCTL_DAEMON_URL=http://127.0.0.1:8099 sbtctl daemon start`
derives the daemon's bind address from the URL.

## `... is not a loopback address; agents may only reach sbtd over loopback HTTP`

sbtd refuses any `--bind` that is not `127.0.0.1`, `::1` or another
loopback address, by design. There is no authentication, so it must never
be reachable from the network.

## The first command is slow

That is sbt's cold start: a JVM boot plus loading the build definition,
announced on stderr as `[sbtctl] starting sbt for <root> (cold start)`.
It happens once per project per daemon lifetime, and again after an
instance is reaped for being idle two hours (`sbtd --idle-timeout-secs`
changes that; `0` disables reaping). If a large build takes longer than
three minutes to come up, raise `sbtd --boot-timeout-secs`.

## `project_load_failed`

sbt exited or stalled before publishing its server. The error carries the
last lines sbt printed, which name the cause: a broken `build.sbt`, a
missing plugin, a JVM that is not on `PATH`, a pinned sbt version that
could not be downloaded. Fix it and run the command again; nothing is
cached.

## My interactive `sbt` shell or IDE lost its server

When sbtd starts an instance for a project, it takes over any sbt server
already described by the project's `project/target/active.json`: it asks
that server to shut down, deletes the descriptor, and starts its own. This
is deliberate: a foreign server has no output capture and no lifecycle
control, and trusting it made the original build's behaviour impossible to
reason about. The practical rule: let sbtd own the server for a project
while agents are working on it, and expect an open `sbt` shell or an
IDE's background sbt to be stopped the first time `sbtctl` touches that
project. Restart them afterwards and they will start their own again.

## Output I expected is missing

- **Test framework lines** (`MySuite:`, `+ test passed`) require sbt-server
  0.1.0 or later; earlier builds never attached to sbt's channel and lost
  them.
- **`Not a valid command: foo`**: sbt's own parse errors arrive as
  `[error]` lines, followed by the synthesized
  `[error] 'foo' failed (exit code 1)`. Check the spelling against
  `sbt tasks`, and remember that sbtctl's own flags must come before the
  sbt words.
- **Output printed while the project loads** (a plugin's warning on boot)
  belongs to no command; it is in `sbtd.log` and in the
  `project_load_failed` error if the load failed.
- **Debug lines** (`Processing sbt/exec`) are dropped unless `--verbose`.

## `exit 125` with no obvious reason

The exec was aborted: sbt died mid-command (`sbt_died`), the connection
dropped (`connection_lost`), or the daemon was stopping
(`daemon_shutdown`). The stderr line says which. `sbtctl status` will show
the instance gone; the next command starts a fresh one.

## Stray JVMs

`sbtctl status` lists what sbtd owns; `sbtctl shutdown --all` stops them.
If sbtd itself was killed with SIGKILL, its instances survive it; the next
daemon takes them over through the descriptor rule above the first time
each project is used, or you can kill them by pid.

## Linux

The Linux archive builds and the test suite passes on Linux, but it is not
yet a supported platform; reports are welcome. Windows is out of scope.
