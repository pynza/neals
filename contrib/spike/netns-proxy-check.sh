#!/usr/bin/env bash
# Exercises the real `nealsd --netns-proxy` helper: a service bound to the guest's loopback
# must be unreachable from the host, yet reachable through the helper's host port.
#
# Usage: netns-proxy-check.sh [path/to/nealsd]   (default: cargo's debug build)
set -uo pipefail

need() { command -v "$1" >/dev/null || { echo "missing $1"; exit 1; }; }
need bwrap
need python3
need curl

NEALSD="${1:-$(dirname "$0")/../../target/debug/nealsd}"
if [[ ! -x "$NEALSD" ]]; then
  echo "missing nealsd at $NEALSD (cargo build -p nealsd)"
  exit 1
fi

UID_=$(id -u)
GID_=$(id -g)
if ! bwrap --unshare-user --uid "$UID_" --gid "$GID_" --unshare-net --dev-bind / / -- true 2>/dev/null; then
  echo "skip: unprivileged user namespaces unavailable"
  exit 0
fi

HOST_PORT="${HOST_PORT:-38481}"
GUEST_PORT="${GUEST_PORT:-38482}"
BWRAP_PID=""
DRIVER_PID=""

cleanup() {
  [[ -n "$DRIVER_PID" ]] && kill -KILL "$DRIVER_PID" 2>/dev/null
  [[ -n "$BWRAP_PID" ]] && kill -KILL "$BWRAP_PID" 2>/dev/null
}
trap cleanup EXIT

bwrap --die-with-parent --unshare-user --uid "$UID_" --gid "$GID_" \
  --unshare-net --cap-add CAP_NET_ADMIN --dev-bind / / \
  -- /bin/sh -c "ip link set lo up 2>/dev/null || true; exec python3 -m http.server $GUEST_PORT --bind 127.0.0.1" \
  </dev/null >/dev/null 2>&1 &
BWRAP_PID=$!
sleep 2

INNER=$(awk '{print $1}' "/proc/$BWRAP_PID/task/$BWRAP_PID/children" 2>/dev/null)
if [[ -z "$INNER" ]]; then echo "fail: no process inside bwrap"; exit 1; fi

if curl -s -m 2 "http://127.0.0.1:$GUEST_PORT/" >/dev/null 2>&1; then
  echo "fail: guest :$GUEST_PORT reachable from the host, netns is not isolating"
  exit 1
fi

# nealsd expects the bound host listener on stdin; hand one over the same way the daemon does.
# Keep the heredoc last: a later `<` redirection would replace it.
python3 - >/tmp/neals-netns-proxy-check.log 2>&1 <<PY &
import socket, subprocess, time
ls = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
ls.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
ls.bind(("127.0.0.1", $HOST_PORT))
ls.listen(16)
p = subprocess.Popen(["$NEALSD", "--netns-proxy", "$INNER", "$GUEST_PORT"], stdin=ls.fileno())
time.sleep(20)
p.kill()
PY
DRIVER_PID=$!
sleep 3

if curl -s -m 5 "http://127.0.0.1:$HOST_PORT/" | grep -qi "directory listing"; then
  echo "ok: host :$HOST_PORT → guest 127.0.0.1:$GUEST_PORT (inner pid $INNER)"
  exit 0
fi
echo "fail: guest service not reachable through the proxy helper"
cat /tmp/neals-netns-proxy-check.log 2>/dev/null
exit 1
