# Changelog

## Unreleased

- Every task in the list now names the project it runs in, right after the worker. Work with a copy of its own shows the branch too, so two copies of one project never look alike.
- Oga now tells you when a task finishes, stops short, or needs an answer, even with the window closed. Click the notification to open that task. The task you are already reading stays quiet, tasks landing together arrive as one notification, and you can turn all of it off under Settings ▸ Notifications.
- Work you hand off now reaches the tools you already connected for that project, instead of only Oga's own.
- Work you hand off stays where you sent it. Ask for it to hand work onward and it can split the job up itself; otherwise it never can.
- Dark mode on the landing page now uses light outlines and shadows, so buttons and cards stand out against the dark background instead of smearing into it.
- Live task updates stay responsive during busy periods, and the app catches up if it falls behind.

## 0.0.10 - 2026-09-07

- Pi activity events now appear while a run is still in progress.
- The About panel now reports the installed release version and build type correctly.

## 0.0.9 - 2026-09-07

- `oga query` now indexes Java, C#, C, C++, and Ruby source alongside its existing language support.
- `oga query` now returns up to 10 candidates by default. Delegated workers can add `--code` when they want code lookups to include the matching source without opening another file.
- Pi runs now show thinking, replies, tool calls, and their results in the activity view.
- Update notes now show headings, lists, code, and links correctly in the desktop app.
- Docs and changelog sections now have shareable links you can copy directly.
- Pi runs now include token counts and estimated cost in usage summaries.
- Models switched off in Settings can no longer receive delegated work.

## 0.0.8 - 2026-09-07

### Removed

- `difficulty` is gone from `oga delegate`, the delegate MCP tool, and the routing API. Sending it now gets a clear error naming what to send instead — `kind` for what the work is, `effort` for how hard the model thinks — rather than a silent drop.

### Changed

- The docs and the site now say what Oga is in one consistent way: an orchestrator for coding agents — the layer above the AI coding tools you already use, deciding which one takes each piece of work and carrying it there and back. Pages that called it something else have been corrected.
- Which model a delegated task lands on is decided by `kind` and a loved rule, never a hardness score. `kind` gains a `ux` subject alongside the existing ones.
- Reasoning effort now comes from `effort` when you set it, else a loved model's own configured effort, else that model's own default — never guessed from how hard the work sounds.

### Fixed

- `oga love <model> --when general` now takes every piece of work no other rule claimed, instead of only the work Oga reads as nothing in particular. A rule naming the task's subject or class still wins, and a rule with no `--when` at all still catches whatever is left.
- The Usage activity grid is now a compact, centred strip with weekday labels aligned to its rows.
- `oga query` now returns every strong match, ranked, instead of collapsing to one arbitrary pick when a word was taught as a hint for several unrelated places. Adding or dropping a word no longer flips the answer between unrelated files for no visible reason.

## 0.0.7 - 2026-09-06

### Added

- Release manifests now include the release notes shown in the update prompt.
- `oga love --when` now routes by subject as well as class: `oga love opencode:muse --when ui` sends UI work there, with `backend`, `database`, `docs`, `tests`, `review`, `research`, and `refactor` alongside the existing kinds. A subject rule wins over a class rule, and `oga delegate --kind <subject>` names the subject when the task text never says so.
- `--kind` (in `oga delegate`, the delegate MCP tool, and the routing API) now also names a class of work — `mechanical`, `general`, `build`, `context`, `reasoning` — not just a subject, so a coding agent that already knows the work is, say, mechanical or a build reaches its loved model for that kind even when the brief itself reads like something else.
- `oga love` now takes an ordered list of destinations per rule, like `oga love opencode:luna:max claude:opus:low --when ui`. The first destination that can take the work runs it; when it is rate-limited, out of credits, or otherwise unavailable, the next one runs instead, and the task record says which one ran and why it was not the first. Each destination is `worker:model:effort` with the model or the effort left out.
- Work a delegated worker ships now carries a short stamp naming the provider, model, and effort behind it: a `Co-Authored-By:` trailer on commits it creates, so git credits the run. Turn it off per project with `worker: attribution: false` in `.oga.yaml`, or everywhere with the same key in `~/.oga.yaml`.
- `oga archive --delete-branch` can now remove a worktree task's local branch safely, while explaining when Git keeps it.
- The Usage window can now switch its activity view between the month grid and charts: cost and tokens over the selected range, plus a cost-by-model breakdown. The grid stays the default.

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

- Usage activity grids and charts now fill the Usage window instead of staying in a narrow strip.
- Grouped running steps now animate the group summary instead of every step row.
- Removed the divider below the task search field so the sidebar reads as one column.
- Usage summary tiles and activity views now use the full width of the Usage window.
- Made the Usage window's summary tiles compact and sized the window to its content.
- Naming a model by its short name (like `luna` for `openai/gpt-5.6-luna`) now resolves the same way everywhere a task can be sent, so a model you switched on is no longer refused as off, and one you switched off can no longer run. A short name that could mean more than one model is refused, listing the models it could mean, instead of guessing.
- Fixed the app going unresponsive for everything — the task list, dispatching, even the health check — for stretches while you had a large archive of tasks and several workers running at once.
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
- The activity heatmap in Usage now shows one full month at a time, named and aligned to weekdays, with buttons to step to the month before or after. A month with no activity still draws its full grid instead of collapsing.
- Took a task out of the task list the moment you archive it, and put it back the moment you restore it, instead of waiting for a reload.
- Sorted "Newest first" by when you started a task, instead of repeating "Recently updated".
- A shell command in the activity view no longer carries a green "Succeeded" line, which also appeared while the command was still running. Only a command that failed says so.

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
