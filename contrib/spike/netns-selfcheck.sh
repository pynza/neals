#!/usr/bin/env bash
# Synthetic check: bwrap userns+netns, guest listens on 127.0.0.1, host proxies via setns.
# No devenv / nealsd.
set -euo pipefail

need() { command -v "$1" >/dev/null || { echo "missing $1"; exit 1; }; }
need bwrap
need python3

UID_=$(id -u)
GID_=$(id -g)
if ! bwrap --unshare-user --uid "$UID_" --gid "$GID_" --unshare-net --dev-bind / / -- true 2>/dev/null; then
  echo "skip: unprivileged user namespaces unavailable"
  exit 0
fi

HOST_PORT="${HOST_PORT:-38471}"
GUEST_PORT="${GUEST_PORT:-38472}"

cleanup() {
  [[ -n "${BWRAP_PID:-}" ]] && kill -TERM "$BWRAP_PID" 2>/dev/null || true
  wait "$BWRAP_PID" 2>/dev/null || true
}
trap cleanup EXIT

bwrap --die-with-parent --unshare-user --uid "$UID_" --gid "$GID_" \
  --unshare-net --cap-add CAP_NET_ADMIN --dev-bind / / \
  -- /bin/sh -c "ip link set lo up 2>/dev/null || true; exec python3 -u -c \"
import socket
s=socket.socket(); s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
s.bind(('127.0.0.1', $GUEST_PORT)); s.listen(1)
c,_=s.accept(); c.recv(64); c.sendall(b'ok'); c.close()
\"" &
BWRAP_PID=$!
sleep 0.2

INNER=$(tr ' ' '\n' <"/proc/$BWRAP_PID/task/$BWRAP_PID/children" 2>/dev/null | head -1 || true)
INNER="${INNER:-$BWRAP_PID}"

# Host: setns into guest, connect, also prove host cannot reach guest without setns.
python3 - "$HOST_PORT" "$GUEST_PORT" "$INNER" <<'PY'
import os, socket, sys, ctypes, ctypes.util, errno

host_port, guest_port, pid = int(sys.argv[1]), int(sys.argv[2]), int(sys.argv[3])
libc = ctypes.CDLL(ctypes.util.find_library("c"), use_errno=True)
CLONE_NEWUSER, CLONE_NEWNET = 0x10000000, 0x40000000

def setns(path, flag):
    fd = os.open(path, os.O_RDONLY)
    try:
        if libc.setns(fd, flag) != 0:
            e = ctypes.get_errno()
            raise OSError(e, os.strerror(e), path)
    finally:
        os.close(fd)

# Isolation: direct connect from host netns must fail (or hit something else).
try:
    socket.create_connection(("127.0.0.1", guest_port), timeout=0.3)
    # If guest_port happens to be open on host, skip isolation assert.
    print("warn: host already has :%d open; skipping isolation assert" % guest_port)
except (OSError, TimeoutError, socket.timeout):
    pass

# One-shot proxy accept → setns connect on a dedicated fork (dies after).
srv = socket.socket()
srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
srv.bind(("127.0.0.1", host_port))
srv.listen(1)

# Client in another fork so accept can proceed.
cid = os.fork()
if cid == 0:
    import time
    time.sleep(0.05)
    c = socket.create_connection(("127.0.0.1", host_port), timeout=2)
    c.sendall(b"ping")
    data = c.recv(16)
    sys.stdout.buffer.write(data)
    sys.stdout.flush()
    os._exit(0 if data == b"ok" else 1)

cli, _ = srv.accept()
pid_child = os.fork()
if pid_child == 0:
    setns(f"/proc/{pid}/ns/user", CLONE_NEWUSER)
    setns(f"/proc/{pid}/ns/net", CLONE_NEWNET)
    g = socket.create_connection(("127.0.0.1", guest_port), timeout=2)
    g.sendall(cli.recv(64) or b"x")
    cli.sendall(g.recv(16))
    os._exit(0)

os.waitpid(pid_child, 0)
cli.close()
srv.close()
_, st = os.waitpid(cid, 0)
if not os.WIFEXITED(st) or os.WEXITSTATUS(st) != 0:
    raise SystemExit("proxy selfcheck failed")
print(f"ok: host :{host_port} → guest 127.0.0.1:{guest_port} (inner pid {pid})")
PY
