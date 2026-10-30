# FAQ

**How is this different from `sbtn` or `sbt --client`?**
Both talk to sbt's own server protocol; sbt-server even uses the same
`sbt/exec` request and the same attach handshake sbtn does. The
difference is who gets to connect. sbtn attaches a terminal over a Unix
socket, which coding agents in sandboxes typically cannot use, and it
serves one project. sbtd keeps the socket to itself, exposes loopback HTTP
with a typed NDJSON stream, keeps a warm instance per project and routes
each call by directory, and owns the lifecycle (takeover of stale servers,
bounded shutdown, idle reaping, real exit codes).

**Does it change my build?**
No. It launches the `sbt` you already have, in your project, with your
`project/build.properties`. It writes nothing into the project; sbt writes
its own `project/target/active.json` as it always does.

**Which sbt versions work?**
sbt 1.x's server protocol, verified against 1.11.7. The launcher on your
machine can be newer; it runs whatever each project pins. sbt 2.x is
untested.

**Is it safe to run?**
The daemon listens on loopback only and refuses any other bind address,
but it has no authentication: any process on your machine can ask it to
run sbt commands in your projects, which is the same trust your shell
already extends to any local process. Do not run it on a shared host.
Requests that look like they came from a browser (a `Host` that is not
loopback, or any `Origin` header) are refused with 403, so a web page
cannot reach it through DNS rebinding.

**How much memory does it use?**
Each warm instance is a full sbt JVM, the same one `sbt` would run for that
project. Idle instances are shut down after two hours by default;
`sbtctl shutdown --all` frees everything now.

**Two agents, one project?**
Fine. Commands against one project run one at a time, in order; different
projects run concurrently.

**Why do my test lines show up but not `[success] Total time: ...`?**
sbt deliberately skips that banner for clients; the synthesized
`[success] 'test' completed in 0.6s` line replaces it.

**Can I run several daemons?**
Yes, on different ports and state directories:
`SBTD_STATE_DIR=~/.config/sbt-server-b SBTCTL_DAEMON_URL=http://127.0.0.1:8099 sbtctl daemon start`.
Two daemons must not share a project; the second would take over the
first's instance.

**Where is the daemon's log?**
`~/.config/sbt-server/sbtd.log` when started with `sbtctl daemon start`;
on stderr when you run `sbtd` by hand.

**Can I keep it running at login?**
Not through `brew services` yet; the Homebrew formula is generated and
cannot carry a service definition. A launchd plist that runs `sbtd` works
and may be documented later. Until then, `sbtctl daemon status || sbtctl daemon start`
in a shell profile is the simple answer.
