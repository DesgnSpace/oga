
Cargo workspace for the Oga broker and desktop application.

## Layout

- `crates/` — library crates, one per subsystem (`oga-domain`, `oga-store`,
  `oga-config`, `oga-routing`, `oga-providers`, `oga-worktree`,
  `oga-context`, `oga-events`, `oga-runner`, `oga-service`,
  `oga-client`, `oga-http`, `oga-mcp`, `oga-oga`, `oga-oga-tui`)
- `apps/` — binaries (`oga-cli`, `oga-desktop`)

## Commands

Run from this directory (`rust/`):

```sh
cargo fmt --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
```

CI runs all three; a change must pass every one.
