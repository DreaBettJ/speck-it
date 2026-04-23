#!/usr/bin/env bash

set -euo pipefail

PROJECT_DIR="/home/lijiang/桌面/code/speak-it"
BINARY="$PROJECT_DIR/target/release/speak-it"
PID_FILE="$PROJECT_DIR/speak-it.pid"

cd "$PROJECT_DIR"

if [[ "${XDG_SESSION_TYPE:-x11}" != "x11" ]]; then
  echo "speak-it autostart skipped: X11 session required" >&2
  exit 0
fi

if [[ ! -x "$BINARY" ]]; then
  echo "speak-it autostart skipped: binary not found at $BINARY" >&2
  exit 1
fi

if [[ -f "$PID_FILE" ]]; then
  existing_pid="$(cat "$PID_FILE" 2>/dev/null || true)"
  if [[ -n "$existing_pid" ]] && kill -0 "$existing_pid" 2>/dev/null; then
    exit 0
  fi
  rm -f "$PID_FILE"
fi

nohup "$BINARY" daemon >/dev/null 2>&1 &
echo $! >"$PID_FILE"
