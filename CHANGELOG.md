# Changelog

## Unreleased

### Changed

- `oga query` now finds constants, types, class members, enum cases, fields, and documentation headings, not only functions.
- Answers cite Rust, TypeScript, TSX, JavaScript, Swift, Python, Go, PHP, and Markdown from the real syntax of each, so a name inside a comment or a string is no longer mistaken for a definition.
- Asking about a setting lands on the exact line in a JSON, TOML, or YAML file, named by its full path through the file, so "sparkle feed url" points at the key that holds it.
- Looking something up returns in milliseconds, and a project you have not touched is ready again almost instantly.

### Fixed

- `oga relearn --force` reads the project again from scratch instead of failing.

## 0.0.6 - 2026-09-05

### Added

- Added in-app updates for the Oga desktop app.
- Guided new desktop users to connect their AI before delegating work.
- Add task text search for MCP and `oga tasks --query`.
- Added per-kind default model rules with `oga love --when`.
- Added `oga query --limit` and `oga query --code`.
- Added scheduled starts for delegated and resumed work.
- Added task-specific next-step guidance to MCP responses.
- Added queued follow-up instructions when a running worker cannot accept them.
- Showed the files and images handed to a worker beside the request that sent them.
- Added public documentation for installation, setup, delegation, task follow-up, worktrees, local data, and release notes.

### Changed

- Consolidated the maintained reference docs.

### Fixed

- Kept toast notifications inside the desktop window at every size.
- Kept Codex and Pi account directories separate for each worker profile.
- Installed source builds beside released Oga without sharing an app identity.
- Let workers clear local, reversible obstacles before reporting a blocker.
- Moved "Show thinking" next to the reply box, out of the empty space above the transcript.
- Kept the task menu's "Move to another worker" from being cut off at the top of the window.

## 0.0.5 - 2026-09-04

### Fixed

- Packaged the broker sidecar for supported macOS architectures.
- Located the bundled broker beside the app executable.
- Let Tauri handle app notarization during publishing.
