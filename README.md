# Neals — local platform orchestrator

```bash
nix develop   # rustc/cargo via rust-overlay on nixos-24.11
cargo test
cargo build -p neals -p nealsd
```

Project name: set `neals.name = "my-app";` in `devenv.nix` (folder name is fallback).

## Daemon + project lifecycle

`neals up` / `down` / `status` talk to `nealsd` over a UNIX socket. If the daemon is not running, the CLI starts it automatically (logs in `~/.local/state/neals/nealsd.log`).

```bash
cargo run -p neals -- register
cargo run -p neals -- up my-app
cargo run -p neals -- status
cargo run -p neals -- logs my-app    # last 100 lines
cargo run -p neals -- down my-app
```

You can also run the daemon in the foreground: `cargo run -p nealsd`.

## Shell completions

```bash
# bash
echo 'source <(COMPLETE=bash neals)' >> ~/.bashrc
# or: eval "$(cargo run -q -p neals -- completions bash)"
```
