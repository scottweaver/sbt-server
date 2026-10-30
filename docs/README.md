# sbt-server documentation

`sbtd` keeps one warm sbt server per project; `sbtctl` is what you, and
your coding agents, run instead of `sbt`.

| Page | Read it when |
|---|---|
| [Install](install.md) | you are putting sbt-server on a machine, or taking it off |
| [Quickstart](quickstart.md) | you want the daemon up and a compile running in a minute |
| [CLI reference](cli.md) | you need every flag, environment variable and exit code of `sbtctl` and `sbtd` |
| [HTTP API](http-api.md) | you are calling the daemon directly, or writing a tool that does |
| [Agents](agents.md) | you are pointing Claude Code, Cursor or another agent at `sbtctl` |
| [Troubleshooting](troubleshooting.md) | something did not behave as these pages say |
| [FAQ](faq.md) | you are wondering how it compares to sbtn, what it costs, or whether it is safe |

The binding design decisions, including the exact sbt protocol facts the
implementation relies on, live in
[`.claude/rules/ARCHITECTURE.md`](../.claude/rules/ARCHITECTURE.md).
