# Changelog

## Unreleased

### Added

- `oga love --when` now routes by subject as well as class: `oga love opencode:muse --when ui` sends UI work there, with `backend`, `database`, `docs`, `tests`, `review`, `research`, and `refactor` alongside the existing kinds. A subject rule wins over a class rule, and `oga delegate --kind <subject>` names the subject when the task text never says so.

### Changed

- Cleaned up the task activity timeline for Claude runs: shell commands now show a short summary instead of the full command line, and consecutive commands fold into one row with a count.
- `oga query` now finds constants, types, class members, enum cases, fields, and documentation headings, not only functions.
- Answers cite Rust, TypeScript, TSX, JavaScript, Swift, and Markdown from the real syntax of each, so a name inside a comment or a string is no longer mistaken for a definition.
- `oga handoff <task-id> --worker <name>` moves a task to another worker or model from the terminal, keeping its id, request, and place in line.
- Looking something up returns in milliseconds, and a project you have not touched is ready again almost instantly.
- The task footer now shows how full the worker's context is, and how far it has grown since the run started.

### Fixed

- The follow-up box now keeps your text if a send fails, disables itself while sending, and lets Escape clear a draft.
- The keyboard hint under the follow-up box now names the action it actually triggers (queue, reply, or continue) instead of always saying "send".
- Fixed the read-only scope indicator on the follow-up box showing the same "active" color as the editable one.
- `oga relearn --force` reads the project again from scratch instead of failing.
- Fixed the task header's options menu opening clipped inside the header strip.
- Fixed workers on your main Claude account failing with "Not logged in" while the same account worked in the terminal.
- Named the task or worker in toast notifications, so you can tell which one is archiving, stopping, or updating.
- Kept the app responsive while several workers look up code in a project at once.
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
