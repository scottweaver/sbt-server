# HTTP API

`sbtctl` is a thin client; everything it does is one of four requests to
`sbtd`, and anything that can issue loopback HTTP can do the same. The base
URL is `http://127.0.0.1:8077` unless the daemon was started elsewhere.
There is no authentication: the daemon binds loopback only and trusts
everything on this machine.

| Method and path | Request body | Response |
|---|---|---|
| `GET /health` | | `{"status":"ok","pid":80822,"version":"0.1.0","uptime_secs":12}` |
| `GET /status` | | `{"instances":[...]}` |
| `POST /exec` | `{"cwd":"...","command_line":"compile","verbose":false}` | `application/x-ndjson`, one event per line |
| `POST /shutdown` | `{"target":"one","cwd":"..."}` or `{"target":"all"}` | `{"stopped":["/path/to/project"]}` |

Bodies are JSON with `Content-Type: application/json`.

## `GET /status`

```json
{
  "instances": [
    {
      "build_root": "/Users/you/code/my-project",
      "pid": 80829,
      "phase": "idle",
      "idle_secs": 12,
      "uptime_secs": 180
    }
  ]
}
```

`phase` is `booting`, `idle` or `running`. `pid` is absent while an instance
is still being spawned; `idle_secs` is present only when idle. Dead
instances are not listed; the next `/exec` for that project starts a fresh
one.

## `POST /exec`

`cwd` is any directory inside the project; the daemon resolves the build
root by walking upward. `command_line` is the sbt command line, as one
string. `verbose` (default `false`) keeps sbt's debug-level log lines.

The response streams as soon as the instance is ready, one JSON object per
line. The first event is always `started`; the last is exactly one terminal
event, `done` or `aborted`; free text is always in a field named `text`.

```json
{"type":"started","build_root":"/Users/you/code/my-project","pid":80829,"cold_start":true}
{"type":"log","level":"info","text":"compiling 12 Scala sources to ..."}
{"type":"output","stream":"sbt_stdout","text":"MySuite:"}
{"type":"output","stream":"sbt_stdout","text":"  + example test that succeeds 0.004s"}
{"type":"diagnostics","file":"/Users/you/code/my-project/src/main/scala/A.scala","items":[{"line":3,"column":5,"severity":"error","text":"not found: value foo"}]}
{"type":"done","outcome":"success","exit_code":0,"elapsed_ms":656,"summary":"[success] 'test' completed in 656ms"}
```

| `type` | Fields | Notes |
|---|---|---|
| `started` | `build_root`, `pid`, `cold_start` | `cold_start: true` means a JVM is booting; expect seconds, not milliseconds. |
| `log` | `level`: `debug`, `info`, `warning`, `error`; `text` | sbt's own logger. `debug` only when `verbose`. |
| `output` | `stream`: `stdout`, `stderr`, `sbt_stdout`, `sbt_stderr`; `text` | `sbt_*` is what sbt's terminal received (test output, `println`); plain `stdout`/`stderr` are the JVM's raw pipes, which only a subprocess spawned by the build writes to. |
| `diagnostics` | `file`, `items[]` with `line`, `column` (1-based), `severity`: `error`, `warning`, `information`, `hint`; `text` | One event per file per compile. An empty `items` means the file is clean now. |
| `done` | `outcome`: `success` or `failure`; `exit_code`; `elapsed_ms`; `summary` | sbt answered. `elapsed_ms` is the command's time, not the boot. |
| `aborted` | `reason`: `sbt_died`, `connection_lost`, `daemon_shutdown`; `summary` | sbt never answered. Never carries a fake exit code. |

Ordering: every `log`, `sbt_*` output and `diagnostics` event precedes
`done`. Raw `stdout`/`stderr` lines come from a separate pipe and may
arrive slightly out of step with the rest.

Commands against the same project run one at a time; a second `/exec`
waits for the first to finish. Closing the connection mid-stream does not
stop sbt: the command runs to completion so the next one finds the
instance idle.

## `POST /shutdown`

`{"target":"one","cwd":"/any/dir/in/the/project"}` stops that project's
instance; `{"target":"all"}` stops every one. The response lists the build
roots that were stopped; an empty list means nothing was warm. Shutdown is
bounded: sbt gets a `shutdown` request, then a kill if it does not exit.

## Errors

Any non-2xx response is JSON:

```json
{"code":"no_build_root","message":"no build.sbt or project/build.properties in /tmp or any parent directory"}
```

| HTTP | `code` | When |
|---|---|---|
| 400 | `no_build_root` | `cwd` is not inside an sbt project. |
| 400 | `bad_request` | `cwd` does not exist, or `command_line` is empty. |
| 500 | `project_load_failed` | sbt exited or stalled before its server came up. `output` carries the last lines sbt printed, which usually say why. |
| 500 | `start_failed` | sbt came up but the connection or handshake failed, or the server descriptor it was waiting for named a non-loopback address or a token file outside `~/.sbt`. |

## curl

```bash
curl -sN -X POST localhost:8077/exec \
  -H 'content-type: application/json' \
  -d '{"cwd":"/Users/you/code/my-project","command_line":"compile"}'
```

`-N` turns off curl's buffering so the lines appear as sbt produces them.
