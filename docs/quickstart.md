# Quickstart

Two binaries. `sbtd` is a daemon that keeps one warm sbt per project and
listens on loopback HTTP; `sbtctl` is the command you run instead of `sbt`.

## 1. Start the daemon

```bash
sbtctl daemon start
```

```
sbtd started (pid 80822) at http://127.0.0.1:8077
```

It detaches, logs to `~/.config/sbt-server/sbtd.log`, and stays up until
`sbtctl daemon stop` or you log out. Starting it twice is an error that
says so; `sbtctl daemon status` tells you whether it is running.

## 2. Run a command in a project

From anywhere inside an sbt project:

```bash
cd ~/code/my-project
sbtctl compile
```

The first command for a project pays sbt's full start, a JVM boot plus the
build-definition load, and says so on stderr:

```
[sbtctl] starting sbt for /Users/you/code/my-project (cold start)
[info] compiling 12 Scala sources to /Users/you/code/my-project/target/scala-2.13/classes ...
[info] done compiling
[success] 'compile' completed in 4.1s
```

Run it again and the same sbt answers warm, typically in tens of
milliseconds for a no-op:

```
[success] 'compile' completed in 44ms
```

Any sbt command line passes through: `sbtctl test`,
`sbtctl testOnly com.example.FooSpec`, `sbtctl "Test / compile"`. sbtctl
finds the project by walking up from the current directory to the nearest
`build.sbt` or `project/build.properties`, so `-C path/to/project` is only
needed when you are elsewhere.

The exit code is sbt's own: `0` on success, nonzero when the command
failed, so scripts and agents can branch on it. `125` is reserved for the
tool itself failing, such as an unreachable daemon.

## 3. See what is warm

```bash
sbtctl status
```

```
BUILD ROOT                            PID  PHASE       IDLE   UPTIME
/Users/you/code/my-project          80829  idle         12s      3m
```

Each project you touch gets its own instance; different projects run
concurrently, commands against the same project run one at a time.

## 4. Stop things

```bash
sbtctl shutdown          # this project's sbt
sbtctl shutdown --all    # every warm sbt
sbtctl daemon stop       # the daemon, which stops every sbt first
```

Instances idle for two hours are shut down on their own; the next command
simply starts a fresh one.

Next: the [CLI reference](cli.md) for every flag, or [Agents](agents.md)
to make your coding agent use it.
