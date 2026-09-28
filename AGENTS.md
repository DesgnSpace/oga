# Oga

The product is `rust/` and `web/`. Everything else (landing site, docs, scripts) supports it.

## Finding code

Locate code with `oga query "<what you need>"` first; glob and grep are the fallback. Run `oga relearn` only when a lookup sent you to the wrong place or found nothing and you then found it yourself, or when you are asked to refresh or teach it. A lookup that already answered needs nothing, and a brief that forbids relearning wins.

## Checks

Run JS tools through `bun` / `bunx` from `web/`. Rust checks are `cargo fmt`, `cargo clippy`, and `cargo test -p <crate>` from `rust/`. A failure that already exists on `main` does not block your work; name it in your report so it isn't mistaken for yours.

## Code and UI

- Comments describe the code as it stands. The history of a bug or request belongs in the commit message.
- Icons are inline SVG in `web/src/ui/icons.tsx`; the app ships no icon package.
- UI follows `docs/design.md`: spacing, radii, and type come from its tokens in `web/src/oga.css`, never raw pixel values for spacing. Settings-style pages are built from `PageHeader`, `Section`, `Card`, and `CardRow`.
- Interface copy names what the user gets. Internal event, state, and model names never reach the screen.

## Changelog and docs

`CHANGELOG.md` is the app's release notes and nothing else. A line goes in only when someone using the app would notice the change and care: a new ability, a fixed bug they could hit, or a visible change to how something looks or works. Refactors, tests, tooling, internal cleanups, and fixes to things no user could reach get no line, even when the PR is large. When a line does belong, it goes under `## Unreleased`. Write it in the user's words, about what they can now do: no languages, frameworks, file names, internal parts, or how it was built. If a line only makes sense to someone who has read the code, rewrite it.

Changes to the landing page, `landing/docs`, marketing copy, the README, or internal docs get no changelog line, however visible they are, because the changelog tracks the app only.

A PR that changes a feature updates the affected `landing/docs` page in the same PR. `landing/docs/changelog.html` is generated at deploy time, so never commit it.

`docs/` holds only lasting reference that later work follows or looks up: the pages listed in `docs/README.md`. Plans, audits, research notes, investigations, and write-ups of a single change go in the commit message, the PR body, or the task report. Add a page to `docs/` only for a new maintained surface, and list it in `docs/README.md`.

## Git

Use conventional commits. The only attribution trailer allowed is the co-author line Oga adds to its own workers' commits. Branches are `oga/<slug>`, and PRs go to `main` via `gh pr create --base main`.

## Doing work here or delegating it

Do small work here: one file, a Makefile target, a config or doc edit, a rename, a constant, a single check. If the brief would take longer to write than the change, make the change.

Delegate only a named deliverable with several files or steps and its own definition of done. A worktree is for that kind of task, one that ends in a branch and a PR; a small fix is done here or delegated in place, because a fresh checkout's cold build costs more than the fix.

Resume before you redispatch. A changed brief, a follow-up, a review fix, or a conflict on an existing task goes to `oga resume <id> -m "..."` on that task, in its own checkout, so the worker keeps what it already learned. If `steer` is refused, wait for the task to settle and then resume it. Cancel and start a new task only when the deliverable itself changes.
