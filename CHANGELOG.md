# Changelog

## Unreleased

### Added

- `oga love --when` now routes by subject as well as class: `oga love opencode:muse --when ui` sends UI work there, with `backend`, `database`, `docs`, `tests`, `review`, `research`, and `refactor` alongside the existing kinds. A subject rule wins over a class rule, and `oga delegate --kind <subject>` names the subject when the task text never says so.
- `oga love` now takes an ordered list of destinations per rule, like `oga love opencode:luna:max claude:opus:low --when ui`. The first destination that can take the work runs it; when it is rate-limited, out of credits, or otherwise unavailable, the next one runs instead, and the task record says which one ran and why it was not the first. Each destination is `worker:model:effort` with the model or the effort left out.
- Work a delegated worker ships now carries a short stamp naming the provider, model, and effort behind it: a `Supervised-by:` trailer on commits it creates and a footer on pull requests it opens. Turn it off per project with `worker: attribution: false` in `.oga.yaml`, or everywhere with the same key in `~/.oga.yaml`.
- `oga archive --delete-branch` can now remove a worktree task's local branch safely, while explaining when Git keeps it.

### Removed

- The project map is gone. `oga query` answers the same questions in plain language and points at the exact line, so browsing a generated listing of files and symbols no longer has a place. Anything the old map stored is cleaned up the first time this version runs. A worker prompt you customized to include `{{context_map}}` keeps working: that spot now fills with nothing rather than showing the placeholder.

### Changed

- The worker's default habits — clearing small reversible obstacles itself, looking code up with `oga query` first, and delivering through checks, a commit, and a pull request — moved out of the app into the worker rules you can rewrite or delete in Settings. If you already customized your rules, they stay exactly as you left them.
- The worker prompt is now plain text you own top to bottom: reorder the sections, rewrite one, or drop one, with `{{brief}}`, `{{scope}}`, `{{memories}}`, `{{attribution}}`, and `{{reporting}}` filled in per task and nothing appended behind your back. Prompts customized before templates existed keep working untouched, gaining only the task slot first.
- Cleaned up the task activity timeline for Claude runs: shell commands now show a short summary instead of the full command line, and consecutive commands fold into one row with a count.
- `oga query` now finds constants, types, class members, enum cases, fields, and documentation headings, not only functions.
- Answers cite Rust, TypeScript, TSX, JavaScript, Swift, Python, Go, PHP, and Markdown from the real syntax of each, so a name inside a comment or a string is no longer mistaken for a definition.
- Asking about a setting lands on the exact line in a JSON, TOML, or YAML file, named by its full path through the file, so "sparkle feed url" points at the key that holds it.
- `oga handoff <task-id> --worker <name>` moves a task to another worker or model from the terminal, keeping its id, request, and place in line.
- Looking something up returns in milliseconds, and a project you have not touched is ready again almost instantly.
- The task footer now shows how full the worker's context is, and how far it has grown since the run started.
- Scrollbars in the task list and other panes now stay hidden until you scroll or hover, appearing as a thin overlay instead of a permanent bar.

### Fixed

- Dialogs now dim everything behind them: page scrollbars and menus no longer float above the overlay while a settings or usage window is open.
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
