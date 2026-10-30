# Using sbt-server with coding agents

sbt-server exists for agents. An agent that runs `sbt compile` through its
shell tool pays sbt's cold start on every call, often 10 to 30 seconds for
a real build, and tends to leave stray JVMs behind. The same agent running
`sbtctl compile` gets a warm answer in milliseconds after the first call,
with the same output and the same exit code.

It fits agent sandboxes by design: the agent only ever runs a normal
command that speaks loopback HTTP. No custom MCP server, no Unix socket,
nothing to install inside the agent.

## Setup

1. Install sbt-server and start the daemon once per login:
   `sbtctl daemon start`. It is safe to put that in a shell profile or a
   launch script; it reports "already running" and exits 125 when a daemon
   is already up, so guard it with `sbtctl daemon status || sbtctl daemon start`.
2. Tell the agent to use `sbtctl` instead of `sbt`. The rest of this page
   is what to tell it.

## Claude Code

Add to the project's `CLAUDE.md` (or `AGENTS.md`):

```markdown
## Building and testing

Run sbt through `sbtctl`, never `sbt` directly: `sbtctl compile`,
`sbtctl test`, `sbtctl testOnly com.example.FooSpec`. It keeps a warm
sbt per project, so the first call takes seconds and later ones take
milliseconds. Its own flags go before the sbt words
(`sbtctl -C path/to/project test`). The exit code is sbt's own: 0 means
success, 1 means the compile or tests failed, 125 means sbtctl itself
could not run (then run `sbtctl daemon start` and retry once).
If `sbtctl` prints `[sbtctl] starting sbt ... (cold start)`, the first
call is booting a JVM; that is normal and happens once per project.
```

Claude Code will follow that for every build and test it runs. If you run
Claude Code with a restricted permission allowlist, allow `sbtctl` there
the way you would allow `sbt`.

## Cursor and other editor agents

Cursor reads `AGENTS.md`, so the same snippet works. For a rules file,
`.cursor/rules/sbt.mdc` with `alwaysApply: true` and the same text does the
job.

## Writing your own tool

Skip the CLI and call the daemon: `POST /exec` (with the bearer token from
`sbtd.token` in the state dir, see the HTTP API) returns an NDJSON stream
with a typed event per line, including structured compiler diagnostics
with 1-based file positions, which is friendlier to parse than terminal
output. See the [HTTP API](http-api.md). Watch for exactly one terminal
event per request, `done` or `aborted`, and take the exit code from `done`.

## What the agent should know

- The first command in a project is slow once; everything after is fast.
  Instances idle for two hours are reaped and simply restart on demand.
- Commands against one project run one at a time. Two agents on one
  project are fine; they queue.
- `sbtctl` finds the project from the current directory. In a monorepo
  with several sbt builds, the nearest `build.sbt` wins, or pass `-C`.
- Output is what sbt's shell would show, plus a final
  `[success] ...`/`[error] ...` line with the exit code and elapsed time.
- `sbtctl` never needs the agent to manage the daemon beyond the one-time
  start; `sbtctl shutdown --all` frees memory when a session is over.
- Authentication is invisible to the agent: `sbtctl` reads the daemon's
  token from the state directory. The one way to get it wrong is to start
  the daemon with one `--state-dir` and call `sbtctl` with another; the
  error then says so and names the flag.
