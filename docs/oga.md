# Working with `oga`

The macOS app starts the local broker. Use the command line to inspect work,
follow it, or operate without the app open.

## Commands

| Command | What it does |
| --- | --- |
| `oga serve` | Run the local broker. |
| `oga watch <task-id>...` | Stream task events until a watched task settles. |
| `oga tail` | Stream broker events. |
| `oga query "question"` | Find files and symbols. Add `--limit` or `--code`. |
| `oga relearn` | Refresh a project map or save source routes. |
| `oga love` | Read or set defaults for unnamed work. |
| `oga inflight` | List work a broker restart would interrupt. |
| `oga tasks [--query <text>]` | List today's tasks, or search active history. |
| `oga inspect <task-id>` | Print a complete task record. |
| `oga archive` / `oga restore` | Hide or restore task records. |
| `oga cancel <task-id>...` | Stop a task. |
| `oga resume <task-id>` | Continue a task, optionally with `-m` or `--start-at`. |
| `oga complete <task-id>` | Mark a task complete. |
| `oga cleanup` | Preview removable activity and worktrees. |
| `oga config [cwd]` | Print resolved profiles, models, routes, and worker rules. |
| `oga version` | Print build information. |

## Task actions

Ask a connected coding agent to delegate new bounded work. It returns a task
ID. Follow it with:

```sh
oga watch <task-id> --timeout 30m &
```

`watch` writes JSON lines. It reports lifecycle events by default; `--all`
also includes tool, command, file, and error events. It exits with `0` when
there is news, `1` on timeout, and `2` for invalid input.

Use `oga inspect <task-id>` before acting on a settled task. Resume failed,
cancelled, or blocked work with `oga resume`; use `-m` when the next run needs
an instruction. Archive hides a record without deleting it. `cleanup` is the
only command that permanently removes task activity.

Before starting work, search for a task on the same feature, file, or command:

```sh
oga tasks --query "feature, file, or command"
```

Resume a matching task instead of creating a duplicate. Search results rank
title matches first, then summary and prompt matches.
