# Changelog

Format: [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
Versioning: [SemVer](https://semver.org/). The IPC protocol version is bumped on
any incompatible change between `neals` and `nealsd`.

## [Unreleased]

### Added

- `neals restart <project> [-d]`: stop if running, then start like `up`.
- `neals down --all`: stop every running project (keeps going if one fails).
- IPC protocol versioning: `nealsd` reports its protocol and `neals` refuses to
  talk to a mismatched daemon, telling you to restart it.
- CI (fmt, clippy, tests, lifecycle smoke); tagged releases are gated on it.
- `LICENSE` (MIT) and crate metadata.

### Changed

- **Upgrade step:** restart `nealsd` after upgrading from 0.x (the `Pong` reply
  changed shape). See [docs/install.md](docs/install.md#upgrading).
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

### Known limitations

- In ad-hoc mode a `nealsd` killed with `SIGKILL` can leave its Caddy running.
- `down` holds the daemon state lock while stopping, so `status` can lag by a
  few seconds during a stop.
