# Stopping a task (`cancel`)

`cancel` stops a task and kills its worker's process tree. The task record
survives — its title, prompt, spend, event trace and provider session all stay —
so a cancelled task can be resumed, handed off, or archived like any other.

```
cancel(taskId | taskId[], reason?)
```

Valid from `queued`, `running`, `needs_input` and `blocked`. A task parked on a
question you do not want to answer is therefore not a dead end. Cancelling an
already-cancelled task returns it unchanged rather than failing.

`taskId` takes a single id or an array. A single id returns one entry, as
always; an array returns one entry per id, in input order. Each id is handled
independently, so a task that cannot be cancelled — already settled, or an
unknown id — comes back as `{id, error}` without leaving the rest of the batch
unattempted. The response says plainly which ids were stopped and which were
not.

`reason` is stored as the task's error and shown wherever the failure is, so it
is the one place to say *why* this run was stopped. It applies to every id in a
batch. Each surface supplies its own default when the caller omits it.

The worker gets `SIGTERM`, then `SIGKILL` two seconds later if it is still
alive. Completion is recorded as `blocked: true, code: "cancelled"` — or
`code: "timeout"` when the task's own deadline was what stopped it.

## Cancelling is an attention event

The backgrounded `oga watch` exits and prints its settled line when the task
is cancelled, with `reason: "attention"` — the same as a question or a
completion. Nothing polls for this.

## Every surface can do it

| Surface | Call |
| --- | --- |
| MCP | `cancel(taskId | taskId[], reason?)` |
| REST | `DELETE /api/tasks/:id?reason=...` |
| macOS app | the Cancel button in the detail header, and the sidebar context menu |

## In the app

Cancel is offered from the detail header and the sidebar context menu whenever
the state allows it, and always asks first:

> **Cancel this task?**
> This stops the worker's process tree. The task can be resumed later.

The confirmation is not configurable. Cancel is the one action in the app that
destroys work in progress, and a misclick on a run that is forty turns deep
costs the whole run.

Resume sits next to it for the states that can take it — `failed`, `cancelled`,
`blocked` — and fires immediately on click. Option-click opens a sheet for an
instruction instead. The sidebar's context menu offers only the immediate path,
since a context menu has nowhere to type. See [resume.md](resume.md).
