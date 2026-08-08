# Neals

Local platform orchestrator for [devenv](https://devenv.sh) projects.

Neals keeps a registry of projects, starts and stops them through a daemon
(`nealsd`), exposes HTTP services at `{service}.{project}.localhost` via a
dedicated Caddy, and gives you a live view of routes + logs plus a branded
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
    route = {
      backend = "backend.sock";  # UNIX socket
      api = "tcp";               # dynamic loopback port
    };
  };
}
```

```bash
neals register
neals doctor
neals up demo          # live view: sticky routes + logs
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
| `neals status` | Running projects, PIDs, routes |
| `neals logs <name> [-f]` | Tail logs; `-f` opens live view |
| `neals bash <name>` | Interactive devenv shell (`$SHELL`) |
| `neals exec <name> -- …` | One-shot command in devenv |
| `neals doctor` | Check tools, dirs, bind, daemon |
| `neals repl` | Interactive command loop |
| `neals completions <shell>` | Print completion snippet |

Global: `-y` / `--yes` skips confirmations. `neals --help` for full text.

### Live view keys

| Key | Action |
|-----|--------|
| `Ctrl+C` or `q` | Detach; project keeps running |
| `Ctrl+X` | Stop the project and leave |

### Project shell

`neals bash` respects `$SHELL`, runs devenv quietly, and for bash/zsh sets a
short prompt `neals:<project> …`. Use `neals status` / live view for routes.

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

## What Neals injects

On `neals up`, for each declared route:

| `devenv.nix` | Environment |
|--------------|-------------|
| `neals.route.api = "tcp"` | `NEALS_PORT_API`, `NEALS_LISTEN_API=127.0.0.1:<port>` |
| `neals.route.api-backend = "tcp"` | `NEALS_PORT_API_BACKEND`, `NEALS_LISTEN_API_BACKEND=…` |
| UNIX routes | `NEALS_RUNTIME` pointing at the runtime dir; bind sockets there |

Service names: uppercase, `-` → `_`. Apps must bind **exactly**
`127.0.0.1` at that port (not `0.0.0.0`).

Public URL shape: `http://{service}.{project}.localhost[:port]/`.

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
