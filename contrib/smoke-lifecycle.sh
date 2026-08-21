#!/usr/bin/env bash
# Minimal lifecycle smoke: up → nsenter (bash/exec path) → [exec] → down.
# Isolated XDG + NEALS_SOCKET; UP_CMD=sleep (no ferrari/devenv stack).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

need() { command -v "$1" >/dev/null || { echo "missing $1"; exit 1; }; }
need bwrap
need nsenter
need python3

UID_=$(id -u)
GID_=$(id -g)
if ! bwrap --unshare-user --uid "$UID_" --gid "$GID_" --unshare-net --dev-bind / / -- true 2>/dev/null; then
  echo "skip: userns unavailable"
  exit 0
fi

echo "==> build"
if [[ -d .cargo-home ]]; then export CARGO_HOME="$ROOT/.cargo-home"; fi
# Agent/CI may set CARGO_TARGET_DIR elsewhere; prefer repo-local target for PATH.
unset CARGO_TARGET_DIR
cargo build -q -p neals -p nealsd
BIN="$ROOT/target/debug"
export PATH="$BIN:$PATH"

TMP=$(mktemp -d)
NEALSD_PID=""
cleanup() {
  if [[ -n "$NEALSD_PID" ]]; then kill "$NEALSD_PID" 2>/dev/null || true; wait "$NEALSD_PID" 2>/dev/null || true; fi
  rm -rf "$TMP"
}
trap cleanup EXIT

export XDG_CONFIG_HOME="$TMP/config"
export XDG_STATE_HOME="$TMP/state"
export XDG_RUNTIME_DIR="$TMP/runtime"
export NEALS_SOCKET="$TMP/runtime/neals/nealsd.sock"
export NEALS_CADDY_CMD="-"
UP_WRAP="$TMP/up.sh"
cat >"$UP_WRAP" <<'EOF'
#!/bin/sh
mkdir -p /run/devenv-smoke-boot || exit 1
exec sleep 3600
EOF
chmod +x "$UP_WRAP"
export NEALS_UP_CMD="$UP_WRAP"
# PATH set after build
mkdir -p "$XDG_CONFIG_HOME/neals" "$XDG_STATE_HOME/neals" "$XDG_RUNTIME_DIR/neals"

PROJ="$TMP/smoke-proj"
mkdir -p "$PROJ"
cat >"$PROJ/devenv.nix" <<'EOF'
{
  neals.name = "smoke";
}
EOF

echo "==> version"
neals -V | grep -q '0.3.0'

echo "==> register"
( cd "$PROJ" && neals register )

echo "==> nealsd"
nealsd &
NEALSD_PID=$!
for _ in $(seq 1 50); do
  if python3 - <<'PY'
import os, socket
s = socket.socket(socket.AF_UNIX)
s.settimeout(0.2)
s.connect(os.environ["NEALS_SOCKET"])
s.sendall(b'{"Ping":null}\n')
assert b"Pong" in s.recv(256)
PY
  then break; fi
  sleep 0.1
done

echo "==> up"
neals up smoke -d

ipc() {
  python3 -c '
import json, os, socket, sys
s = socket.socket(socket.AF_UNIX)
s.connect(os.environ["NEALS_SOCKET"])
s.sendall((sys.argv[1] + "\n").encode())
print(s.recv(65536).decode())
' "$1"
}

STATUS=$(ipc '{"Status":null}')
NETNS_PID=$(python3 -c '
import json,sys
d=json.loads(sys.argv[1])
ps=d["Status"]["projects"]
print(next(p["netns_pid"] for p in ps if p["name"]=="smoke"))
' "$STATUS")
echo "    netns_pid=$NETNS_PID"
[[ "$NETNS_PID" -gt 1 ]]

echo "==> nsenter (same flags as bash/exec)"
nsenter --user --net --mount --preserve-credentials -t "$NETNS_PID" -- true

echo "==> /run writable in guest mount ns (devenv needs /run/devenv-*)"
nsenter --user --net --mount --preserve-credentials -t "$NETNS_PID" -- \
  test -d /run/devenv-smoke-boot
nsenter --user --net --mount --preserve-credentials -t "$NETNS_PID" -- \
  mkdir -p /run/devenv-smoke-check

# Skip `neals exec` here: UP_CMD is a stub, not devenv shell.
echo "==> skip neals exec (stub UP_CMD)"

echo "==> down"
neals down smoke
STATUS=$(ipc '{"Status":null}')
python3 -c '
import json,sys
d=json.loads(sys.argv[1])
ps=d["Status"]["projects"]
assert not any(p["name"]=="smoke" for p in ps), ps
' "$STATUS"

echo "ok: up / nsenter(+exec) / down @ 0.3.0"
