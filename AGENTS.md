# Rules

1. Locate code with `oga query "<what you need>"` first; glob and grep are the fallback.
2. The product is `rust/` and `web/`. `src/` is legacy; do not extend it.
3. Run JS tools through `bun` / `bunx` from `web/`; Rust checks with `cargo fmt`, `cargo clippy`, `cargo test -p <crate>` from `rust/`.
4. Pre-existing failures on `main` do not block; name them in the report.
5. Icons are inline SVG in `web/src/ui/icons.tsx`. No icon packages.
6. Interface copy names what the user gets, never internal event, state, or model names.
7. Comments describe the code as it stands, never the change history.
8. Conventional commits. No AI attribution trailers.
9. Branches are `oga/<slug>`; PRs go to `main` via `gh pr create --base main`.
10. After the PR, run `oga relearn` with symbols that exist in the diff.
