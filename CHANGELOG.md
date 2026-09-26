# Changelog

## Unreleased

- A task's status dot now looks the same everywhere it shows — the task list, its header, and its reply box — so a running, waiting, or blocked task reads the same no matter where you're looking. A task blocked on another task now shows as blocked instead of looking like it's merely waiting.
- A task's subagents now carry that same status dot, so you can tell at a glance which are still running, done, or hit a problem, without opening each one. A collapsed group of subagents shows how many are still running, and reads as done once none are.
- You can choose the font Oga's interface uses in Settings → Appearance.
- You can replace Oga's default handoff instructions with your own.

- You can no longer set custom worker instructions. Workers receive your brief, project memories, and optional commit attribution; scope stays enforced by permissions, and your brief sets how they report back.
- A task whose worker gives up after Oga blocks one of its steps, such as writing outside the task's folder, now asks you how to go on and names what it couldn't reach, instead of showing as done.
- Workers can read their skills' files wherever those skills are installed.
- When a worker needs to reach something outside its task, the task now asks you to allow or refuse it, and the worker carries on with your answer. You can also reply with what it should do instead.

## 0.2.2 - 2026-09-25

- When a worker splits its task across subagents, each subagent's work now shows as its own group in the task's activity, with its status and its answer, instead of mixing into one list.
- Settings is roomier and easier to scan: sections are grouped with icons in the sidebar, each page has a clear title, and every setting sits on its own row with its switch or button on the right.
- OpenCode workers keep working when OpenCode 2 has taken over the `opencode` command: they find your OpenCode 1 install, and if there isn't one, the task stops straight away and says what to do instead of failing with an unrecognized flag. A worker can also point at a specific OpenCode 1 install.
- OpenCode 2 workers work again with OpenCode 2.0 and later, even when that install has also taken over the `opencode` command. Tasks, follow-ups, the model list with its effort levels, and continuing a task in a terminal all work, and a worker can point at a specific OpenCode 2 install.
- Picking a task from the list now puts you straight in its reply box.
- ⌘B (Ctrl+B on Windows and Linux) now shows or hides the task list, and puts you right in it so the arrow keys move between tasks.
- The reply box is simpler: task details and options now live behind the + button, so typing your next message is the only thing in the way.
- Settings can now pick the worker for you: turn on Choosing a worker, paste your TypeSafe key, and work you hand over without naming a worker lands on the one whose strengths fit what you asked for. Your own rules still catch anything it can't answer, and the task shows which worker was suggested and whether the work went there.
- Claude workers can now run newer models, like Opus 5.5, by using the Claude Code already installed on your machine instead of the older copy Oga shipped with.
- A task no longer fails with a confusing error when it starts just as Oga itself is starting up.
- The reply box no longer jumps when the note about continuing a paused task appears, and that note now lines up with the options under the box.
- Text in a task's activity log — command output, labels, and the response — now reads at one consistent size instead of jumping between sizes.

## 0.2.1 - 2026-09-21

- The Charts view in the usage window now works. It shows cost and tokens day by day for the month you are on, and the month arrows move the charts as well as the grid. Both now fill the window instead of sitting in a narrow strip down the middle.
- A task no longer ends as blocked just because the worker said it was stuck. A run that hit a slow test or a missing setup and worked around it now finishes as done, and a task only stops for you when the worker asks a question or the run itself fails.
- A worker that never answers when a task starts now gives up in half the time, and the task says what that worker said before it went quiet instead of only that it did not answer.

- Settings now lists every keyboard shortcut in one place, grouped by what you are doing, so you can look up what to press without hunting through tooltips.

