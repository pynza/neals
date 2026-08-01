# Neals — local platform orchestrator

```bash
nix develop   # rustc/cargo via rust-overlay on nixos-24.11
cargo test
cargo run -p neals -- --help
```

Project name: set `neals.name = "my-app";` in `devenv.nix` (folder name is fallback).

Shell completions (dynamic project names):

```bash
# bash
echo 'source <(COMPLETE=bash neals)' >> ~/.bashrc
# or: eval "$(cargo run -q -p neals -- completions bash)"
```
