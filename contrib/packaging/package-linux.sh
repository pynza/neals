#!/usr/bin/env bash
# Build release binaries and package as .tar.gz, .zip, and .deb for one arch.
# Does not publish to any store — artifacts land in ./dist for GitHub Releases.
#
# Usage:
#   ./contrib/packaging/package-linux.sh [VERSION] [TARGET]
# TARGET defaults to host triple (x86_64-unknown-linux-gnu or aarch64-unknown-linux-gnu).
# Set CLEAR_DIST=1 to wipe ./dist first (default: keep other arch artifacts).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

host_target() {
  case "$(uname -m)" in
    x86_64|amd64) echo "x86_64-unknown-linux-gnu" ;;
    aarch64|arm64) echo "aarch64-unknown-linux-gnu" ;;
    *)
      echo "error: unsupported host arch $(uname -m)" >&2
      exit 1
      ;;
  esac
}

deb_arch_for() {
  case "$1" in
    x86_64-unknown-linux-gnu) echo "amd64" ;;
    aarch64-unknown-linux-gnu) echo "arm64" ;;
    *)
      echo "error: no deb arch mapping for $1" >&2
      exit 1
      ;;
  esac
}

if [[ "${1:-}" != "" ]]; then
  VERSION="${1#v}"
else
  VERSION="$(sed -n 's/^version = "\([^"]*\)"/\1/p' crates/cli/Cargo.toml | head -1)"
fi
if [[ -z "$VERSION" ]]; then
  echo "error: could not determine version" >&2
  exit 1
fi

TARGET="${2:-$(host_target)}"
ARCH_DEB="$(deb_arch_for "$TARGET")"
HOST_TARGET="$(host_target)"

NAME="neals"
BASE="${NAME}-v${VERSION}-${TARGET}"
DIST="$ROOT/dist"
STAGE="$DIST/$BASE"
DEB_ROOT="$DIST/deb-root-${ARCH_DEB}"

if [[ "${CLEAR_DIST:-0}" == "1" ]]; then
  rm -rf "$DIST"
fi
rm -rf "$STAGE" "$DEB_ROOT"
mkdir -p "$STAGE"/{man,systemd,doc} "$DEB_ROOT" "$DIST"

echo "==> cargo build --release ($NAME + nealsd) for $TARGET"
if [[ "$TARGET" == "$HOST_TARGET" ]]; then
  cargo build --release -p neals -p nealsd
  TARGET_DIR="$(cargo metadata --no-deps --format-version 1 | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')"
  BIN_DIR="$TARGET_DIR/release"
else
  rustup target add "$TARGET"
  cargo build --release -p neals -p nealsd --target "$TARGET"
  TARGET_DIR="$(cargo metadata --no-deps --format-version 1 | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')"
  BIN_DIR="$TARGET_DIR/$TARGET/release"
fi

install -m755 "$BIN_DIR/neals" "$STAGE/neals"
install -m755 "$BIN_DIR/nealsd" "$STAGE/nealsd"
strip "$STAGE/neals" "$STAGE/nealsd" || true

cp contrib/man/neals.1 "$STAGE/man/"
cp contrib/man/nealsd.8 "$STAGE/man/"
cp contrib/systemd/nealsd@.service "$STAGE/systemd/"
cp contrib/systemd/install.sh "$STAGE/systemd/"
cp contrib/systemd/uninstall.sh "$STAGE/systemd/"
chmod 755 "$STAGE/systemd/install.sh" "$STAGE/systemd/uninstall.sh"
cp contrib/systemd/README.md "$STAGE/doc/systemd.md"
cp README.md "$STAGE/README.md"

cat >"$STAGE/INSTALL.txt" <<EOF
Neals v${VERSION} (${TARGET})

Binaries: ./neals ./nealsd

Quick (user-local):
  install -m755 neals nealsd ~/.local/bin/

System (portless *.localhost on :80):
  sudo ./systemd/install.sh "\$USER"

Requires on PATH at runtime: nix, devenv, caddy
EOF

echo "==> archives"
tar -C "$DIST" -czf "$DIST/${BASE}.tar.gz" "$BASE"
(
  cd "$DIST"
  zip -qr "${BASE}.zip" "$BASE"
)

echo "==> .deb ($ARCH_DEB)"
PKG="$DEB_ROOT/${NAME}_${VERSION}_${ARCH_DEB}"
mkdir -p \
  "$PKG/DEBIAN" \
  "$PKG/usr/bin" \
  "$PKG/usr/share/man/man1" \
  "$PKG/usr/share/man/man8" \
  "$PKG/usr/lib/systemd/system" \
  "$PKG/usr/share/doc/${NAME}"

install -m755 "$STAGE/neals" "$PKG/usr/bin/neals"
install -m755 "$STAGE/nealsd" "$PKG/usr/bin/nealsd"

cp "$STAGE/man/neals.1" "$PKG/usr/share/man/man1/"
cp "$STAGE/man/nealsd.8" "$PKG/usr/share/man/man8/"
gzip -9n "$PKG/usr/share/man/man1/neals.1" "$PKG/usr/share/man/man8/nealsd.8"

sed 's|/usr/local/bin/nealsd|/usr/bin/nealsd|' \
  "$STAGE/systemd/nealsd@.service" \
  >"$PKG/usr/lib/systemd/system/nealsd@.service"

# Ship install helpers under /usr/share/doc for reference (deb uses dpkg paths).
cp "$STAGE/systemd/install.sh" "$PKG/usr/share/doc/${NAME}/install.sh"
cp "$STAGE/systemd/uninstall.sh" "$PKG/usr/share/doc/${NAME}/uninstall.sh"
cp "$STAGE/doc/systemd.md" "$PKG/usr/share/doc/${NAME}/systemd.md"
cp README.md "$PKG/usr/share/doc/${NAME}/README.md"
cat >"$PKG/usr/share/doc/${NAME}/copyright" <<EOF
Format: https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/
Upstream-Name: neals
Source: https://github.com/pynza/neals

Files: *
Copyright: Neals contributors
License: MIT
EOF

INSTALLED_SIZE="$(du -sk "$PKG/usr" | awk '{print $1}')"

cat >"$PKG/DEBIAN/control" <<EOF
Package: ${NAME}
Version: ${VERSION}
Section: utils
Priority: optional
Architecture: ${ARCH_DEB}
Maintainer: Neals contributors <noreply@users.noreply.github.com>
Installed-Size: ${INSTALLED_SIZE}
Depends: libc6
Recommends: caddy
Homepage: https://github.com/pynza/neals
Description: Local devenv project orchestrator
 Neals registers devenv projects, runs them via nealsd, allocates loopback
 TCP ports, and reverse-proxies {service}.{project}.localhost with Caddy.
EOF

find "$PKG" -type d -exec chmod 755 {} +
chmod 755 "$PKG/DEBIAN"
chmod 644 "$PKG/DEBIAN/control"

dpkg-deb --root-owner-group --build "$PKG" "$DIST/${NAME}_${VERSION}_${ARCH_DEB}.deb"

# Top-level copies of install helpers (same for every arch; last build wins).
install -m755 contrib/systemd/install.sh "$DIST/install.sh"
install -m755 contrib/systemd/uninstall.sh "$DIST/uninstall.sh"

rm -rf "$STAGE" "$DEB_ROOT"

echo "==> checksums (all files currently in dist/)"
(
  cd "$DIST"
  # Exclude checksum file itself while regenerating.
  rm -f SHA256SUMS
  sha256sum ./* >SHA256SUMS
)

echo "==> done ($TARGET)"
ls -lh "$DIST"
