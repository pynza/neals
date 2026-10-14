# Neals

Local platform orchestrator for [devenv](https://devenv.sh) projects.

https://github.com/user-attachments/assets/6bed70a0-1ba7-4c77-afd1-c9acb5ebc896

Neals runs each project in its own network namespace, leases host loopback
ports without collisions, reverse-proxies into guest `127.0.0.1` binds, and
exposes selected HTTP services at `{service}.{project}.localhost` via Caddy —
with multiplexed process logs and a project shell in the same namespace.

## Install

Linux + systemd. Needs `bwrap`, `slirp4netns`, [nix](https://nixos.org),
[devenv](https://devenv.sh), and [caddy](https://caddyserver.com) on `PATH`.

From [GitHub Releases](https://github.com/pynza/neals/releases):

```bash
# Debian / Ubuntu
sudo apt install ./neals_*_amd64.deb          # or *_arm64.deb

# Fedora / RHEL / openSUSE
sudo dnf install ./neals-*-1.x86_64.rpm       # or *.aarch64.rpm
```

Post-install can enable `nealsd@$USER` (Caddy on `:80`) and shell completion.
Manual / Arch / ad-hoc / packaging details: [docs/install.md](docs/install.md).

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
      redis.port = 6379;                      # private TCP (no Caddy)
      api = { port = 8000; proxy = true; };   # http://api.demo.localhost
      backend.socket = "backend.sock";        # UNIX under NEALS_RUNTIME + Caddy
    };
  };

  # Neals allocates + injects env; the process must bind that port:
  # processes.redis.exec = ''exec redis-server --port "$NEALS_REDIS_PORT" --bind 127.0.0.1'';
}
```

Wire the app yourself (Neals does **not** edit `.env`):

```dotenv
REDIS_HOST=127.0.0.1
REDIS_PORT=${NEALS_REDIS_PORT}
```

```bash
neals init             # skip if the neals block is already there
neals register
neals doctor
neals up demo          # prints routes, then follows process logs
# Ctrl+Q detach · Ctrl+B shell · Ctrl+C / X stop
neals status
neals bash demo        # same netns (project must be up)
neals down demo
```

More layouts: [docs/examples/](docs/examples/). Guest vs host ports, proxy
rules, and env injection: [docs/services.md](docs/services.md).

## Docs

| | |
|--|--|
| [Install](docs/install.md) | Requirements, deb/rpm, manual, ad-hoc |
| [Usage](docs/usage.md) | Commands, logs, refresh, `bash` / `exec`, paths |
| [Services](docs/services.md) | `neals.services`, env, HTTP modes |
| [Examples](docs/examples/) | Reference `devenv.nix` snippets |
| [Develop](docs/develop.md) | Build, test, releases |
| [Agent skill](docs/agent-skills/neals/) | Portable skill for coding agents ([agentskills.io](https://agentskills.io)) — copy to e.g. `~/.cursor/skills/neals/` |
