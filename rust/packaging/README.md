# Packaging

The Tauri app bundles the Rust broker as `oga-server`. The packaging hook
builds the web UI, compiles the target-specific `oga-cli` binary, and stages
the sidecar under `apps/oga-desktop/binaries/`. Tauri strips the target
suffix when it copies that sidecar into the app resources directory.

## Prerequisites

- Rust stable and Cargo
- `cargo-tauri` 2.11.4 or a compatible Tauri 2 CLI
- `bun`, to install and build the `web/` React UI
- macOS: Xcode Command Line Tools for `.app` and `.dmg` bundles
- Linux: GTK 3, WebKitGTK 4.1, Ayatana AppIndicator, OpenSSL, librsvg,
  `patchelf`, and the native package tools
- `curl` for the broker smoke test

Ubuntu package names are listed in `release.yml`. Other distributions need
the equivalent development packages. Linux desktop packages require WebKitGTK
at runtime; the generated package declares its native runtime dependencies.

## Local builds

Run from `rust/`:

```sh
cargo tauri build --no-sign
```

The unsigned local build produces native bundles under
`target/release/bundle/`. The build hook stages the sidecar automatically.
Use `./packaging/build.sh --signed` when updater and platform signing
credentials are available.

The matrix in `release.yml` covers macOS and Linux without signing,
notarization, or uploads. Linux rows also run the portability gate described in
`linux-portability.md`.

For the complete signed macOS release flow, run `make publish fix` from the
repository root. It bumps and commits the version, builds the universal app,
notarizes and verifies the bundles, then creates the GitHub release.

## Signed updates

`tauri.conf.json` enables Tauri updater artifacts and points clients at the
GitHub `latest.json` manifest. The committed public-key value is an explicit
placeholder until the release owner selects the update key. No private key is
stored in the repository.

For a signed build, use the wrapper and provide the matching values through
the environment:

```sh
TAURI_PUBLIC_KEY='...' \
TAURI_SIGNING_PRIVATE_KEY='...' \
./packaging/build.sh --signed
```

`TAURI_SIGNING_PRIVATE_KEY_PASSWORD` may hold the key password. The wrapper
injects only the public key into the Tauri config; Tauri reads the private key
from its standard environment variable. Do not run this path on a developer
machine without the release credentials.

## Package smoke test

The smoke test checks the packaged broker identity through both surfaces:

```sh
./packaging/smoke-test.sh target/aarch64-apple-darwin/release/oga-cli
```

Set `OGA_SMOKE_PORT` when port `17331` is already in use. The test uses a
temporary database and stops its broker before exiting.

## Source installs

Source installs are development installs. They build the CLI from the current
checkout and place it at `${PREFIX:-$HOME/.local}/bin/oga`:

```sh
./packaging/install-source.sh
```

To update one, pull the checkout explicitly, then run the script again. Source
installs do not use the signed Tauri updater and do not update automatically.
