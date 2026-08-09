#!/usr/bin/env bash
# Install neals + nealsd, man pages, docs, and enable the system daemon for one user.
# Requires: root, systemd, caddy on PATH for that user.
#
# Works from:
#   - git checkout:  cargo build --release && sudo ./contrib/systemd/install.sh "$USER"
#   - release archive: tar xf neals-v*-*.tar.gz && cd neals-v*-* && sudo ./systemd/install.sh "$USER"
#
# Optional: PREFIX=/usr/local (default), BIN_DIR=… override binary source dir.
# Shell completion prompt: [y/N]. Non-interactive: NEALS_INSTALL_COMPLETIONS=y|n.
set -euo pipefail

USER_NAME="${1:-${SUDO_USER:-$USER}}"
PREFIX="${PREFIX:-/usr/local}"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

resolve_layout() {
  # Release tarball/zip: systemd/ next to neals + nealsd + man/
  if [[ -x "$SCRIPT_DIR/../neals" && -x "$SCRIPT_DIR/../nealsd" && -f "$SCRIPT_DIR/nealsd@.service" ]]; then
    PKG_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
    BIN_DIR="${BIN_DIR:-$PKG_ROOT}"
    UNIT_SRC="$SCRIPT_DIR/nealsd@.service"
    DOC_SRC="$PKG_ROOT/doc/systemd.md"
    [[ -f "$DOC_SRC" ]] || DOC_SRC="$SCRIPT_DIR/README.md"
    MAN1_SRC="$PKG_ROOT/man/neals.1"
    MAN8_SRC="$PKG_ROOT/man/nealsd.8"
    UNINSTALL_HINT="$SCRIPT_DIR/uninstall.sh"
    return
  fi

  # Git checkout: contrib/systemd/
  if [[ -f "$SCRIPT_DIR/nealsd@.service" && -f "$SCRIPT_DIR/../man/neals.1" ]]; then
    ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
    if [[ -z "${BIN_DIR:-}" ]]; then
      if command -v cargo >/dev/null 2>&1; then
        local target_dir
        target_dir="$(
          cd "$ROOT" && cargo metadata --no-deps --format-version 1 2>/dev/null \
            | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])' 2>/dev/null \
            || true
        )"
        BIN_DIR="${target_dir:-$ROOT/target}/release"
      else
        BIN_DIR="$ROOT/target/release"
      fi
    fi
    UNIT_SRC="$SCRIPT_DIR/nealsd@.service"
    DOC_SRC="$SCRIPT_DIR/README.md"
    MAN1_SRC="$SCRIPT_DIR/../man/neals.1"
    MAN8_SRC="$SCRIPT_DIR/../man/nealsd.8"
    UNINSTALL_HINT="$SCRIPT_DIR/uninstall.sh"
    return
  fi

  echo "cannot find release archive or git checkout layout next to $0" >&2
  echo "  from release: extract the .tar.gz/.zip and run: sudo ./systemd/install.sh" >&2
  echo "  from git:     cargo build --release -p neals -p nealsd && sudo ./contrib/systemd/install.sh" >&2
  exit 1
}

# Marker block written into the user's shell rc (removed by uninstall.sh).
COMPLETION_BEGIN="# >>> neals completions >>>"
COMPLETION_END="# <<< neals completions <<<"

ask_yes_no() {
  local prompt="$1"
  local reply
  # Prefer the controlling terminal (works under sudo / pipes).
  if [[ -r /dev/tty ]]; then
    read -r -p "$prompt" reply </dev/tty || reply=n
  elif [[ -t 0 ]]; then
    read -r -p "$prompt" reply || reply=n
  else
    # Non-interactive: NEALS_INSTALL_COMPLETIONS=y|n (default n).
    case "${NEALS_INSTALL_COMPLETIONS:-n}" in
      y|Y|yes|YES) return 0 ;;
      *) return 1 ;;
    esac
  fi
  case "$reply" in
    y|Y|yes|YES) return 0 ;;
    *) return 1 ;;
  esac
}

setup_shell_completion() {
  local login_shell shell_name rc snippet
  login_shell="$(getent passwd "$USER_NAME" | cut -d: -f7)"
  shell_name="$(basename "$login_shell")"

  case "$shell_name" in
    bash)
      rc="$user_home/.bashrc"
      snippet='source <(COMPLETE=bash neals)'
      ;;
    zsh)
      rc="$user_home/.zshrc"
      snippet='source <(COMPLETE=zsh neals)'
      ;;
    fish)
      rc="$user_home/.config/fish/config.fish"
      snippet='COMPLETE=fish neals | source'
      ;;
    *)
      echo
      echo "shell completion: login shell is $shell_name — skip auto-setup."
      echo "  enable later:  $PREFIX/bin/neals completions bash|zsh|fish"
      return
      ;;
  esac

  if [[ -f "$rc" ]] && grep -qF "$COMPLETION_BEGIN" "$rc" 2>/dev/null; then
    echo
    echo "shell completion: already present in $rc"
    return
  fi

  echo
  if ! ask_yes_no "Enable shell completion for $shell_name ($rc)? [y/N] "; then
    echo "shell completion: skipped (later: $PREFIX/bin/neals completions $shell_name)"
    return
  fi

  if [[ "$shell_name" == "fish" ]]; then
    install -d -o "$USER_NAME" -g "$(id -gn "$USER_NAME")" "$(dirname "$rc")"
  fi
  # Create rc if missing so first-time fish/zsh users still get the block.
  if [[ ! -e "$rc" ]]; then
    touch "$rc"
    chown "$USER_NAME:$(id -gn "$USER_NAME")" "$rc"
    chmod 644 "$rc"
  fi

  {
    echo ""
    echo "$COMPLETION_BEGIN"
    echo "$snippet"
    echo "$COMPLETION_END"
  } >>"$rc"
  chown "$USER_NAME:$(id -gn "$USER_NAME")" "$rc"

  echo "shell completion: added to $rc (open a new shell or: source $rc)"
}

resolve_layout

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
  src="$BIN_DIR/$bin"
  if [[ ! -x "$src" ]]; then
    echo "missing $src" >&2
    echo "  build first: cargo build --release -p neals -p nealsd" >&2
    echo "  or extract a release archive that contains the binaries" >&2
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

setup_shell_completion

echo
echo "nealsd@${USER_NAME} enabled."
echo "  socket: /run/neals/nealsd.sock"
echo "  HTTP:   http://{service}.{project}.localhost/  (port 80)"
echo "  man:    man neals    man nealsd"
echo
echo "Try:  sudo -u $USER_NAME $PREFIX/bin/neals doctor"
echo "      sudo -u $USER_NAME $PREFIX/bin/neals status"
echo
echo "Uninstall: sudo ${UNINSTALL_HINT} $USER_NAME"
echo "           sudo ${UNINSTALL_HINT} --purge $USER_NAME"