- You can now send work to fx, alongside Claude, Codex, OpenCode, Antigravity, and Pi. Pick it in Settings and choose from the models your fx account can reach.
- You can now send work to Cursor. Sign in to Cursor once from your terminal, then pick it in Settings and choose from the models your account can reach.
- Workers in Settings now open as their own page instead of unfolding inside the list, so the form, the models, and the environment have room to read. The window also uses the full height of your screen.
- Worker messages in the task timeline now show their formatting — bold, code, lists — instead of the raw symbols.
- An instruction you send a Claude or OpenCode worker while it works now reaches it straight away, instead of waiting for the current run to end. Other workers still pick it up the moment that run finishes.
- Each step a worker takes now shows as one row that expands to its result, grouped under the turn it ran in; repeated or renamed steps no longer split into duplicates, and worker logs stay out of the way.
- Answers you send a worker now appear in full in the transcript instead of stopping mid-sentence.
- "Show more" on a long request now opens the whole thing; it used to stop part way with no way to read the rest.
- Changed files now let you pick what you are looking at: this run's edits, everything not yet committed, or the checkout compared with any branch you choose.
- Code questions now answer with up to seven places instead of ten.
- A task whose model is unavailable or rate limited now says so on one row, with the attempt it is on, instead of a run of rows that say nothing. When the worker gives up because it ran out of usage, the task waits for that to reset and carries on by itself instead of ending with an error.
- The task list keeps every task's status right on its own. A task that finished no longer slips back to looking like it is still working, and the list catches up by itself after a connection drop, so you never have to refresh to trust what it says.

## 0.2.0 - 2026-09-13

- Code questions no longer answer from files your git setup ignores, such as local notes and reports.
- Code questions with several words now leave out files that only share one everyday word with them.
- Code questions answer faster; the first question after updating rebuilds the project's index once.
- You can now point a code question at one folder or file, so the answer only comes from there.
- A task's details now show the branch its checkout is on right now, not the one it started with. The details row also stays tidy when you zoom the app.
- Starting a task no longer freezes the app or other running tasks. Oga now writes its records and reads your shell setup off the main path, and keeps its database log from growing without limit.
- Tasks now stay responsive while Oga prepares or removes large separate checkouts in the background.
- Dozens of small polish fixes across the app: waiting tasks show a fine dashed ring, buttons no longer double-submit, stopping a task always asks first, truncated paths show the full name on hover, settings and usage speak in plain words instead of internal names, and dialogs keep your focus while you type.
- Changed files now open full screen, with the file list beside the diffs so you can read a task's whole change end to end.
- The red connection bar no longer flashes across the top every time a new task lands. It now shows only when Oga is really unreachable.
- Each code answer now says in a few words what the match is, so you can pick the right one without opening it.
- Repeated code questions answer almost at once, since the project is not re-scanned each time.

## 0.1.0 - 2026-09-10

- Replies, briefs and update notes now show tables, checklists, crossed-out words, dividers, and lists inside lists, instead of the raw symbols they were written with.
- Underlined titles now read as titles, links written as references resolve, spelled-out punctuation shows the real character, and a star or bracket meant literally stays on the page instead of formatting the words around it.
- Long replies now appear the moment you open them. A reply with several paragraphs of plain prose used to take a visible pause to draw.
- Changes now read the way they do in your editor: full colour in every language, the exact words that changed picked out inside a line, and line numbers on what came from the checkout. Both the changed files panel and the diffs inside activity get it, and the Unified and Split choice still holds.
- Activity rows now name work done through Oga in plain language and show concise results when you expand them, instead of exposing technical tool names or raw response data.
- Work you hand off now carries a brief the size of the job. A one-line request stays one line, instead of being padded out with headings and numbered steps it doesn't need.
- Sorting the task list by priority now puts failed, cancelled, and blocked tasks near the top, where they need you, instead of burying them at the bottom with the finished ones.
- You can now set the rules your agent follows when it writes up work to hand off — for one project or for all of them — under Settings ▸ Brief Rules.
- Opening the app no longer replays old notifications and activity you had already seen before you last closed it.
- `oga query` now takes a path, or `path#name`, and answers with that exact place. It also finds two-letter names like `db`, and no longer lets something you taught it with `oga relearn` come ahead of the name you typed.

## 0.0.11 - 2026-09-08

- Every task in the list now names the project it runs in, right after the worker. Work with a copy of its own shows the branch too, so two copies of one project never look alike.
- Oga now tells you when a task finishes, stops short, or needs an answer, even with the window closed. Click the notification to open that task. The task you are already reading stays quiet, tasks landing together arrive as one notification, and you can turn all of it off under Settings ▸ Notifications.
- Work you hand off now reaches the tools you already connected for that project, instead of only Oga's own.
- Work you hand off stays where you sent it. Ask for it to hand work onward and it can split the job up itself; otherwise it never can.
- Live task updates stay responsive during busy periods, and the app catches up if it falls behind.
- Task durations now count only time spent running, not scheduled or interrupted waiting time.
- Expanding a search step in the activity view now shows what it found, or says plainly that nothing matched, instead of an empty panel.

