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
11. Do small work here: one file, a Makefile target, a config or doc edit, a rename, a constant, a single check. If the brief takes longer to write than the change, make the change.
12. Delegate only a named deliverable with several files or steps and its own definition of done. A worktree is for that kind of task — one that ends in a branch and a PR. Never open a worktree for a small fix; do it here or delegate it in place.
13. Resume before you redispatch. A change of brief, a follow-up, a review fix, or a conflict on an existing task goes to `oga resume <id> -m "..."` on that task, in its own checkout. If `steer` is refused, wait for the task to settle and resume it. Cancel and start a new task only when the deliverable itself changes.
14. `CHANGELOG.md` is the app's release notes and nothing else. A PR that changes what the app does or shows adds one line under `## Unreleased`, written for the user.
15. Never add a changelog entry for the landing page, `landing/docs`, marketing copy, README, or internal docs, however visible the change is. If the only thing that changed is a page on the site, the PR adds no changelog line at all.
16. A PR that changes a feature updates the affected `landing/docs` page in the same PR. The changelog page is generated at deploy time, so never commit `landing/docs/changelog.html`.
