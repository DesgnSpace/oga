
Cargo workspace for the Oga broker and desktop application.

## Layout

- `crates/` — library crates, one per subsystem (`oga-domain`, `oga-store`,
  `oga-config`, `oga-routing`, `oga-advisor`, `oga-pricing`, `oga-providers`,
  `oga-acp`, `oga-worktree`, `oga-context`, `oga-events`, `oga-runner`,
  `oga-service`, `oga-client`, `oga-http`, `oga-mcp`)
- `apps/` — binaries (`oga-cli`, `oga-desktop`)

## Commands

Run from this directory (`rust/`):

```sh
cargo fmt --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
```

`make rust-fmt`, `make rust-lint`, and `make rust-test` run the same checks
from the repository root. A change must pass every one.
