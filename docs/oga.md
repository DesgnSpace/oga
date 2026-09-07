# Working with `oga`

The macOS app starts Oga's local service. Use the command line to inspect work,
follow it, or operate without the app open.

## Commands

| Command | What it does |
| --- | --- |
| `oga serve` | Run Oga's local service. |
| `oga delegate "<task>"` | Start a task and print its ID. `-` reads the task from stdin. |
| `oga watch <task-id>...` | Stream task events until a watched task settles. |
| `oga tail` | Stream events from every task. |
| `oga query "question"` | Find the place in the code that answers a question. Add `--limit` or `--code`. |
| `oga relearn` | Pick up what changed on disk, or save a hint. Add `--force` to read the project again from scratch. |
| `oga love` | Read or set defaults for unnamed work. |
| `oga inflight` | List work a restart would interrupt. |
| `oga tasks [--query <text>]` | List today's tasks, or search active history. |
| `oga inspect <task-id>` | Print a complete task record. |
| `oga archive` / `oga restore` | Hide or restore task records. Add `--delete-branch` to archive a worktree task and safely remove its local branch. |
| `oga cancel <task-id>...` | Stop a task. |
| `oga resume <task-id>` | Continue a task, optionally with `-m` or `--start-at`. |
| `oga handoff <task-id>` | Move a task to another worker or model with `--worker`, `--model`, `--effort`. |
| `oga complete <task-id>` | Mark a task complete. |
| `oga cleanup` | Preview removable activity and worktrees. |
| `oga config [cwd]` | Print resolved profiles, models, routes, and worker rules. |
| `oga version` | Print build information. |

## Finding code

`oga query` takes a question in plain words and answers with the file, line,
and name that hold the answer:

```sh
oga query "where does the sandbox binary path come from"
oga query --code --limit 1 "how a saved route survives a rename"
```

It knows functions, methods, types, classes, protocols, enum cases, constants,
fields, modules, macros, and documentation headings, across Rust, TypeScript,
TSX, JavaScript, Swift, and Markdown.

When one place is clearly the answer, you get one line. When several could be,
you get up to `--limit` of them, each with the words it matched. When nothing
fits, it says so instead of guessing.

`oga relearn` picks up whatever changed on disk. Add `--force` to read the
whole project again. You can also teach it where something lives, so the words
you use for it land there next time:

```sh
oga relearn "front door|entry point" rust/apps/oga-cli/src/main.rs#run
```

## Task actions

Ask a connected coding agent to delegate new bounded work, or start it here:

```sh
oga delegate "Port the CSV importer to the new parser and keep its tests green."
oga delegate - < brief.md
```

`--worker` and `--model` choose who runs it; `--kind` names what the work is,
so a loved model for that kind isn't missed; `--effort` sets how hard it
thinks; `--worktree` gives it its own checkout and branch; `--cwd` runs it
somewhere other than the current directory; `--json` prints the task record
instead of a line. Scope comes from the project's saved grant for that
account. With no grant the task falls back to the whole directory, and the
output says so.

Either way you get a task ID. Follow it with:

```sh
oga watch <task-id> --timeout 30m &
```

`watch` writes JSON lines. It reports lifecycle events by default; `--all`
also includes tool, command, file, and error events. It exits with `0` when
there is news, `1` on timeout, and `2` for invalid input.

Use `oga inspect <task-id>` before acting on a settled task. Resume failed,
cancelled, or blocked work with `oga resume`; use `-m` when the next run needs
an instruction. Move a task to a different account or model with
`oga handoff <task-id> --worker <name>`; it keeps its id, request, and place in
line, and one still waiting on other work or on a start time keeps waiting.
Archive hides a record without deleting it. `cleanup` is the
only command that permanently removes task activity.

To archive a worktree task and also ask Git to remove its local branch:

```sh
oga archive <task-id> --delete-branch
```

Oga uses Git's safe branch delete. Unmerged commits, another checkout, or a
checkout kept for uncommitted work keep the branch, and the result explains why.
`oga restore` does not accept `--delete-branch`.

Before starting work, search for a task on the same feature, file, or command:

```sh
oga tasks --query "feature, file, or command"
```

Resume a matching task instead of creating a duplicate. Search results rank
title matches first, then summary and prompt matches.
