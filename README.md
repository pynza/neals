# Neals

Local platform orchestrator for [devenv](https://devenv.sh) projects.

Neals keeps a registry of projects, starts and stops them through a daemon
(`nealsd`), runs each project in its own network namespace, allocates host
loopback TCP ports without collisions, reverse-proxies into the guest
`127.0.0.1` binds, exposes selected HTTP services at
`{service}.{project}.localhost` via Caddy, and gives you a live view plus a
branded project shell in the same namespace.

## Install

### Requirements

- Linux with systemd
- [`bubblewrap`](https://github.com/containers/bubblewrap) (`bwrap`) and
  [`slirp4netns`](https://github.com/rootless-containers/slirp4netns)
  (deb/rpm pull these in via Depends); unprivileged user namespaces enabled
  on the host
- [`nix`](https://nixos.org) and [`devenv`](https://devenv.sh) for projects
- [`caddy`](https://caddyserver.com) on PATH (or under `~/.nix-profile/bin`)
- For system (portless) mode: `127.0.0.1:80` free

### Plug and play (deb / rpm)

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

Unit details: [contrib/systemd/README.md](contrib/systemd/README.md).

### Manual install (Arch / tarball / from source)

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

### Development (ad-hoc)

```bash
nix develop
cargo build -p neals -p nealsd
export PATH="$PWD/target/debug:$PATH"
```

Ad-hoc mode auto-starts `nealsd` on first use. Caddy listens on
`127.0.0.1:2015` — URLs need the port: `http://api.demo.localhost:2015/`

## Quick start

In a project with `devenv.nix`:

```nix
{ pkgs, lib, ... }: {
  imports = [
    ({ lib, ... }: {
      options.neals = lib.mkOption {
        type = lib.types.attrs;
        default = { };
      };
    })
  ];

  neals = {
    name = "demo";
    services = {
      redis.port = 6379;                        # preferred start; private (no Caddy)
      api = { port = 8000; proxy = true; };     # preferred + http://api.demo.localhost
      backend.socket = "backend.sock";          # UNIX + Caddy
    };
  };

  # Neals only allocates + injects env; the process must bind that port:
  # processes.redis.exec = ''exec redis-server --port "$NEALS_REDIS_PORT" --bind 127.0.0.1'';
}
```

In the app `.env` (Neals does **not** edit this file):

```dotenv
REDIS_HOST=127.0.0.1
REDIS_PORT=${NEALS_REDIS_PORT}
```

Two projects can declare the same preferred ports; inside each netns the
preferred port stays fixed, while `nealsd` leases distinct **host** ports
and bridges them into the guest.

```bash
neals register
neals doctor
neals up demo          # live view: services (real ports) + logs
# Ctrl+C / q  → detach (keeps running)
# Ctrl+X      → stop project
neals status
neals bash demo        # same netns as the running project (must be up)
neals down demo
```

Or use the interactive loop: `neals repl`.

## Commands

| Command | What it does |
|---------|----------------|
| `neals register` | Add current directory to the registry |
| `neals list` | Show registered projects |
| `neals unregister <name>` | Remove from registry |
| `neals prune` | Drop ghost entries (missing paths) |
| `neals up <name> [-d]` | Start project; live view unless `-d` |
| `neals down <name>` | Stop project |
| `neals status` | Running projects, PIDs, services (real ports) |
| `neals logs <name> [-f]` | Tail logs; `-f` opens live view |
| `neals bash <name>` | Shell in the project's netns (project must be up) |
| `neals exec <name> -- …` | One-shot command in that netns + devenv |
| `neals doctor` | Check tools, dirs, bind, daemon |
| `neals repl` | Interactive command loop |
| `neals completions <shell>` | Print completion snippet for shell rc |

Global: `-y` / `--yes` skips confirmations. `neals --help` for full text.

### Live view keys

| Key | Action |
|-----|--------|
| `Ctrl+C` or `q` | Detach; project keeps running |
| `Ctrl+X` | Stop the project and leave |

### Project shell

`neals bash` respects `$SHELL`, runs devenv quietly, and for bash/zsh sets a
short prompt `neals:<project> …`. Use `neals status` / live view for services.

## Directories & data

| Path | Role |
|------|------|
| `~/.config/neals/projects.json` | Project registry (name → path) |
| `~/.local/state/neals/<project>.log` | Project stdout/stderr from `devenv up` |
| `~/.local/state/neals/nealsd.log` | Ad-hoc daemon log |
| `~/.local/state/neals/caddy.json` | Generated Caddy config |
| `~/.local/state/neals/caddy.log` | Caddy log |
| `$XDG_RUNTIME_DIR/neals/` | Ad-hoc IPC socket + per-project runtime |
| `/run/neals/nealsd.sock` | System daemon IPC (if installed) |
| `<project>/.neals/*.sock` | Convenience symlinks to UNIX sockets |

Override IPC with `NEALS_SOCKET`. Override HTTP listen with
`NEALS_CADDY_HTTP_ADDR` (e.g. `127.0.0.1:8080`).

## `neals.services` API

Declare preferred ports for binds **inside** the project network namespace.
On the host, `nealsd` leases a free loopback port (starting at the preferred
value when available) and proxies into the guest.

| Declaration | Meaning |
|-------------|---------|
| `services.redis.port = 6379` | Guest binds `:6379`; host lease ≥ 6379; no Caddy |
| `services.api = { port = 8000; proxy = true; }` | Same + reverse proxy `api.<project>.localhost` |
| `services.backend.socket = "backend.sock"` | UNIX socket under `NEALS_RUNTIME` + Caddy |

Legacy `neals.route.<name> = "tcp" | "*.sock"` still works (ephemeral TCP +
proxy, or UNIX) but is deprecated — prefer `neals.services`.

## What Neals injects

On `neals up`, before processes start:

| `devenv.nix` | Environment |
|--------------|-------------|
| `services.redis.port = 6379` | `NEALS_REDIS_PORT=6379` (guest port) |
| `services.api-backend = { port = 8000; proxy = true; }` | `NEALS_API_BACKEND_PORT=8000` |
| UNIX sockets | `NEALS_RUNTIME` — bind socket files there |

Service names: uppercase, `-` → `_`. Apps must bind **exactly**
`127.0.0.1` at the guest port (not `0.0.0.0`).

`neals up` / `neals status` show **host** ports (what Caddy and tools use).
If the host lease differs from the preferred guest port:
`redis → 127.0.0.1:6380 (guest :6379)`.

Public URL shape (proxy services only):
`http://{service}.{project}.localhost[:caddy-port]/`.

From the host, connect to the **host** port (e.g. `redis-cli -p 6380`).
`neals bash` / `neals exec` enter the project netns, so inside the shell the
guest ports apply.
## HTTP modes

| Mode | How | Listen | Browser URL |
|------|-----|--------|-------------|
| System | `nealsd@.service` | `127.0.0.1:80` | no port |
| Ad-hoc | CLI auto-start / `cargo run -p nealsd` | `127.0.0.1:2015` | `:2015` |

Set `NEALS_MODE=system` (done by the unit) for the :80 default.

## Develop Neals itself

```bash
nix develop
cargo test
cargo build -p neals -p nealsd
cargo run -p neals -- --help
```

## Releases

Pushing an annotated tag `v*` runs [.github/workflows/release.yml](.github/workflows/release.yml):
builds **Linux amd64 + arm64** packages and publishes a GitHub Release.
Nothing is pushed to apt/crates.io/Homebrew — only GitHub Release files.

| Asset | Contents |
|-------|----------|
| `neals-v*-x86_64-unknown-linux-gnu.tar.gz` / `.zip` | bins + man + unit (manual install) |
| `neals-v*-aarch64-unknown-linux-gnu.tar.gz` / `.zip` | same for arm64 |
| `neals_*_amd64.deb` / `neals_*_arm64.deb` | `/usr` + postinst hooks |
| `neals-*-1.x86_64.rpm` / `neals-*-1.aarch64.rpm` | same tree + same hooks |
| `SHA256SUMS` | checksums |

```bash
git tag -a v0.1.0 -m "v0.1.0"
git push origin v0.1.0
```

Local package smoke test (host arch → `./dist`):

```bash
CLEAR_DIST=1 ./contrib/packaging/package-linux.sh 0.1.0
```
