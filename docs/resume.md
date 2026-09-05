# Continuing a task (`resume`)

`resume` continues a task on the same profile and keeps the same task id. When a
provider session exists, it reopens the same conversation the worker was already
having. That session is the expensive thing: it holds the files the worker read,
the structure it mapped, and why it built what it built. Oga's task row holds
none of that.

When no provider session was captured, `resume` starts a fresh session on the
same profile and task id. It rebuilds a full worker brief from the task's stored
prompt and run record, then adds the new `instruction` when one was supplied.

Two things reach for it.

## Check the task list first

Before any new task or continuation, use `tasks` to list active and recent work.
Resume the task that already owns the same file, feature, or command instead of
creating a duplicate. If stale child state remains, cancel the child and resume
the parent with fresh instructions.

After `resume` returns, start `oga watch <taskId>` immediately. Treat it
ending or printing an event as the required trigger to call `inspect` before
reporting the task's state or deciding what to do next — a watch line alone is
not the result, and state read before that `inspect` call is never "still
running". The response's [`next`](next.md) says the same thing in place.

## A task still working takes a steer, not a resume

`resume` covers work that has stopped. On a `running`, `queued`, `answered`, or
`needs_input` task it is refused, and the refusal names the tool that does
apply — `steer` for a run in flight, `reply` for one parked on a question:

```json
{
  "error": "task cannot be resumed from state running: t_4f2",
  "next": [
    { "tool": "steer", "when": "tells a running task something — delivered live, or queued for when the run finishes" }
  ]
}
```

`steer` handles both halves of that: the worker gets the instruction live when
the provider takes live input, and otherwise it waits and runs as a follow-up
when the current run finishes clean. `resume` with `queue: "add"` still queues
an instruction directly for callers that already do.

## Retry a run that died

States `failed`, `cancelled`, `blocked`, and `pending`. `instruction` is
optional; without one the worker is told to continue from where it stopped.

`pending` force-starts the task and drops its hold. This is useful for a task
that has never launched and has no provider session yet.

```
resume(taskId)
resume(taskId, instruction: "the approved area covers tests — continue there")
```

## Follow up on a run that finished

State `completed`, and here `instruction` is **required**.

```
resume(taskId, instruction: "move the state dot to the title's tail")
```

This is the case a fresh `delegate` handles badly. A follow-up usually touches
the same file, the same layout, the same constraints the worker just spent turns
reasoning about — and a new task re-reads all of it, re-derives the same
structure, and still ends up not knowing *why* the code is the way it is. The
session already knows.

A completed task **without** an instruction is rejected:

```
task <id> cannot be resumed without an instruction — it already finished,
so there is nothing to retry; say what to do next in instruction
```

There is no default here worth guessing. Re-entering a finished session with
"continue" spends a turn on a worker rereading its own output.

## The finished run is not overwritten

`resume` reuses the task row, so the row's `output`, `error`, `question` and
`completion` are cleared before the new run starts. They are not lost: the same
`closeAttempt` path that reply, resume and handoff have always used files the
finished run as a `TaskAttempt` first, carrying its output, its completion, the
profile it ran on and its session id.

```
inspect(taskId, fields: ["attempts"])
```

still shows what the earlier run produced. Spend accumulates on the row across
runs, as it does for reply and handoff. The last 10 attempts are kept.

### Why the same task id, and not a child task

A provider session has one owner. Two task rows naming the same `session_id`
would both claim the right to continue it, and Oga already treats a resumed
run landing in an unexpected session as a fault (`resume_session_mismatch`).
`parentTaskId` means fan-out — the sidebar groups a batch by it — so a follow-up
filed there would render as a sibling of a batch it is not part of.

What that costs the reader: the task's state leaves `completed` and comes back
to it, so the list shows one row for work that happened in two passes, and the
first pass's output is one `fields: ["attempts"]` away instead of being the
visible result.

## When the session is gone

A provider can drop a session — expired, evicted, too large. Oga detects the
shape of that failure (the resumed worker exits nonzero having emitted no
events at all) and does not treat it as a dead end: it starts a fresh provider
session **under the same task id**, seeded with a brief built from Oga's own
stored record of the run that just failed to reopen — the original prompt
verbatim, plus a summary of why the prior run ended and what it left behind.
The `resume` call itself does not surface this; it returns once the new run is
under way, the same as any other resume.

The task's event trace carries both steps: `resume_failed` records the
provider's refusal, and `resume_fallback` records the fresh session that
replaced it. Nothing about the caller's `instruction` is lost — it rides into
the new brief along with the rest of the prior run's context.

This is the same brief-building [`handoff`](handoff.md) uses to move a task to
a different account, reused here to rebuild a session on the *same* account
when the session itself, not the account, is what broke.

## When no session was captured

The provider may never create a session before a task ends. This happens when a
task never launches behind a dependency, or when a provider exits before its
first session event. `resume` does not refuse that task: it starts a fresh
session under the same task id and records `resume_fallback` with
`reason: "session_not_captured"`.

If the task never started, the fresh brief says why Oga kept it waiting or
blocked. A dependency-blocked run's ending is kept as a `TaskAttempt` with its
error and completion, even though it has no session or worker output. A task
that was only `pending` has no attempt until it actually runs.

When a prerequisite ends `failed`, `cancelled`, or `blocked`, its dependents
wait behind dependency holds and carry a dependency-blocked completion. Resume
the prerequisite and those dependents return to `pending`; the restore walks
the dependency chain, so descendants do not stay dead while their prerequisite
is being retried. They launch when every prerequisite completes. A caller that
explicitly cancels one loses the dependency marker and it stays cancelled.

Profiles that run a custom command still cannot resume or capture provider
sessions. Resume names that limitation; use a fresh `delegate` when a new task
is intended, or configure a supported provider profile.

## Resuming an archived task

An archived task is resumed in one call: it is unarchived and runs again,
keeping its id and history. Attempts already recorded stay recorded.

If the task ran in a worktree, archiving removed its checkout. Resuming
recreates it on the same branch when the branch still exists, so the follow-up
continues where the work left off. The tool result says it did. Only when the
branch itself is gone does the resume fail, naming the missing branch rather
than "task is archived".

## In the app

A completed task's detail header carries **Follow up**, which opens a sheet for
the instruction. It is not offered from the sidebar's context menu: that menu is
the no-argument fast path, and a follow-up cannot exist without an instruction.
