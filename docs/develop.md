# Develop Neals

```bash
nix develop
cargo test
cargo build -p neals -p nealsd
cargo run -p neals -- --help
```

Ad-hoc CLI use (no systemd): see [Install → Development](install.md#development-ad-hoc).

## Releases

Pushing an annotated tag `v*` runs [.github/workflows/release.yml](../.github/workflows/release.yml):
builds **Linux amd64 + arm64** packages and publishes a GitHub Release.
Nothing is pushed to apt/crates.io/Homebrew — only GitHub Release files.

| Asset | Contents |
|-------|----------|
| `neals-v*-x86_64-unknown-linux-gnu.tar.gz` / `.zip` | bins + man + unit (manual install) |
| `neals-v*-aarch64-unknown-linux-gnu.tar.gz` / `.zip` | same for arm64 |
| `neals_*_amd64.deb` / `neals_*_arm64.deb` | `/usr` + postinst hooks |
| `neals-*-1.x86_64.rpm` / `neals-*-1.aarch64.rpm` | same tree + same hooks |
| `SHA256SUMS` | checksums |

```bash
git tag -a v0.1.0 -m "v0.1.0"
git push origin v0.1.0
```

Local package smoke test (host arch → `./dist`):

```bash
CLEAR_DIST=1 ./contrib/packaging/package-linux.sh 0.1.0
# .deb only (faster): DEB_ONLY=1 ./contrib/packaging/package-linux.sh
# build + install like a release: ./contrib/packaging/install-local.sh
```
