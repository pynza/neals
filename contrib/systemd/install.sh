#!/usr/bin/env bash
# Install neals + nealsd and enable the system daemon for one user.
# Requires: root, systemd, caddy on PATH for that user, built release binaries.
set -euo pipefail

USER_NAME="${1:-${SUDO_USER:-$USER}}"
PREFIX="${PREFIX:-/usr/local}"
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
UNIT_SRC="$ROOT/contrib/systemd/nealsd@.service"
DOC_SRC="$ROOT/contrib/systemd/README.md"

if [[ "$(id -u)" -ne 0 ]]; then
  echo "run as root: sudo $0 [username]" >&2
  exit 1
fi

if [[ "$USER_NAME" == "root" ]]; then
  echo "pass your login user: sudo $0 alice" >&2
  exit 1
fi

if ! id "$USER_NAME" >/dev/null 2>&1; then
  echo "unknown user: $USER_NAME" >&2
  exit 1
fi

for bin in neals nealsd; do
  src="$ROOT/target/release/$bin"
  if [[ ! -x "$src" ]]; then
    echo "missing $src — build first: cargo build --release -p neals -p nealsd" >&2
    exit 1
  fi
  install -Dm755 "$src" "$PREFIX/bin/$bin"
done

install -Dm644 "$UNIT_SRC" /etc/systemd/system/nealsd@.service
install -Dm644 "$DOC_SRC" "$PREFIX/share/doc/neals/systemd.md"

# Point ExecStart at PREFIX if not /usr/local
if [[ "$PREFIX" != "/usr/local" ]]; then
  sed -i "s|/usr/local/bin/nealsd|$PREFIX/bin/nealsd|" /etc/systemd/system/nealsd@.service
fi

# systemd PATH is minimal; prepend the user's nix profile when present.
user_home="$(getent passwd "$USER_NAME" | cut -d: -f6)"
nix_bin="$user_home/.nix-profile/bin"
drop_in_dir="/etc/systemd/system/nealsd@${USER_NAME}.service.d"
if [[ -x "$nix_bin/caddy" || -d "$nix_bin" ]]; then
  install -d "$drop_in_dir"
  cat >"$drop_in_dir/path.conf" <<EOF
[Service]
Environment=PATH=${nix_bin}:/usr/local/bin:/usr/bin:/bin
EOF
fi

systemctl daemon-reload
systemctl enable --now "nealsd@${USER_NAME}.service"

echo
echo "nealsd@${USER_NAME} enabled."
echo "  socket: /run/neals/nealsd.sock"
echo "  HTTP:   http://{service}.{project}.localhost/  (port 80)"
echo
echo "Try:  sudo -u $USER_NAME $PREFIX/bin/neals doctor"
echo "      sudo -u $USER_NAME $PREFIX/bin/neals status"
