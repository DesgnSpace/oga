# What to do next (`next`)

Every task response carries `next`: the moves that fit the state the task is
now in, one line each.

```json
{
  "id": "t_4f2",
  "state": "cancelled",
  "next": [
    { "tool": "resume", "when": "picks up where it stopped; add an instruction to change course" },
    { "tool": "handoff", "when": "same task on another model or profile" },
    { "tool": "archive", "when": "removes the checkout at ~/.oga/worktrees/t_4f2; the branch stays" }
  ]
}
```

`tool` names an Oga tool, or `shell` when the move is a terminal command.
`when` is one short sentence. The array is left out entirely when nothing
applies, so an empty `next` never has to be read.

## Why it is not in the tool descriptions

A tool description is read on every turn; the fact a caller needs is the one
that applies once. Agents used to read four thousand characters about
`delegate` and still miss that archiving a worktree task is what removes the
checkout — because that sentence lived on `archive`, a tool they had no reason
to read. The state the task is in is what selects the fact, so the response is
where it belongs.

Descriptions now say what a tool does and when to pick it over its neighbours,
and parameter descriptions keep the contract — formats, defaults, refusals.
Everything situational moved here.

## What decides the hints

| Fact | Where it comes from |
| --- | --- |
| Task state | the row, at the moment the tool answered |
| A checkout | the task ran in a worktree and the directory is still on disk |
| The branch | the task's own branch, when a checkout or a removal names one |
| A rate limit | the last run's completion code, and its `resetsAt` |
| Denied paths | the last run's `suggestedScope` |
| A queued instruction | `steer` left it waiting for the current run |

## By state

| State | What `next` offers |
| --- | --- |
| `queued`, `running`, `answered` | background `oga watch`, then `inspect`; `steer`; `cancel` |
| `pending` | `resume` to start it now, `cancel` to drop it — plus `handoff` when it waits on an account |
| `needs_input` | `reply`, or `cancel` when the question is not worth answering |
| `failed`, `cancelled`, `blocked` | `resume`, `handoff` — plus `archive` when a checkout is still there |
| rate-limited ending | `handoff` for now, or `resume` with `startAt: "rate_limit"` to wait for free |
| `completed` | `resume` to follow up — plus the branch to push and `archive` when it ran in a checkout |
| archived | `resume` to unarchive and run again; `worktree-remove` for a checkout the archive kept |

## On refusals

A refusal that knows the right tool says so. `resume` on a running task is
refused, and the refusal names `steer`:

```json
{
  "error": "task cannot be resumed from state running: t_4f2",
  "next": [
    { "tool": "steer", "when": "tells a running task something — delivered live, or queued for when the run finishes" }
  ]
}
```
