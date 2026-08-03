# Neals — local platform orchestrator

```bash
nix develop   # rustc/cargo via rust-overlay on nixos-24.11
cargo test
cargo build -p neals -p nealsd
```

Project name: set `neals.name = "my-app";` in `devenv.nix` (folder name is fallback).

## HTTP routes (Caddy)

Declare HTTP services in `devenv.nix`. `nealsd` runs a dedicated Caddy and proxies
`{service}.{project}.localhost` to a UNIX socket under the project runtime dir.

```nix
{ pkgs, ... }: {
  neals.name = "demo";
  neals.route.backend = "backend.sock";
}
```

On `neals up`:

- Creates `$XDG_RUNTIME_DIR/neals/<project>/` (bind path for your app)
- Sets `NEALS_RUNTIME` to that directory for the `devenv up` process
- Symlinks `<project>/.neals/<file>.sock` → the runtime socket (**client convenience only** — do **not** bind on `.neals/`)
- Loads Caddy routes (Admin API). HTTP listens on `127.0.0.1:80` when permitted, else `127.0.0.1:2015` (`NEALS_CADDY_HTTP_ADDR` overrides)

Example app listen path: `$NEALS_RUNTIME/backend.sock`.

```bash
neals up demo
curl -H 'Host: backend.demo.localhost' http://127.0.0.1:2015/
# or: curl --unix-socket .neals/backend.sock http://localhost/
```

Only HTTP belongs in `neals.route.*`. Redis/MariaDB/etc. use their own sockets or TCP and are configured in the app `.env`, not via Caddy.

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
