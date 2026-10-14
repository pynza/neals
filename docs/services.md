# `neals.services`

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
`neals bash` / `neals exec` enter the project's user, net, and mount namespaces,
so inside the shell the guest ports, DNS, and `/run` apply.

## HTTP modes

| Mode | How | Listen | Browser URL |
|------|-----|--------|-------------|
| System | `nealsd@.service` | prefers `:80`, else `:2015`+ | no port if `:80` |
| Ad-hoc | CLI auto-start / `cargo run -p nealsd` | same preference (`:80` usually fails without CAP) | `:{port}` if not 80 |

Override with `NEALS_CADDY_HTTP_ADDR` (no auto-fallback).
