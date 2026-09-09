# Oga desktop

The desktop shell connects to a healthy Rust broker at
`http://127.0.0.1:7331`. If no broker answers, it starts the bundled
`oga-server` or the development `target/debug/oga-cli` binary.

## Development

From `rust/`, build the broker once and start the desktop shell:

```sh
cargo build -p oga-cli
cargo tauri dev --manifest-path apps/oga-desktop/Cargo.toml
```

Set `OGA_BROKER_URL` to use another loopback port. Set `OGA_SERVER_PATH`
to select an explicit Rust broker executable.

## Packaging

See `rust/packaging/README.md` for native bundle prerequisites, local unsigned
builds, updater signing inputs, and the package smoke test.
