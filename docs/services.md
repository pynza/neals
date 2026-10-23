# `neals.services`

Declare preferred ports for binds **inside** the project network namespace.
On the host, `nealsd` gives each TCP service a dedicated `127.53.x.y` address
and proxies the preferred port into the guest (so two projects can both expose
`:3306` without colliding).

| Declaration | Meaning |
|-------------|---------|
| `services.redis.port = 6379` | Guest binds `:6379`; host `127.53.x.y:6379`; no Caddy |
| `services.api = { port = 8000; proxy = true; }` | Same + reverse proxy `api.<project>.localhost` |
| `services.backend.socket = "backend.sock"` | UNIX socket under `NEALS_RUNTIME` + Caddy |

Legacy `neals.route.<name> = "tcp" | "*.sock"` still works (ephemeral TCP +
proxy, or UNIX) but is deprecated — prefer `neals.services`.

## Hostnames (hybrid DNS)

| Where | Name | Resolves to |
|-------|------|-------------|
| Inside the project netns | `{service}` (e.g. `db`, `mailpit`) | `127.0.0.1` |
| Inside the project netns | `{project}_{service}` | `127.0.0.1` |
| On the host | `{project}_{service}` only | `127.53.x.y` |

Wire apps with the short name in-guest (`DB_HOST=db`, `MAIL_HOST=mailpit`) and
the long name from host tools (`mysql -h laravel-nuxt_db`). Use the preferred
service port in both places.

`nealsd` writes `~/.local/state/neals/hosts` and mirrors `/run/neals/hosts`.
With the packaged install, `neals-hosts-sync.path` watches that mirror and
merges `/etc/hosts` as root (no manual sudo after setup). Fallback:

```bash
sudo neals hosts apply
# or: sudo neals hosts apply --from /run/neals/hosts
```

`neals hosts` shows the fragment; `neals doctor` checks the path unit and sync.
Override the system file path with `NEALS_HOSTS_FILE` (tests / custom setups).

## What Neals injects

On `neals up`, before processes start:

| `devenv.nix` | Environment |
|--------------|-------------|
| `services.redis.port = 6379` | `NEALS_REDIS_PORT=6379` (guest port) |
| `services.api-backend = { port = 8000; proxy = true; }` | `NEALS_API_BACKEND_PORT=8000` |
| UNIX sockets | `NEALS_RUNTIME` — bind socket files there |

Service names: uppercase, `-` → `_`. Apps must bind **exactly**
`127.0.0.1` at the guest port (not `0.0.0.0`).

`neals up` / `neals status` show **host** endpoints
(`demo_redis → 127.53.0.1:6379`).

Public URL shape (proxy services only):
`http://{service}.{project}.localhost[:caddy-port]`.

`neals bash` / `neals exec` enter the project's user, net, and mount namespaces,
so inside the shell the guest ports, short DNS names, and `/run` apply.

## HTTP modes

| Mode | How | Listen | Browser URL |
|------|-----|--------|-------------|
| System | `nealsd@.service` | prefers `:80`, else `:2015`+ | no port if `:80` |
| Ad-hoc | CLI auto-start / `cargo run -p nealsd` | same preference (`:80` usually fails without CAP) | `:{port}` if not 80 |

Override with `NEALS_CADDY_HTTP_ADDR` (no auto-fallback).
