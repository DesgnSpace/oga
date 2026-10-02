# Changelog

## Unreleased

- A task whose worker stops without ever replying is no longer marked completed. Oga first asks the worker once to finish and report; if it still says nothing, the task shows as unfinished with a reason, so you can resume it, and tasks waiting on it hold.
- Search and filter now sit in the task list's top row. Usage moved into Settings, and Refresh tasks is in the filter menu.
- Each group in the task list shows how many tasks it really holds, not just the ones loaded so far, and Load more no longer covers a collapsed group.
- Choosing a worker has its own Settings page, where you can also tell it how to pick — which workers to prefer or avoid, and when.
- Finished tasks no longer leave old copies of your project taking up space. Once a task's work is done and the age you set has passed, Oga removes its copy; a copy with uncommitted changes or still in use by a running task stays, and the task tells you why.
- Full-screen changed files now opens with every file closed, so long diffs no longer slow it down. Open a file to read its diff.
- You can now mark a file reviewed as you read it. The file closes and dims, the header counts how far you are, and a file the worker edits again loses its mark so you never trust a stale tick. Your marks stay put per task, and full screen can open or close every file at once.
- Review changed files without the mouse: J and K move to the next or previous file, O or Enter opens and closes the one you are on, R marks it reviewed, and N jumps to the next file you have not reviewed. A ? in the panel's header lists them.
- Changed files can now be read by folder, as they always were, or by size so the biggest edits come first. Lockfiles, snapshots and build output collect into a Generated group at the bottom, closed until you open it and left out of the review count. Whichever order you pick sticks.

## 0.2.3 - 2026-09-29

- Work that waits on other work now starts from what that work left behind. Its branch is made when it starts, so a chain of branches builds on each one before it, however long.
- A task no longer shows as done when its worker stopped while work it started, like a test run, was still going. Oga asks the worker to wait for it, and if it still stops early, the task shows as unfinished so you can resume it. A Claude task that hits its usage limit now waits for the limit to reset instead of showing as done.

- Buttons and pickers now line up in tighter rows across tasks, changed files, and settings.
- A task's header now names the project it runs in, and shows lines added and removed right beside the changed files button.
- Copy any message or code block with one click, and selected text copies itself.
- A task's menu now sits right beside its title, with the task's details, the thinking toggle, and actions like archive all in one place.
- Files and images attached when a task is handed off now show beside its request. Images open larger when you click them, and other files open in their usual app.
- Oga is ready as soon as it opens or restarts, instead of keeping the app and your agents waiting several seconds while your terminal setup loads.
- Tasks waiting on the same task now all start together when it finishes. One that can't start no longer leaves the others stuck waiting, and replies you queued on a finished task are sent.
- A long-running task stays smooth to watch and scroll, and a new task shows up in the task list as soon as it starts.
- Cancelling a task keeps what its worker did and what it cost up to that point.
- Your agents get answers about models, tasks, and task progress without long pauses, and use less of their context reading them.
- Looking up where code lives is faster and answers only from the project you're in.
- An OpenCode 1 worker that is slow to report its version once no longer has every later task refused.
- A task's status dot looks the same everywhere it shows — the task list, its header, and its reply box — and a task blocked on another task now shows as blocked instead of merely waiting.
- A task's subagents carry the same status dot, so you can tell at a glance which are running, done, or hit a problem. A collapsed group shows how many are still running.
- You can choose the font Oga's interface uses in Settings → Appearance.
- A task shows your messages just as you wrote them and each step in its readable form. To see the raw data behind them while troubleshooting, turn on Show technical details in Settings → Appearance.
- You can replace Oga's default handoff instructions with your own.
- A task's activity keeps the worker's words in a full-width transcript, shown in full, and folds each run of tool calls into one line you can open.
- A task you've opened stays marked as viewed, and shows a new update only when something new happens on it.
- You can change a task that is waiting to start — what it waits for, which worker or model runs it, or an extra instruction — without cancelling it. A task you cancelled before it started can go back in line, and the tasks waiting on it keep waiting until it does.
- You can no longer set custom worker instructions. Workers receive your brief, project memories, and optional commit attribution, and your brief sets how they report back.
- A task whose worker gives up after Oga blocks one of its steps, such as writing outside the task's folder, now asks you how to go on and names what it couldn't reach, instead of showing as done.
- Workers can use their skills wherever those skills are installed.
- When a worker needs to reach something outside its task, the task asks you to allow or refuse it, or to say what it should do instead.
- A task remembers which rows you expanded and where you scrolled, so switching away and back puts you right where you left off.

## 0.2.2 - 2026-09-25

