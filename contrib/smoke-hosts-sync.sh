#!/usr/bin/env bash
# Stress-test hosts fragment merge + skip-unchanged behaviour (no root).
# Simulates what neals-hosts-sync.path does: watch a mirror, apply into a fake /etc/hosts.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

need() { command -v "$1" >/dev/null || { echo "error: missing $1" >&2; exit 1; }; }
need cargo

TMP="$(mktemp -d /tmp/neals-hosts-smoke.XXXXXX)"
trap 'rm -rf "$TMP"' EXIT

export NEALS_HOSTS_FILE="$TMP/etc-hosts"
MIRROR="$TMP/mirror"
mkdir -p "$TMP"
printf '127.0.0.1 localhost\n::1 localhost\n' >"$NEALS_HOSTS_FILE"

echo "==> unit tests (hosts)"
cargo test -p neals-common hosts:: -- --nocapture

echo "==> build neals"
cargo build -q -p neals
TARGET_DIR="$(cargo metadata --no-deps --format-version 1 | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')"
NEALS="$TARGET_DIR/debug/neals"
[[ -x "$NEALS" ]] || { echo "error: missing $NEALS" >&2; exit 1; }

block_a='# BEGIN neals
127.53.0.1 kering_db
# END neals
'
block_b='# BEGIN neals
127.53.0.1 kering_db
127.53.0.5 kering_mailpit
# END neals
'

write_mirror() {
  local content="$1"
  # Match daemon: skip rewrite when identical.
  if [[ -f "$MIRROR" ]] && cmp -s <(printf '%s' "$content") "$MIRROR"; then
    return 0
  fi
  local tmp="$MIRROR.tmp"
  printf '%s' "$content" >"$tmp"
  mv -f "$tmp" "$MIRROR"
}

apply_from_mirror() {
  "$NEALS" hosts apply --from "$MIRROR" >/dev/null
}

echo "==> stress: 100 identical mirror writes + applies"
write_mirror "$block_b"
apply_from_mirror
mtime0="$(stat -c %Y "$NEALS_HOSTS_FILE")"
for _ in $(seq 1 100); do
  write_mirror "$block_b"
  apply_from_mirror
done
mtime1="$(stat -c %Y "$NEALS_HOSTS_FILE")"
[[ "$mtime0" == "$mtime1" ]] || { echo "error: idempotent apply bumped mtime" >&2; exit 1; }
grep -q kering_db "$NEALS_HOSTS_FILE"
grep -q kering_mailpit "$NEALS_HOSTS_FILE"
[[ "$(grep -c 'BEGIN neals' "$NEALS_HOSTS_FILE")" == 1 ]]

echo "==> stress: flip blocks 40 times"
for i in $(seq 1 40); do
  if (( i % 2 == 0 )); then write_mirror "$block_a"; else write_mirror "$block_b"; fi
  apply_from_mirror
  [[ "$(grep -c 'BEGIN neals' "$NEALS_HOSTS_FILE" || true)" == 1 ]]
  grep -q '127.0.0.1 localhost' "$NEALS_HOSTS_FILE"
done

echo "==> clear block"
write_mirror ''
# empty fragment file
: >"$MIRROR"
apply_from_mirror
! grep -q 'BEGIN neals' "$NEALS_HOSTS_FILE"
grep -q '127.0.0.1 localhost' "$NEALS_HOSTS_FILE"

echo "ok: hosts sync smoke passed ($TMP)"
