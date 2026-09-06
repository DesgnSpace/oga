# Changelog

## Unreleased

### Fixed

- Fixed the task header's options menu opening clipped inside the header strip.
- Fixed workers on your main Claude account failing with "Not logged in" while the same account worked in the terminal.
- Fixed the Usage window leaving a large empty gap below your spending data.
- Took a task out of the task list the moment you archive it, and put it back the moment you restore it, instead of waiting for a reload.
- Sorted "Newest first" by when you started a task, instead of repeating "Recently updated".

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