## 0.0.10 - 2026-09-07

- Pi activity events now appear while a run is still in progress.
- The About panel now shows the version you are actually running.

## 0.0.9 - 2026-09-07

- `oga query` now indexes Java, C#, C, C++, and Ruby source alongside its existing language support.
- `oga query` now returns up to 10 candidates by default. Delegated workers can add `--code` when they want code lookups to include the matching source without opening another file.
- Pi runs now show thinking, replies, tool calls, and their results in the activity view.
- Update notes now show headings, lists, code, and links correctly in the desktop app.
- Pi runs now include token counts and estimated cost in usage summaries.
- Models switched off in Settings can no longer receive delegated work.

## 0.0.8 - 2026-09-07

### Removed

- `difficulty` is gone from `oga delegate`. Sending it now gets a clear error naming what to send instead — `kind` for what the work is, `effort` for how hard the model thinks — rather than being ignored in silence.

### Changed

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
- `--kind` now also names a class of work — `mechanical`, `general`, `build`, `context`, `reasoning` — not just a subject, so a coding agent that already knows the work is, say, mechanical or a build reaches its loved model for that kind even when the brief itself reads like something else.
- `oga love` now takes an ordered list of destinations per rule, like `oga love opencode:luna:max claude:opus:low --when ui`. The first destination that can take the work runs it; when it is rate-limited, out of credits, or otherwise unavailable, the next one runs instead, and the task record says which one ran and why it was not the first. Each destination is `worker:model:effort` with the model or the effort left out.
- Work a delegated worker ships now says which worker, model, and effort produced it, credited on the commits it makes. Turn it off per project with `worker: attribution: false` in `.oga.yaml`, or everywhere with the same key in `~/.oga.yaml`.
- `oga archive --delete-branch` can now remove a worktree task's local branch safely, while explaining when Git keeps it.
- The Usage window can now switch its activity view between the month grid and charts: cost and tokens over the selected range, plus a cost-by-model breakdown. The grid stays the default.

### Removed

- The project map is gone. Asking `oga query` a question in plain language points at the exact line, so browsing a generated listing no longer has a place. Anything the old map kept is cleaned up the first time this version runs, and a worker prompt that mentioned it keeps working.

### Changed

- The worker's default habits — clearing small reversible obstacles itself, looking code up with `oga query` first, and delivering through checks, a commit, and a pull request — moved out of the app into the worker rules you can rewrite or delete in Settings. If you already customized your rules, they stay exactly as you left them.
- The worker prompt is now plain text you own top to bottom: reorder the sections, rewrite one, or drop one. Nothing is appended behind your back, and a prompt you had already customized keeps working.
- Cleaned up the task activity timeline for Claude runs: shell commands now show a short summary instead of the full command line, and consecutive commands fold into one row with a count.
- `oga query` now finds constants, types, class members, enum cases, fields, and documentation headings, not only functions.
- Answers read each language properly, so a name inside a comment or a piece of text is no longer mistaken for the real thing.
- Asking about a setting lands on the exact line that holds it, named in full, so "sparkle feed url" points straight at it.
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
- Search your tasks by their text, in the app and with `oga tasks --query`.
- Added per-kind default model rules with `oga love --when`.
- Added `oga query --limit` and `oga query --code`.
- Added scheduled starts for delegated and resumed work.
- Every task now suggests what you can do with it next.
- Added queued follow-up instructions when a running worker cannot accept them.
- Showed the files and images handed to a worker beside the request that sent them.

### Fixed

- Kept toast notifications inside the desktop window at every size.
- Each worker profile now keeps its own Codex and Pi sign-in, instead of sharing one.
- A build you made yourself now installs alongside the released Oga instead of replacing it.
- Let workers clear local, reversible obstacles before reporting a blocker.
- Moved "Show thinking" next to the reply box, out of the empty space above the transcript.
- Kept the task menu's "Move to another worker" from being cut off at the top of the window.

## 0.0.5 - 2026-09-04

### Fixed

- Oga now installs and launches cleanly on both Intel and Apple silicon Macs.
