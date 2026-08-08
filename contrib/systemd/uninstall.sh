#!/usr/bin/env bash
# Remove the system daemon unit and (by default) installed binaries.
# Does not delete user data under ~/.config/neals or ~/.local/state/neals
# unless --purge is passed (as that user, or with USER_NAME=).
set -euo pipefail

PREFIX="${PREFIX:-/usr/local}"
PURGE=0
USER_NAME=""

usage() {
  echo "usage: sudo $0 [--purge] [username]" >&2
  echo "  stops/disables nealsd@USER, removes unit/drop-in/binaries/man/docs" >&2
  echo "  --purge  also remove ~/.config/neals and ~/.local/state/neals for USER" >&2
  exit 1
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --purge) PURGE=1; shift ;;
    -h|--help) usage ;;
    -*)
      echo "unknown option: $1" >&2
      usage
      ;;
    *)
      USER_NAME="$1"
      shift
      ;;
  esac
done

USER_NAME="${USER_NAME:-${SUDO_USER:-$USER}}"

if [[ "$(id -u)" -ne 0 ]]; then
  echo "run as root: sudo $0 [--purge] [username]" >&2
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

unit="nealsd@${USER_NAME}.service"
drop_in_dir="/etc/systemd/system/${unit}.d"

if systemctl cat "$unit" &>/dev/null || systemctl list-unit-files "$unit" &>/dev/null; then
  systemctl disable --now "$unit" 2>/dev/null || true
fi
# Template may still be enabled for this instance
systemctl disable --now "$unit" 2>/dev/null || true

rm -f /etc/systemd/system/nealsd@.service
rm -rf "$drop_in_dir"

rm -f "$PREFIX/bin/neals" "$PREFIX/bin/nealsd"
rm -f "$PREFIX/share/man/man1/neals.1" "$PREFIX/share/man/man8/nealsd.8"
rm -rf "$PREFIX/share/doc/neals"

systemctl daemon-reload
systemctl reset-failed "$unit" 2>/dev/null || true

if [[ "$PURGE" -eq 1 ]]; then
  user_home="$(getent passwd "$USER_NAME" | cut -d: -f6)"
  if [[ -n "$user_home" && -d "$user_home" ]]; then
    rm -rf "$user_home/.config/neals" "$user_home/.local/state/neals"
    echo "purged $user_home/.config/neals and $user_home/.local/state/neals"
  fi
fi

echo "neals system install removed for user ${USER_NAME}."
echo "  (ad-hoc nealsd may still use \$XDG_RUNTIME_DIR/neals if you start it again)"
if [[ "$PURGE" -eq 0 ]]; then
  echo "  user data kept; pass --purge to delete config/state"
fi
