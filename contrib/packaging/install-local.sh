#!/usr/bin/env bash
# Build a host-arch .deb from this tree and install it like a GitHub Release
# (files under /usr, same postinst/prerm/postrm). No tag or release needed.
#
# Usage:
#   ./contrib/packaging/install-local.sh [VERSION]
# VERSION defaults to crates/cli/Cargo.toml.
#
# Needs: cargo, dpkg-deb, sudo; apt-get recommended (resolves Depends).
# Postinst prompts like the packaged install. Overrides:
#   NEALS_DEB_SETUP=0 skip daemon/completion
#   NEALS_DEB_SETUP=y auto-yes
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

if [[ "$(id -u)" -eq 0 ]]; then
  echo "error: run as your user (script uses sudo). postinst needs SUDO_USER." >&2
  exit 1
fi

need() { command -v "$1" >/dev/null || { echo "error: missing $1" >&2; exit 1; }; }
need cargo
need dpkg-deb
need sudo

case "$(uname -m)" in
  x86_64|amd64) ARCH_DEB=amd64 ;;
  aarch64|arm64) ARCH_DEB=arm64 ;;
  *)
    echo "error: unsupported host arch $(uname -m)" >&2
    exit 1
    ;;
esac

echo "==> package .deb (host arch, skip zip/rpm)"
DEB_ONLY=1 "$ROOT/contrib/packaging/package-linux.sh" "$@"

if [[ -n "${1:-}" ]]; then
  VERSION="${1#v}"
else
  VERSION="$(sed -n 's/^version = "\([^"]*\)"/\1/p' crates/cli/Cargo.toml | head -1)"
fi
DEB="$ROOT/dist/neals_${VERSION}_${ARCH_DEB}.deb"
if [[ ! -f "$DEB" ]]; then
  echo "error: missing $DEB" >&2
  ls -la "$ROOT/dist" >&2 || true
  exit 1
fi
echo "==> install $DEB"

# dpkg -i overwrites the same version (apt without --reinstall would skip).
sudo --preserve-env=NEALS_DEB_SETUP,NEALS_PKG_SETUP,DEBIAN_FRONTEND \
  dpkg -i "$DEB" && exit_dpkg=0 || exit_dpkg=$?
if [[ "$exit_dpkg" -ne 0 ]]; then
  if command -v apt-get >/dev/null 2>&1; then
    echo "==> apt-get install -f (unmet Depends)"
    sudo --preserve-env=NEALS_DEB_SETUP,NEALS_PKG_SETUP,DEBIAN_FRONTEND \
      apt-get install -f -y
  else
    echo "error: dpkg -i failed; install bubblewrap + slirp4netns and retry" >&2
    exit "$exit_dpkg"
  fi
fi

echo "==> which neals: $(command -v neals || echo 'not on PATH')"
neals -V || true
echo "done. same as a release .deb; remove with: sudo apt remove neals  (or dpkg -P neals)"
