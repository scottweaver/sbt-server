#!/usr/bin/env bash
# End-to-end smoke against a real sbt and JVM: build both binaries, start
# sbtd on a spare port, run the fixture project cold then warm through
# sbtctl, check exit codes, status, shutdown and daemon stop.
#
# Usage: scripts/smoke.sh            (needs `sbt` and a JVM on PATH)
#        SBT_SERVER_SMOKE_PORT=8099 scripts/smoke.sh
set -euo pipefail
cd "$(dirname "$0")/.."

cargo build -q -p sbtd -p sbtctl
export SBTD_STATE_DIR
SBTD_STATE_DIR="$(mktemp -d)"
export SBTCTL_DAEMON_URL="http://127.0.0.1:${SBT_SERVER_SMOKE_PORT:-8099}"
SBTCTL=target/debug/sbtctl
FIX=tests/fixtures/hello-sbt

cleanup() { "$SBTCTL" daemon stop >/dev/null 2>&1 || true; }
trap cleanup EXIT

step() { printf '\n--- %s ---\n' "$1"; }

step "daemon start"
"$SBTCTL" daemon start
"$SBTCTL" daemon status

step "cold compile"
time "$SBTCTL" -C "$FIX" compile

step "warm compile"
time "$SBTCTL" -C "$FIX" compile

step "failing command (expect exit 1)"
if "$SBTCTL" -C "$FIX" doesNotExist; then
  echo "smoke: expected a failure" >&2
  exit 1
else
  code=$?
  [ "$code" -eq 1 ] || { echo "smoke: expected exit 1, got $code" >&2; exit 1; }
fi

step "subprocess output reaches the pipes"
"$SBTCTL" -C "$FIX" shout | grep -q raw-from-subprocess

step "status, shutdown, daemon stop"
"$SBTCTL" status
"$SBTCTL" -C "$FIX" shutdown
"$SBTCTL" shutdown --all
"$SBTCTL" daemon stop
trap - EXIT

printf '\nsmoke: OK\n'
