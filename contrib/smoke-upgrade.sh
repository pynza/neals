#!/usr/bin/env bash
# Upgrade lifecycle smoke (no real packages/services): admin drop-ins survive,
# postinst try-restarts without re-enabling, first install enables, removal
# still stops/disables.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

# Any non-root user with a real passwd entry; postinst skips SUDO_USER=root.
TEST_USER="$(getent passwd | awk -F: '$3 >= 1000 && $7 !~ /nologin/ {print $1; exit}')"
TEST_USER="${TEST_USER:-nobody}"

mkdir -p "$TMP/bin" "$TMP/state" "$TMP/run/systemd/system" \
  "$TMP/unit/nealsd@${TEST_USER}.service.d"

# Stub systemctl records calls instead of touching the host manager.
cat >"$TMP/bin/systemctl" <<'EOF'
#!/bin/sh
printf '%s\n' "$*" >>"$NEALS_SYSTEMCTL_LOG"
exit 0
EOF
chmod +x "$TMP/bin/systemctl"

echo "$TEST_USER" >"$TMP/state/deb-user"
DROP_IN="$TMP/unit/nealsd@${TEST_USER}.service.d/override.conf"
printf '[Service]\nEnvironment=NEALS_ALLOW_HOST_LOOPBACK=1\n' >"$DROP_IN"

export PATH="$TMP/bin:$PATH"
export NEALS_SYSTEMCTL=systemctl
export NEALS_STATE_DIR="$TMP/state"
export NEALS_SYSTEMD_RUN_DIR="$TMP/run/systemd/system"
export NEALS_SYSTEMCTL_LOG="$TMP/calls"
export NEALS_DEB_SETUP=y
export SUDO_USER="$TEST_USER"

fail() { echo "smoke-upgrade: $*" >&2; exit 1; }

# 1) prerm on upgrade: no stop/disable, no touch of /etc drop-ins.
"$ROOT/contrib/packaging/deb/prerm" upgrade
[[ -f "$DROP_IN" ]] || fail "prerm upgrade deleted the admin drop-in"
grep -q 'NEALS_ALLOW_HOST_LOOPBACK=1' "$DROP_IN" || fail "prerm upgrade changed the drop-in"
[[ ! -s "$NEALS_SYSTEMCTL_LOG" ]] || fail "prerm upgrade called systemctl"

# 2) postinst on upgrade: reload + try-restart, drop-in still intact.
: >"$NEALS_SYSTEMCTL_LOG"
"$ROOT/contrib/packaging/deb/postinst" configure 0.6.2
grep -qx 'daemon-reload' "$NEALS_SYSTEMCTL_LOG" || fail "postinst did not daemon-reload"
grep -qx "try-restart nealsd@${TEST_USER}.service" "$NEALS_SYSTEMCTL_LOG" \
  || fail "postinst did not try-restart nealsd"
grep -q 'NEALS_ALLOW_HOST_LOOPBACK=1' "$DROP_IN" || fail "postinst changed the drop-in"
if grep -q '^enable ' "$NEALS_SYSTEMCTL_LOG"; then
  fail "postinst upgrade re-enabled a deliberately disabled unit"
fi

# 3) First install (deb configure with no previous version): enable as before.
rm -f "$TMP/state/deb-user"
: >"$NEALS_SYSTEMCTL_LOG"
"$ROOT/contrib/packaging/deb/postinst" configure
grep -qx "enable --now nealsd@${TEST_USER}.service" "$NEALS_SYSTEMCTL_LOG" \
  || fail "first install did not enable nealsd"
if grep -q '^try-restart ' "$NEALS_SYSTEMCTL_LOG"; then
  fail "first install should not try-restart"
fi
grep -q 'NEALS_ALLOW_HOST_LOOPBACK=1' "$DROP_IN" || fail "first install changed the drop-in"

# 4) Real removal (dpkg -r, no purge): stop/disable, leave drop-in, mark reinstall.
: >"$NEALS_SYSTEMCTL_LOG"
"$ROOT/contrib/packaging/deb/prerm" remove
grep -qx "stop nealsd@${TEST_USER}.service" "$NEALS_SYSTEMCTL_LOG" || fail "prerm remove did not stop"
grep -qx "disable nealsd@${TEST_USER}.service" "$NEALS_SYSTEMCTL_LOG" || fail "prerm remove did not disable"
grep -q 'NEALS_ALLOW_HOST_LOOPBACK=1' "$DROP_IN" || fail "prerm remove changed the drop-in"
"$ROOT/contrib/packaging/deb/postrm" remove
[[ -f "$TMP/state/reinstall" ]] || fail "postrm remove did not mark the reinstall"

# 5) Reinstall after `dpkg -r`: dpkg passes configure <old version>, but prerm
#    disabled the unit — this must still re-enable.
: >"$NEALS_SYSTEMCTL_LOG"
"$ROOT/contrib/packaging/deb/postinst" configure 1.0
grep -qx "enable --now nealsd@${TEST_USER}.service" "$NEALS_SYSTEMCTL_LOG" \
  || fail "reinstall after remove did not re-enable nealsd"
[[ ! -f "$TMP/state/reinstall" ]] || fail "postinst did not clear the reinstall marker"

# 6) A real upgrade after that must not re-enable again.
: >"$NEALS_SYSTEMCTL_LOG"
"$ROOT/contrib/packaging/deb/postinst" configure 1.1
if grep -q '^enable ' "$NEALS_SYSTEMCTL_LOG"; then
  fail "upgrade after reinstall re-enabled nealsd"
fi

# 7) Purge: admin drop-in is still not ours to delete.
"$ROOT/contrib/packaging/deb/postrm" purge
grep -q 'NEALS_ALLOW_HOST_LOOPBACK=1' "$DROP_IN" || fail "postrm purge changed the drop-in"

echo "smoke-upgrade: drop-ins preserved across upgrade; try-restart issued"
