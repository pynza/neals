# Changelog

Format: [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
Versioning: [SemVer](https://semver.org/). The IPC protocol version is bumped on
any incompatible change between `neals` and `nealsd`.

## [1.0.1] - 2026-11-19

### Fixed

- deb/rpm upgrades no longer delete `/etc/systemd/system/nealsd@$USER.service.d/`:
  local `systemctl edit` drop-ins (e.g. `NEALS_ALLOW_HOST_LOOPBACK=1`) now survive
  upgrades and purge. `prerm` only stops/disables on real removal.
- Packaged upgrades reload systemd and `try-restart` a running `nealsd@$USER`, so
  the new binary is used without a manual restart (running projects stop). Enablement
  is left untouched: `systemctl disable` survives upgrades; only first installs
  enable the daemon and hosts-sync units. `dpkg -r` + reinstall re-enables them
  (dpkg passes `configure <old-version>` there, so postrm leaves a reinstall marker).

### Changed

- The nix profile path is now in the shipped unit (`/home/%i/.nix-profile/bin`);
  post-install no longer writes a `path.conf` drop-in in `/etc`. For homes outside
  `/home`, add your own drop-in.
- CI: `contrib/smoke-upgrade.sh` asserts drop-in preservation and restart behavior.

## [1.0.0] - 2026-11-16

First stable release.

### Added

- `neals restart <project> [-d]`: stop if running, then start like `up`.
- `neals down --all`: stop every running project (keeps going if one fails).
- IPC protocol versioning: `nealsd` reports its protocol and `neals` refuses to
  talk to a mismatched daemon, telling you to restart it.
- CI (fmt, clippy, tests, lifecycle smoke); tagged releases are gated on it.
- `LICENSE` (MIT) and crate metadata.

### Changed

- **Upgrade from ≤0.6.1:** restart `nealsd` after installing (the `Pong` reply
  changed shape). From 0.6.2 the protocol is already compatible.
  See [docs/install.md](docs/install.md#upgrading).
- **`$XDG_RUNTIME_DIR` is required.** The shared `/tmp/neals` fallback is gone.
- The IPC socket is created with mode `0600`.
- `nealsd` answers an undecodable request with an error instead of dropping
  the connection.
- On start, `nealsd` removes host aliases and project processes left by an
  unclean shutdown.

### Fixed

- Route updates failed with `403 host not allowed` on Caddy 2.8, which made
  `neals up` fail.
- `neals-hosts-sync` (root) accepted arbitrary lines from the daemon-writable
  fragment. It now only applies `127.53.x.y {project}_{service}` entries.
- `neals down` without arguments now shows `--all` in the usage line.
- CI: locate `nealsd` from cargo test `build/.../out` layouts.
- Flaky socket / HTTP port unit tests under parallel `cargo test`.

### Known limitations

- In ad-hoc mode a `nealsd` killed with `SIGKILL` can leave its Caddy running.
- `down` holds the daemon state lock while stopping, so `status` can lag by a
  few seconds during a stop.
