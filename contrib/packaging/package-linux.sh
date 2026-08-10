#!/usr/bin/env bash
# Build release binaries and package as .tar.gz, .zip, .deb, and .rpm for one arch.
# Does not publish to any store — artifacts land in ./dist for GitHub Releases.
#
# Usage:
#   ./contrib/packaging/package-linux.sh [VERSION] [TARGET]
# TARGET defaults to host triple (x86_64-unknown-linux-gnu or aarch64-unknown-linux-gnu).
# Set CLEAR_DIST=1 to wipe ./dist first (default: keep other arch artifacts).
# Requires: cargo, dpkg-deb, zip; nfpm (downloaded automatically if missing).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

# Pin so CI/local builds are reproducible. Bump intentionally.
NFPM_VERSION="2.41.3"

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

rpm_arch_for() {
  case "$1" in
    x86_64-unknown-linux-gnu) echo "x86_64" ;;
    aarch64-unknown-linux-gnu) echo "aarch64" ;;
    *)
      echo "error: no rpm arch mapping for $1" >&2
      exit 1
      ;;
  esac
}

ensure_nfpm() {
  if command -v nfpm >/dev/null 2>&1; then
    NFPM_BIN="$(command -v nfpm)"
    return 0
  fi
  local nfpm_arch url cache
  case "$(uname -m)" in
    x86_64|amd64) nfpm_arch="x86_64" ;;
    aarch64|arm64) nfpm_arch="arm64" ;;
    *)
      echo "error: cannot fetch nfpm for $(uname -m); install nfpm on PATH" >&2
      exit 1
      ;;
  esac
  local host_m
  case "$(uname -s)" in
    Linux) host_m="Linux" ;;
    Darwin) host_m="Darwin" ;;
    *)
      echo "error: cannot fetch nfpm for $(uname -s)" >&2
      exit 1
      ;;
  esac
  cache="${XDG_CACHE_HOME:-$HOME/.cache}/neals-nfpm/${NFPM_VERSION}"
  NFPM_BIN="$cache/nfpm"
  if [[ -x "$NFPM_BIN" ]]; then
    return 0
  fi
  url="https://github.com/goreleaser/nfpm/releases/download/v${NFPM_VERSION}/nfpm_${NFPM_VERSION}_${host_m}_${nfpm_arch}.tar.gz"
  mkdir -p "$cache"
  echo "==> fetching nfpm v${NFPM_VERSION}"
  curl -fsSL "$url" | tar -xz -C "$cache" nfpm
  chmod 755 "$NFPM_BIN"
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
ARCH_RPM="$(rpm_arch_for "$TARGET")"
HOST_TARGET="$(host_target)"

NAME="neals"
BASE="${NAME}-v${VERSION}-${TARGET}"
DIST="$ROOT/dist"
STAGE="$DIST/$BASE"
PKG_ROOT="$DIST/pkg-root-${ARCH_DEB}"
USR="$PKG_ROOT/usr"

if [[ "${CLEAR_DIST:-0}" == "1" ]]; then
  rm -rf "$DIST"
fi
rm -rf "$STAGE" "$PKG_ROOT"
mkdir -p "$STAGE"/{man,systemd,doc} "$USR" "$DIST"

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
cp contrib/systemd/README.md "$STAGE/doc/systemd.md"
cp README.md "$STAGE/README.md"

cat >"$STAGE/INSTALL.txt" <<EOF
Neals v${VERSION} (${TARGET})

Binaries: ./neals ./nealsd

This archive is for manual install (e.g. Arch). Prefer .deb / .rpm from
GitHub Releases when available — see README.md → Install / Manual install.

Quick:
  install -m755 neals nealsd ~/.local/bin/
  # or: sudo install -m755 neals nealsd /usr/local/bin/

System daemon (portless *.localhost on :80):
  sudo install -m644 systemd/nealsd@.service /etc/systemd/system/
  # edit ExecStart if binaries are not under /usr/local/bin
  sudo systemctl daemon-reload
  sudo systemctl enable --now "nealsd@\$USER"

Requires at runtime: Linux + systemd, nix, devenv, caddy on PATH
EOF

echo "==> archives"
tar -C "$DIST" -czf "$DIST/${BASE}.tar.gz" "$BASE"
(
  cd "$DIST"
  zip -qr "${BASE}.zip" "$BASE"
)

echo "==> stage /usr tree"
mkdir -p \
  "$USR/bin" \
  "$USR/share/man/man1" \
  "$USR/share/man/man8" \
  "$USR/lib/systemd/system" \
  "$USR/share/doc/${NAME}" \
  "$USR/share/bash-completion/completions"

