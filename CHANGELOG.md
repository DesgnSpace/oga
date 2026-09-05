# Changelog

## Unreleased

### Added

- Added in-app updates for the Oga desktop app.
- Add task text search for MCP and `oga tasks --query`.
- Added per-kind default model rules with `oga love --when`.
- Added `oga query --limit` and `oga query --code`.
- Added scheduled starts for delegated and resumed work.
- Added task-specific next-step guidance to MCP responses.
- Added queued follow-up instructions when a running worker cannot accept them.
- Added public documentation for installation, setup, delegation, task follow-up, worktrees, local data, and release notes.

### Changed

- Consolidated the maintained reference docs.

### Fixed

- Kept Codex and Pi account directories separate for each worker profile.
- Installed source builds beside released Oga without sharing an app identity.
- Let workers clear local, reversible obstacles before reporting a blocker.

## 0.0.5 - 2026-09-04

### Fixed

- Packaged the broker sidecar for supported macOS architectures.
- Located the bundled broker beside the app executable.
- Let Tauri handle app notarization during publishing.