- When a worker splits its task across subagents, each subagent's work shows as its own group in the task's activity, with its status and its answer.
- Settings is roomier and easier to scan: sections are grouped with icons in the sidebar, each page has a clear title, and every setting sits on its own row.
- OpenCode workers keep working when OpenCode 2 has taken over the `opencode` command: they find your OpenCode 1 install, and if there isn't one, the task stops straight away and says what to do. A worker can also point at a specific OpenCode 1 install.
- OpenCode 2 workers work again with OpenCode 2.0 and later — tasks, follow-ups, the model list with its effort levels, and continuing a task in a terminal — and a worker can point at a specific OpenCode 2 install.
- Picking a task from the list puts you straight in its reply box.
- ⌘B (Ctrl+B on Windows and Linux) shows or hides the task list, and puts you in it so the arrow keys move between tasks.
- The reply box is simpler: task details and options live behind the + button, so typing your next message is the only thing in the way.
- Settings can pick the worker for you: turn on Choosing a worker, paste your TypeSafe key, and work you hand over without naming a worker lands on the one whose strengths fit. Your own rules still catch anything it can't answer, and the task shows which worker was suggested.
- Claude workers can run newer models, like Opus 5.5, by using the Claude Code already installed on your machine.
- A task no longer fails with a confusing error when it starts just as Oga itself is starting up.
- The reply box no longer jumps when the note about continuing a paused task appears.

## 0.2.1 - 2026-09-21

- The Charts view in the usage window works. It shows cost and tokens day by day for the month you are on, and the month arrows move the charts as well as the grid.
- A task no longer ends as blocked just because the worker said it was stuck. A run that worked around a slow test or a missing setup finishes as done, and a task only stops for you when the worker asks a question or the run itself fails.
- A worker that never answers when a task starts gives up in half the time, and the task says what that worker said before it went quiet.
- Settings lists every keyboard shortcut in one place, grouped by what you are doing.
- You can send work to fx, alongside Claude, Codex, OpenCode, Antigravity, and Pi. Pick it in Settings and choose from the models your fx account can reach.
- You can send work to Cursor. Sign in to Cursor once from your terminal, then pick it in Settings and choose from the models your account can reach.
- Workers in Settings open as their own page, so the form, the models, and the environment have room to read.
- Worker messages in the task timeline show their formatting — bold, code, lists — instead of the raw symbols.
- An instruction you send a Claude or OpenCode worker while it works reaches it straight away. Other workers pick it up the moment their current run finishes.
- Each step a worker takes shows as one row that expands to its result, grouped under the turn it ran in, and repeated steps no longer split into duplicates.
- Answers you send a worker appear in full in the transcript instead of stopping mid-sentence.
- "Show more" on a long request opens the whole thing.
- Changed files let you pick what you are looking at: this run's edits, everything not yet committed, or the checkout compared with any branch you choose.
- A task whose model is unavailable or rate limited says so on one row, with the attempt it is on. When the worker runs out of usage, the task waits for it to reset and carries on by itself.
- The task list keeps every task's status right on its own, and catches up by itself after a connection drop, so you never have to refresh to trust it.

## 0.2.0 - 2026-09-13

- Code questions no longer answer from files your git setup ignores, such as local notes and reports.
- Code questions with several words leave out files that only share one everyday word with them.
- Code questions answer faster, and repeated ones answer almost at once. The first question after updating rebuilds the project's index once.
- You can point a code question at one folder or file, so the answer only comes from there.
- Each code answer says in a few words what the match is, so you can pick the right one without opening it.
- A task's details show the branch its checkout is on right now, not the one it started with.
- Starting a task no longer freezes the app or other running tasks, and tasks stay responsive while Oga prepares or removes large separate checkouts.
- Changed files open full screen, with the file list beside the diffs so you can read a task's whole change end to end.
- The red connection bar shows only when Oga is really unreachable, instead of flashing every time a new task lands.
- Many small fixes across the app: buttons no longer double-submit, stopping a task always asks first, truncated paths show the full name on hover, settings and usage speak in plain words, and dialogs keep your focus while you type.

## 0.1.0 - 2026-09-10

- Replies, briefs and update notes show tables, checklists, crossed-out words, dividers, and nested lists, instead of the raw symbols they were written with.
- Long replies appear the moment you open them.
- Changes read the way they do in your editor: full colour in every language, the exact words that changed picked out inside a line, and line numbers — in the changed files panel and in the diffs inside activity.
- Activity rows name work done through Oga in plain language and show concise results when you expand them.
- Work you hand off carries a brief the size of the job. A one-line request stays one line.
- Sorting the task list by priority puts failed, cancelled, and blocked tasks near the top, where they need you.
- You can set the rules your agent follows when it writes up work to hand off — for one project or for all of them — under Settings ▸ Brief Rules.
- Opening the app no longer replays notifications and activity you had already seen.
- `oga query` takes a path, or `path#name`, and answers with that exact place. It also finds two-letter names like `db`.

## 0.0.11 - 2026-09-08