install -m755 "$STAGE/neals" "$USR/bin/neals"
install -m755 "$STAGE/nealsd" "$USR/bin/nealsd"

cp "$STAGE/man/neals.1" "$USR/share/man/man1/"
cp "$STAGE/man/nealsd.8" "$USR/share/man/man8/"
gzip -9n "$USR/share/man/man1/neals.1" "$USR/share/man/man8/nealsd.8"

sed 's|/usr/local/bin/nealsd|/usr/bin/nealsd|;s|file:///usr/local/share/doc/neals/systemd.md|file:///usr/share/doc/neals/systemd.md|' \
  "$STAGE/systemd/nealsd@.service" \
  >"$USR/lib/systemd/system/nealsd@.service"

install -m644 contrib/packaging/deb/bash-completion \
  "$USR/share/bash-completion/completions/neals"

cp "$STAGE/doc/systemd.md" "$USR/share/doc/${NAME}/systemd.md"
cp README.md "$USR/share/doc/${NAME}/README.md"
cat >"$USR/share/doc/${NAME}/copyright" <<EOF
Format: https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/
Upstream-Name: neals
Source: https://github.com/pynza/neals

Files: *
Copyright: Neals contributors
License: MIT
EOF

echo "==> .deb ($ARCH_DEB)"
PKG="$PKG_ROOT/${NAME}_${VERSION}_${ARCH_DEB}"
mkdir -p "$PKG"
cp -a "$USR" "$PKG/usr"
mkdir -p "$PKG/DEBIAN"

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
Recommends: caddy, bash-completion
Homepage: https://github.com/pynza/neals
Description: Local devenv project orchestrator
 Neals registers devenv projects, runs them via nealsd, allocates loopback
 TCP ports, and reverse-proxies {service}.{project}.localhost with Caddy.
 On install (sudo dpkg -i / rpm -i), optionally enables nealsd@\$SUDO_USER
 and shell completion.
EOF

install -m755 contrib/packaging/deb/postinst "$PKG/DEBIAN/postinst"
install -m755 contrib/packaging/deb/prerm "$PKG/DEBIAN/prerm"
install -m755 contrib/packaging/deb/postrm "$PKG/DEBIAN/postrm"

find "$PKG" -type d -exec chmod 755 {} +
chmod 755 "$PKG/DEBIAN" "$PKG/DEBIAN/postinst" "$PKG/DEBIAN/prerm" "$PKG/DEBIAN/postrm"
chmod 644 "$PKG/DEBIAN/control"

dpkg-deb --root-owner-group --build "$PKG" "$DIST/${NAME}_${VERSION}_${ARCH_DEB}.deb"

echo "==> .rpm ($ARCH_RPM)"
ensure_nfpm
NFPM_CFG="$PKG_ROOT/nfpm.yaml"
cat >"$NFPM_CFG" <<EOF
name: ${NAME}
arch: ${ARCH_RPM}
platform: linux
version: ${VERSION}
release: "1"
section: utils
maintainer: Neals contributors <noreply@users.noreply.github.com>
description: |
  Local devenv project orchestrator.
  Registers devenv projects, runs them via nealsd, allocates loopback TCP
  ports, and reverse-proxies {service}.{project}.localhost with Caddy.
  On install, optionally enables nealsd@\$SUDO_USER and shell completion.
homepage: https://github.com/pynza/neals
license: MIT
depends:
  - glibc
recommends:
  - caddy
  - bash-completion
contents:
  - src: ${USR}
    dst: /usr
    type: tree
scripts:
  postinstall: ${ROOT}/contrib/packaging/deb/postinst
  preremove: ${ROOT}/contrib/packaging/deb/prerm
  postremove: ${ROOT}/contrib/packaging/deb/postrm
EOF

"$NFPM_BIN" package -f "$NFPM_CFG" -p rpm -t "$DIST/${NAME}-${VERSION}-1.${ARCH_RPM}.rpm"

rm -rf "$STAGE" "$PKG_ROOT"

echo "==> checksums (all files currently in dist/)"
(
  cd "$DIST"
  rm -f SHA256SUMS
  # Ignore helper dirs/binaries used only during packaging.
  find . -maxdepth 1 -type f ! -name SHA256SUMS -printf '%P\n' | sort | xargs -r sha256sum >SHA256SUMS
)

echo "==> done ($TARGET)"
ls -lh "$DIST"
