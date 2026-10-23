# systemd units

| Unit | Role |
|------|------|
| `nealsd@.service` | Per-user daemon (`nealsd@$USER`): Caddy, netns, proxies |
| `neals-hosts-sync.path` | Watches `/run/neals/hosts` (written by `nealsd`) |
| `neals-hosts-sync.service` | Oneshot: merges that fragment into `/etc/hosts` |

`nealsd@$USER` gets `CAP_NET_BIND_SERVICE` so Caddy can take `:80` when free
(else `:2015`+). Serves `http://{service}.{project}.localhost[:port]/`.

Host DNS uses a **fixed** path (`/run/neals/hosts`) so it works on any home
layout (`/home`, `/var/home`, …) without per-user path templates.

Each project runs in a bubblewrap network namespace; `nealsd` needs `bwrap` and
`slirp4netns` on `PATH` (deb/rpm Depends, or install manually).

## Packaged install

Prefer `.deb` / `.rpm` from [GitHub Releases](https://github.com/pynza/neals/releases)
— use `apt install ./…deb` / `dnf install ./…rpm` so Depends resolve. Post-install
enables `nealsd@$USER` and `neals-hosts-sync.path`. See
[docs/install.md](../../docs/install.md).

## Manual install

1. Install `bubblewrap` and `slirp4netns` from your distro.
2. Install `neals` and `nealsd` somewhere on `PATH` (e.g. `~/.local/bin` or `/usr/local/bin`).
3. Copy units to `/etc/systemd/system/`:
   - `nealsd@.service`
   - `neals-hosts-sync.path`
   - `neals-hosts-sync.service`
4. Set `ExecStart=` in the daemon/hosts-sync **service** files to your real
   binary paths (defaults assume `/usr/local/bin/…`).
5. Enable:

```bash
sudo systemctl daemon-reload
sudo systemctl enable --now "nealsd@$USER"
sudo systemctl enable --now neals-hosts-sync.path
```

If `caddy` lives under nix (`~/.nix-profile/bin`), add a drop-in:

```bash
sudo systemctl edit "nealsd@$USER"
```

```ini
[Service]
Environment=PATH=%h/.nix-profile/bin:/usr/local/bin:/usr/bin:/bin
```

Logs: `journalctl -u "nealsd@$USER" -f` · hosts apply:
`journalctl -u neals-hosts-sync.service -f`

Full steps (binaries, completions, uninstall):
[docs/install.md → Manual install](../../docs/install.md#manual-install-arch--tarball--from-source).
