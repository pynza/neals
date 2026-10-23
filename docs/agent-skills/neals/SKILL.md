---
name: neals
description: >-
  Local platform orchestrator for devenv projects (neals / nealsd): netns,
  127.53.x.y host endpoints, hybrid DNS (short in-guest / project_service on
  host), Caddy routes at {service}.{project}.localhost, logs and project
  shell. Use when the user mentions neals, nealsd, neals up/down, *.localhost
  project URLs, NEALS_*_PORT, devenv.nix neals block, or running devenv apps
  under Neals.
---

# Neals

Linux-only orchestrator for [devenv](https://devenv.sh) projects. A daemon
(`nealsd`) registers projects, starts each in its own network namespace,
assigns each TCP service a `127.53.x.y` host endpoint on the preferred port,
reverse-proxies into guest `127.0.0.1` binds, and exposes selected HTTP
services as `http://{service}.{project}.localhost` via Caddy.

Upstream docs (Neals repository): `README.md`, `docs/`, `docs/examples/`.

## Mental model

- **Guest vs host**: preferred ports in `devenv.nix` bind *inside* the netns
  on `127.0.0.1`. Host proxies listen on a unique `127.53.x.y` at the same
  preferred port. `neals status` shows host endpoints.
- **DNS**: in-guest short name (`db`) + `{project}_{service}`; on the host
  only the long name (`neals-hosts-sync.path` merges `/etc/hosts`, or
  `sudo neals hosts apply`).
- **Proxy**: only services with `proxy = true` (or UNIX socket) get a Caddy
  hostname. Plain `port` services are private (host DNS / `neals bash`).
- **Binds**: apps must listen on `127.0.0.1` at the guest port (not `0.0.0.0`).
- **Env**: Neals injects `NEALS_<SERVICE>_PORT` (and `NEALS_RUNTIME` for
  sockets). It does **not** edit `.env`; wire e.g. `DB_HOST=db` +
  `${NEALS_DB_PORT}`.
- **Daemon**: systemd `nealsd@$USER` (system mode, Caddy often on `:80`) or
  ad-hoc auto-start from the CLI (URLs may need `:{port}` if not on 80).

## Typical workflow

```bash
neals init             # add neals block if missing
neals register
neals doctor
neals up <name>        # routes + follow logs (-d to detach)
neals status
neals bash <name>      # interactive shell in project netns (must be up)
neals exec <name> -- … # non-interactive command in that netns
neals down <name>
```

Log follow (`neals up` / `neals logs -f`): `Ctrl+Q` detach, `Ctrl+B` shell,
`Ctrl+C` / `Ctrl+X` stop project.

## `devenv.nix` (services)

```nix
neals = {
  name = "demo";
  services = {
    redis.port = 6379;                      # private TCP — no *.localhost
    api = { port = 8000; proxy = true; };   # http://api.demo.localhost
    backend.socket = "backend.sock";        # UNIX in $NEALS_RUNTIME + Caddy
  };
};
```

Env names: service → uppercase, `-` → `_` (`api-backend` → `NEALS_API_BACKEND_PORT`).

## Commands (cheat sheet)

| Command | Role |
|---------|------|
| `init` / `register` / `list` / `unregister` / `prune` | Registry |
| `up` / `down` / `refresh` (`--update`, `--hard`) | Lifecycle |
| `status` / `logs` / `info` / `doctor` | Inspect |
| `hosts` / `hosts apply` | Show / install host DNS fragment |
| `bash` / `exec -- …` | Enter netns (interactive / one-shot) |

Global: `-y` / `--yes` skips confirmations.

## Paths & overrides

| Path | Role |
|------|------|
| `~/.config/neals/projects.json` | Registry |
| `~/.local/state/neals/` | Logs, Caddy config |
| `/run/neals/nealsd.sock` | System IPC |
| `$XDG_RUNTIME_DIR/neals/` | Ad-hoc IPC + runtime |

`NEALS_SOCKET`, `NEALS_CADDY_HTTP_ADDR` override IPC / Caddy listen.

## Agent guidance

- Prefer `neals status`, `neals doctor`, and `neals logs` before inventing
  docker-compose / manual port forwards for a Neals-managed project.
- `bash` / `exec` require the project **up**.
- After changing `neals.services`, `refresh` or `up` again.
- For scripts/CI use `neals exec <name> -- CMD …` (literal argv). For a shell
  pipeline inside the netns: `neals exec <name> -- bash -lc '…'`.
- Do not assume host port == preferred guest port; read `neals status`.
- Do not bind services to `0.0.0.0` under Neals; use `127.0.0.1` + injected ports.
- `refresh --hard` wipes `.devenv` / `.neals` (and DB data under `.devenv`);
  not source, `devenv.lock`, or `.env`.
