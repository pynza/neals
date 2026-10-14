# Install

## Requirements

- Linux with systemd
- [`bubblewrap`](https://github.com/containers/bubblewrap) (`bwrap`) and
  [`slirp4netns`](https://github.com/rootless-containers/slirp4netns)
  (deb/rpm pull these in via Depends); unprivileged user namespaces enabled
  on the host
- [`nix`](https://nixos.org) and [`devenv`](https://devenv.sh) for projects
- [`caddy`](https://caddyserver.com) on PATH (or under `~/.nix-profile/bin`)
- For system (portless) mode: `127.0.0.1:80` free

## Plug and play (deb / rpm)

From [GitHub Releases](https://github.com/pynza/neals/releases):

```bash
# Debian / Ubuntu (resolves Depends in one step)
sudo apt install ./neals_*_amd64.deb          # or *_arm64.deb

# Fedora / RHEL / openSUSE
sudo dnf install ./neals-*-1.x86_64.rpm       # or *.aarch64.rpm
```

`dpkg -i` / `rpm -i` also work; if Depends are missing, run
`sudo apt-get install -f` (or install `bubblewrap` + `slirp4netns` by hand).

Post-install asks `[Y/n]` to enable `nealsd@$SUDO_USER` (Caddy on `:80`,
portless `http://{service}.{project}.localhost/`) and shell completion.
Overrides: `NEALS_DEB_SETUP=0` skip, `NEALS_DEB_SETUP=y` auto-yes
(also `NEALS_PKG_SETUP`).

```bash
NEALS_DEB_SETUP=0 sudo apt install ./neals_*_amd64.deb
sudo dpkg -r neals          # remove
sudo dpkg -P neals          # purge
sudo rpm -e neals
```

From this tree (no GitHub Release): `./contrib/packaging/install-local.sh`
builds a host-arch `.deb` and `dpkg -i`s it (`--reinstall` equivalent, same
`/usr` layout and postinst). Skip daemon setup with `NEALS_DEB_SETUP=0`.

Unit details: [contrib/systemd/README.md](../contrib/systemd/README.md).

## Manual install (Arch / tarball / from source)

```bash
# Distro deps (Arch example):
sudo pacman -S bubblewrap slirp4netns

# From a release .tar.gz / .zip, or after cargo build --release:
install -m755 neals nealsd ~/.local/bin/
# or: sudo install -m755 neals nealsd /usr/local/bin/   # or /usr/bin/

sudo install -m644 contrib/systemd/nealsd@.service /etc/systemd/system/
# set ExecStart= to your nealsd path if not /usr/local/bin/nealsd
sudo systemctl daemon-reload
sudo systemctl enable --now "nealsd@$USER"
```

If `caddy` is only under nix, add a PATH drop-in (`systemctl edit nealsd@$USER`):

```ini
[Service]
Environment=PATH=%h/.nix-profile/bin:/usr/local/bin:/usr/bin:/bin
```

Autocomplete: bash via `/usr/share/bash-completion/completions/neals` if you
install that file, or add `neals completions bash|zsh|fish` to your shell rc.

Disable later: `sudo systemctl disable --now "nealsd@$USER"`.

## Development (ad-hoc)

```bash
nix develop
cargo build -p neals -p nealsd
export PATH="$PWD/target/debug:$PATH"
```

Ad-hoc mode auto-starts `nealsd` on first use. Caddy prefers `:80`, then
`:2015`, then the next free port — URLs need the port unless Caddy got `:80`:
`http://api.demo.localhost:2015/`
