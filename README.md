# Neals — local platform orchestrator

```bash
nix develop   # rustc/cargo via rust-overlay on nixos-24.11
cargo test
cargo build -p neals -p nealsd
```

Project name: set `neals.name = "my-app";` in `devenv.nix` (folder name is fallback).

## HTTP routes (Caddy)

Declare HTTP services in `devenv.nix`. `nealsd` runs a dedicated Caddy and proxies
`{service}.{project}.localhost` to a UNIX socket or a loopback TCP port.

```nix
{ pkgs, lib, ... }: {
  # Declare `neals` in an import so the rest of the file stays flat.
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
      backend = "backend.sock";  # UNIX
      api = "tcp";               # TCP (nealsd assigns port)
    };
  };
}
```

On `neals up`:

- Creates `$XDG_RUNTIME_DIR/neals/<project>/`
- Sets `NEALS_RUNTIME` for the `devenv up` process
- **UNIX:** symlinks `<project>/.neals/<file>.sock` → runtime socket (**client convenience only** — do **not** bind on `.neals/`)
- **TCP:** allocates a free `127.0.0.1` port, leases it until Down/crash, injects env (see below)
- Loads Caddy routes. HTTP listens on `127.0.0.1:2015` by default (`NEALS_CADDY_HTTP_ADDR` overrides)

### TCP env vars

Service names are normalized: uppercase, `-` → `_`.

| `devenv.nix` | Env |
|---|---|
| `neals.route.api = "tcp"` | `NEALS_PORT_API`, `NEALS_LISTEN_API=127.0.0.1:<port>` |
| `neals.route.api-backend = "tcp"` | `NEALS_PORT_API_BACKEND`, `NEALS_LISTEN_API_BACKEND=…` |

App contract: bind **exactly** on `127.0.0.1` at that port (not `0.0.0.0`).

If `devenv up` crashes, `nealsd` reaps the child, frees port leases, and updates Caddy.

```bash
neals up demo
curl -H 'Host: backend.demo.localhost' http://127.0.0.1:2015/
curl -H 'Host: api.demo.localhost' http://127.0.0.1:2015/
# unix client: curl --unix-socket .neals/backend.sock http://localhost/
```

Only HTTP belongs in `neals.route.*`. Redis/MariaDB use their own sockets/TCP in the app `.env`, not via Caddy.

## Daemon + project lifecycle

`neals up` / `down` / `status` talk to `nealsd` over a UNIX socket. If the daemon is not running, the CLI starts it automatically (logs in `~/.local/state/neals/nealsd.log`).

```bash
cargo run -p neals -- register
cargo run -p neals -- up my-app
cargo run -p neals -- status
cargo run -p neals -- logs my-app       # last 100 lines
cargo run -p neals -- logs my-app -f    # last 100 lines, then follow
cargo run -p neals -- doctor
cargo run -p neals -- down my-app
```

You can also run the daemon in the foreground: `cargo run -p nealsd`.

## Shell completions

```bash
# bash
echo 'source <(COMPLETE=bash neals)' >> ~/.bashrc
# or: eval "$(cargo run -q -p neals -- completions bash)"
```
