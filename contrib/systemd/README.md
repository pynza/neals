# System daemon (portless URLs)

Goal: open `http://admin.demo.localhost/` with **no port** in the URL, without
lowering privileged ports for every process on the machine.

## Model

Same pattern as nginx / cups / Docker:

| Who | Privileges |
|-----|------------|
| Your shell / `neals` CLI | unprivileged |
| `nealsd` (systemd **system** unit) | `CAP_NET_BIND_SERVICE` only |
| Caddy (child of `nealsd`) | inherits that capability → binds `127.0.0.1:80` |

We do **not** use `net.ipv4.ip_unprivileged_port_start` (that would open low
ports to all user processes).

The unit runs **as your login user** (`nealsd@alice.service`) so config stays in
`~/.config/neals`, project paths under your home work, and `devenv` / nix on your
PATH keep working. Root is only needed once to install the unit and grant the
capability.

## Quick install

From a **git checkout**:

```bash
cargo build --release -p neals -p nealsd
sudo ./contrib/systemd/install.sh "$USER"
```

From a **GitHub Release** archive (`.tar.gz` / `.zip`):

```bash
tar xf neals-v*-x86_64-unknown-linux-gnu.tar.gz   # or aarch64…
cd neals-v*-*
sudo ./systemd/install.sh "$USER"
```

(`install.sh` / `uninstall.sh` attached alone on the Release page are the same
scripts — they need the archive layout: binaries + `man/` + unit next to them.)

Then:

```bash
neals doctor
neals up my-app -d
# → http://backend.my-app.localhost/
```

`install.sh`:

1. Copies `neals` / `nealsd` to `/usr/local/bin` (override with `PREFIX=…`)
2. Installs man pages (`man neals`, `man nealsd`) and this doc under `share/doc/neals/`
3. Installs `nealsd@.service` under `/etc/systemd/system/`
4. `systemctl enable --now nealsd@$USER`
5. Asks **[y/N]** to enable shell completion in the user’s login shell rc
   (bash/zsh/fish). Non-interactive: `NEALS_INSTALL_COMPLETIONS=y sudo ./…`

Manual completion setup (any install method):

```bash
# print the one-liner for your shell, then add it to ~/.bashrc / ~/.zshrc / …
neals completions bash
```

## Uninstall

```bash
sudo ./contrib/systemd/uninstall.sh "$USER"
# also delete ~/.config/neals and ~/.local/state/neals:
sudo ./contrib/systemd/uninstall.sh --purge "$USER"
```

Stops/disables the unit, removes binaries, man pages, unit/drop-in, and docs.
Without `--purge`, user registry and logs are kept.

## Manual install

```bash
sudo install -Dm755 target/release/nealsd /usr/local/bin/nealsd
sudo install -Dm755 target/release/neals /usr/local/bin/neals
sudo install -Dm644 contrib/man/neals.1 /usr/local/share/man/man1/neals.1
sudo install -Dm644 contrib/man/nealsd.8 /usr/local/share/man/man8/nealsd.8
sudo cp contrib/systemd/nealsd@.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now "nealsd@$USER"
```

## Day-to-day UX

```bash
neals up demo          # prints http://admin.demo.localhost/ …
# browser: http://admin.demo.localhost/
neals status
neals down demo
```

No sudo for daily use. If the daemon is down:

```bash
sudo systemctl start "nealsd@$USER"
# logs:
journalctl -u "nealsd@$USER" -f
```

## How the CLI finds the daemon

1. `NEALS_SOCKET` if set  
2. `/run/neals/nealsd.sock` if that file exists (system unit)  
3. else `$XDG_RUNTIME_DIR/neals/nealsd.sock` (ad-hoc / `cargo run`)

While the system socket path exists, the CLI **will not** auto-start a second
user-level daemon; it tells you to `systemctl start` instead.

## Ad-hoc / development (no install)

```bash
cargo run -p nealsd          # NEALS_MODE unset → Caddy on :2015
cargo run -p neals -- up demo
# → http://backend.demo.localhost:2015/
```

| Mode | Env | Listen | URL |
|------|-----|--------|-----|
| System unit | `NEALS_MODE=system` | `127.0.0.1:80` | `http://svc.proj.localhost/` |
| Ad-hoc | (unset) | `127.0.0.1:2015` | `http://svc.proj.localhost:2015/` |

Override anytime: `NEALS_CADDY_HTTP_ADDR=127.0.0.1:8080`.

## Security notes

- Capability is **only** on the `nealsd@$USER` service (and its children), not
  machine-wide.
- `CapabilityBoundingSet` prevents the daemon from gaining other caps.
- `NoNewPrivileges=true` blocks setuid elevation from the service tree.
- Caddy listens on **loopback only** (`127.0.0.1:80`), not on the LAN.
- Remove with `contrib/systemd/uninstall.sh` (see above).

## Requirements

- Linux + systemd  
- `caddy` on the service user’s PATH (login shell PATH may differ from systemd);
  if Caddy is missing, put a drop-in:

  ```bash
  sudo systemctl edit "nealsd@$USER"
  ```

  ```ini
  [Service]
  Environment=PATH=/home/YOU/.nix-profile/bin:/usr/local/bin:/usr/bin
  ```

- Port 80 free on loopback (nothing else bound to `127.0.0.1:80`)