- Every task in the list names the project it runs in, right after the worker. Work with a copy of its own shows the branch too.
- Oga tells you when a task finishes, stops short, or needs an answer, even with the window closed. Click the notification to open that task, or turn notifications off under Settings ▸ Notifications.
- Work you hand off reaches the tools you already connected for that project, instead of only Oga's own.
- Work you hand off stays where you sent it, unless you ask it to split the job up itself.
- Live task updates stay responsive during busy periods, and the app catches up if it falls behind.
- Task durations count only time spent running, not time scheduled or waiting.
- Expanding a search step in the activity view shows what it found, or says plainly that nothing matched.

## 0.0.10 - 2026-09-07

- Pi activity shows while a run is still in progress.
- The About panel shows the version you are actually running.

## 0.0.9 - 2026-09-07

- `oga query` understands Java, C#, C, C++, and Ruby.
- Pi runs show thinking, replies, tool calls, and their results in the activity view, with token counts and estimated cost in usage.
- Update notes show headings, lists, code, and links correctly in the desktop app.
- Models switched off in Settings can no longer receive delegated work.

## 0.0.8 - 2026-09-07

- `difficulty` is gone from `oga delegate`. Sending it gets a clear error naming what to send instead: `kind` for what the work is, `effort` for how hard the model thinks.
- Which model a delegated task lands on is decided by `kind` and your rules, and `kind` gains a `ux` subject.
- Reasoning effort comes from `effort` when you set it, else the model's configured effort, else that model's own default.
- `oga love <model> --when general` takes every piece of work no other rule claimed.
- `oga query` returns every strong match, ranked, instead of flipping between unrelated files when you add or drop a word.

## 0.0.7 - 2026-09-06

- `oga love --when` routes by subject as well as class: `oga love opencode:muse --when ui` sends UI work there, with `backend`, `database`, `docs`, `tests`, `review`, `research`, and `refactor` alongside. `oga delegate --kind` can name the subject or class when the brief never says so.
- `oga love` takes an ordered list of destinations per rule, like `oga love opencode:luna:max claude:opus:low --when ui`. When the first is rate-limited, out of credits, or unavailable, the next one runs, and the task says which one ran and why.
- Work a delegated worker ships says which worker, model, and effort produced it, credited on its commits. Turn it off with `worker: attribution: false` in `.oga.yaml` or `~/.oga.yaml`.
- `oga archive --delete-branch` removes a worktree task's local branch safely, and explains when Git keeps it.
- The Usage window can switch between the month grid and charts of cost, tokens, and cost by model.
- `oga handoff <task-id> --worker <name>` moves a task to another worker or model from the terminal, keeping its id, request, and place in line.
- `oga query` finds constants, types, class members, enum cases, fields, and documentation headings, not only functions, and no longer mistakes a name inside a comment or string for the real thing. Asking about a setting lands on the exact line that holds it.
- Looking something up returns in milliseconds, and a project you haven't touched is ready again almost instantly.
- The project map is gone; asking `oga query` in plain language points at the exact line instead.
- The worker prompt is plain text you own top to bottom, and its default habits live in the worker rules you can rewrite or delete in Settings.
- Shell commands in a Claude run's activity show a short summary, and consecutive commands fold into one row with a count.
- The task footer shows how full the worker's context is, and how far it has grown since the run started.
- Scrollbars stay hidden until you scroll or hover.
- Naming a model by its short name (like `luna`) resolves the same way everywhere, and a short name that could mean more than one model is refused with the options instead of a guess.
- The app no longer goes unresponsive while you have a large archive of tasks and several workers running at once, or while several workers look up code in one project.
- The follow-up box keeps your text if a send fails, and Escape clears a draft. Its keyboard hint names the action it will take: queue, reply, or continue.
- Workers on your main Claude account no longer fail with "Not logged in" while the same account works in the terminal.
- Toast notifications name the task or worker they're about.
- Archiving or restoring a task updates the task list straight away.
- "Newest first" sorts by when you started a task.
- A shell command in the activity view only shows a status line when it failed.
- `oga relearn --force` reads the project again from scratch instead of failing.
- The task header's options menu no longer opens clipped.

## 0.0.6 - 2026-09-05

- The desktop app updates itself.
- New desktop users are guided to connect their AI before handing off work.
- Search your tasks by their text, in the app and with `oga tasks --query`.
- Set a default model per kind of work with `oga love --when`.
- `oga query --limit` and `oga query --code` control how many answers come back and whether they include the code.
- Schedule when delegated or resumed work starts.
- Every task suggests what you can do with it next.
- Follow-up instructions queue when a running worker can't take them yet.
- Files and images handed to a worker show beside the request that sent them.
- Each worker profile keeps its own Codex and Pi sign-in.
- A build you made yourself installs alongside the released Oga instead of replacing it.
- Workers clear small, reversible obstacles themselves before reporting a blocker.
- "Show thinking" sits next to the reply box, and the task menu's "Move to another worker" is no longer cut off.

## 0.0.5 - 2026-09-04

- Oga installs and launches cleanly on both Intel and Apple silicon Macs.
