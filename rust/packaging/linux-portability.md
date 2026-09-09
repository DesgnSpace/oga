# Linux Portability Gate

The supported Linux build image is Ubuntu 24.04 on `x86_64-unknown-linux-gnu`.
The matrix is defined in `release.yml`; it is the only Linux image this rewrite
promises until another distribution has equivalent package and desktop coverage.

## Package prerequisites

The CI image installs the following packages:

```sh
sudo apt-get install --yes \
  build-essential bubblewrap curl file \
  libayatana-appindicator3-dev libgtk-3-dev libssl-dev \
  libwebkit2gtk-4.1-dev libxdo-dev librsvg2-dev patchelf pkg-config
```

The image also needs Rust stable, `bun`, and `cargo-tauri` 2.11.4. Bubblewrap must be allowed to create user
and PID namespaces; an image that installs `bwrap` but disables those
namespaces is not a supported gate image.

## Gate commands

Every Linux matrix row runs these checks:

```sh
cargo test --workspace --all-features
cargo tauri build --target x86_64-unknown-linux-gnu --no-sign
```

`linux-portability.sh` checks the native package modules, Rust target, Tauri
tools, and a real Bubblewrap namespace before the workspace test. The package
smoke test then starts the built broker with a temporary database and compares
`oga version` with `/health`.

## Capability report

| Surface | Linux evidence | Result |
| --- | --- | --- |
| Broker, CLI, and Oga | Workspace tests and CLI integration tests | Supported |
| Tauri and WebKitGTK | Tauri build with `libwebkit2gtk-4.1-dev` | Supported on the matrix image |
| Tray and notifications | Desktop crate compilation and adapter unit tests | Supported; interactive desktop clicks need a Linux desktop session |
| Paths and temporary state | Workspace tests use `PathBuf` and temporary directories | Supported |
| Signals and process groups | Runner tests exercise Unix process groups and cancellation | Supported on Linux |
| Unix sockets | Event socket integration tests cover replay, keepalives, stale cursors, and limits | Supported on Linux |
| Bubblewrap | Capability script and Linux confinement integration test | Required and fail-closed |

No Linux feature is silently skipped. A missing native package, unavailable
namespace, or failed platform adapter test fails the gate.

## Documented macOS-only differences

Only these source-level platform skips are allowed in the portability matrix:

- The macOS Dock badge adapter in `apps/oga-desktop/src/tray.rs`.
- The macOS `RunEvent::Reopen` handler in `apps/oga-desktop/src/main.rs`.
- The macOS Seatbelt runtime test in `oga-runner/tests/confinement.rs`.

Linux uses the tray title and tooltip instead of a Dock badge. Linux uses the
Bubblewrap backend instead of macOS Seatbelt. These are explicit adaptations,
not hidden fallbacks.

Other Linux distributions need equivalent WebKitGTK 4.1, GTK 3, Ayatana
AppIndicator, OpenSSL, librsvg, `patchelf`, and namespace packages before they
can be added to the matrix.
