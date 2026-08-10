# systemd unit (`nealsd@.service`)

Template unit for the portless daemon: `nealsd@$USER` binds `127.0.0.1:80` via `CAP_NET_BIND_SERVICE` and serves `http://{service}.{project}.localhost/`.

## Packaged install

Prefer `.deb` / `.rpm` from [GitHub Releases](https://github.com/pynza/neals/releases) — post-install enables the unit. See the main [README Install](../../README.md#install) section.

## Manual install

1. Install `neals` and `nealsd` somewhere on `PATH` (e.g. `~/.local/bin` or `/usr/local/bin`).
2. Copy this unit to `/etc/systemd/system/nealsd@.service`.
3. Set `ExecStart=` to the real `nealsd` path (default in this file is `/usr/local/bin/nealsd`).
4. Enable:

```bash
sudo systemctl daemon-reload
sudo systemctl enable --now "nealsd@$USER"
```

If `caddy` lives under nix (`~/.nix-profile/bin`), add a drop-in:

```bash
sudo systemctl edit "nealsd@$USER"
```

```ini
[Service]
Environment=PATH=%h/.nix-profile/bin:/usr/local/bin:/usr/bin:/bin
```

Logs: `journalctl -u "nealsd@$USER" -f`

Full steps (binaries, completions, uninstall): [README → Manual install](../../README.md#manual-install).
