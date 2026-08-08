#!/usr/bin/env bash
# Install neals + nealsd, man pages, docs, and enable the system daemon for one user.
# Requires: root, systemd, caddy on PATH for that user, built release binaries.
set -euo pipefail

USER_NAME="${1:-${SUDO_USER:-$USER}}"
PREFIX="${PREFIX:-/usr/local}"
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
UNIT_SRC="$ROOT/contrib/systemd/nealsd@.service"
DOC_SRC="$ROOT/contrib/systemd/README.md"
MAN1_SRC="$ROOT/contrib/man/neals.1"
MAN8_SRC="$ROOT/contrib/man/nealsd.8"

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
install -Dm644 "$MAN1_SRC" "$PREFIX/share/man/man1/neals.1"
install -Dm644 "$MAN8_SRC" "$PREFIX/share/man/man8/nealsd.8"

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
# Refresh man DB when available (ignore failures on minimal systems).
if command -v mandb >/dev/null 2>&1; then
  mandb -q "$PREFIX/share/man" 2>/dev/null || true
fi

systemctl enable --now "nealsd@${USER_NAME}.service"

echo
echo "nealsd@${USER_NAME} enabled."
echo "  socket: /run/neals/nealsd.sock"
echo "  HTTP:   http://{service}.{project}.localhost/  (port 80)"
echo "  man:    man neals    man nealsd"
echo
echo "Try:  sudo -u $USER_NAME $PREFIX/bin/neals doctor"
echo "      sudo -u $USER_NAME $PREFIX/bin/neals status"
echo
echo "Uninstall: sudo $ROOT/contrib/systemd/uninstall.sh $USER_NAME"
echo "           sudo $ROOT/contrib/systemd/uninstall.sh --purge $USER_NAME"
