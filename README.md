# Neals

Local platform orchestrator for [devenv](https://devenv.sh) projects.

Neals keeps a registry of projects, starts and stops them through a daemon
(`nealsd`), allocates loopback TCP ports without collisions across projects,
exposes selected HTTP services at `{service}.{project}.localhost` via a
dedicated Caddy, and gives you a live view of services + logs plus a branded
project shell.

## Install

### Development (ad-hoc)

```bash
nix develop
cargo build -p neals -p nealsd
export PATH="$PWD/target/debug:$PATH"
```

Ad-hoc mode auto-starts `nealsd` on first use. Caddy listens on
`127.0.0.1:2015` — URLs need the port:

`http://api.demo.localhost:2015/`

### Recommended: system daemon (portless URLs)

One-time install grants `CAP_NET_BIND_SERVICE` only to `nealsd` (not a
machine-wide sysctl) so Caddy can bind `127.0.0.1:80`:

```bash
cargo build --release -p neals -p nealsd
sudo ./contrib/systemd/install.sh "$USER"
# later:
sudo ./contrib/systemd/uninstall.sh "$USER"            # keep user data
sudo ./contrib/systemd/uninstall.sh --purge "$USER"    # also wipe config/state
```

Then:

`http://api.demo.localhost/`

After install: `man neals`, `man nealsd`.

Details: [contrib/systemd/README.md](contrib/systemd/README.md).

**Requirements:** Linux + systemd, `nix`, `devenv`, `caddy` on PATH.

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

Two projects can declare the same preferred ports; `nealsd` assigns distinct free ports globally.

```bash
neals register
neals doctor
neals up demo          # live view: services (real ports) + logs
# Ctrl+C / q  → detach (keeps running)
# Ctrl+X      → stop project
neals status
neals bash demo        # quiet devenv shell; prompt shows neals:demo
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
| `neals bash <name>` | Interactive devenv shell (`$SHELL`) |
| `neals exec <name> -- …` | One-shot command in devenv |
| `neals doctor` | Check tools, dirs, bind, daemon |
| `neals repl` | Interactive command loop |
| `neals completions <shell>` | Print completion snippet (also offered by `install.sh` as y/N) |

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

Declare preferred start ports (not final ports). `nealsd` picks the first free
port at or above the preferred value, globally across all running projects.

| Declaration | Meaning |
|-------------|---------|
| `services.redis.port = 6379` | Lease TCP ≥ 6379; inject `NEALS_REDIS_PORT`; no Caddy |
| `services.api = { port = 8000; proxy = true; }` | Same + reverse proxy `api.<project>.localhost` |
| `services.backend.socket = "backend.sock"` | UNIX socket under `NEALS_RUNTIME` + Caddy |

Legacy `neals.route.<name> = "tcp" | "*.sock"` still works (ephemeral TCP +
proxy, or UNIX) but is deprecated — prefer `neals.services`.

## What Neals injects

On `neals up`, before processes start:

| `devenv.nix` | Environment |
|--------------|-------------|
| `services.redis.port = 6379` | `NEALS_REDIS_PORT=<assigned>` |
| `services.api-backend = { port = 8000; proxy = true; }` | `NEALS_API_BACKEND_PORT=<assigned>` |
| UNIX sockets | `NEALS_RUNTIME` — bind socket files there |

Service names: uppercase, `-` → `_`. Apps must bind **exactly**
`127.0.0.1` at that port (not `0.0.0.0`).

`neals up` / `neals status` show the **assigned** ports, e.g.
`redis → 127.0.0.1:6380` or `http://api.demo.localhost:2015/ → 127.0.0.1:8001`.

Public URL shape (proxy services only):
`http://{service}.{project}.localhost[:caddy-port]/`.

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
| `neals-v*-x86_64-unknown-linux-gnu.tar.gz` / `.zip` | bins + man + `systemd/install.sh` |
| `neals-v*-aarch64-unknown-linux-gnu.tar.gz` / `.zip` | same for arm64 |
| `neals_*_amd64.deb` / `neals_*_arm64.deb` | system package under `/usr` |
| `install.sh` / `uninstall.sh` | same helpers (use from an extracted archive) |
| `SHA256SUMS` | checksums |

```bash
# From a release archive (portless :80 daemon):
tar xf neals-v0.1.0-x86_64-unknown-linux-gnu.tar.gz
cd neals-v0.1.0-x86_64-unknown-linux-gnu
sudo ./systemd/install.sh "$USER"
```

```bash
git tag -a v0.1.0 -m "v0.1.0"
git push origin v0.1.0
```

Local package smoke test (host arch → `./dist`):

```bash
./contrib/packaging/package-linux.sh 0.1.0
```
