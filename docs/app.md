# The macOS app

A menu-bar app over the same broker every MCP client talks to. It shows the task
list, one task's full record and event trace, profile settings, and project
memories. It holds no state of its own — everything on screen came from
`127.0.0.1:7331` and can be read back from the CLI.

## What each surface says

The sidebar is for **picking**; the detail header is for **confirming what you
picked**. A fact belongs on both only when it does different work in each place.

**A sidebar row** carries a state dot, the title, and one meta line: the state
in words when the state is not self-evident, then the provider mark, the worker,
and the model in short form. Those are the facts you scan a list against. The
row clips to one line, so hovering shows the whole thing, and the accessibility
label carries the full model ID rather than the short one.

**The detail header** carries a state dot that speaks its state, the title, and
up to three chips: worker and model together in one chip behind the provider
mark, the reasoning level when the run was dispatched with one, and the folder,
shortened with the full path on hover.

Worker and model share a chip because the list you clicked from already named
the pair — here they confirm rather than teach. They are not dropped entirely:
a detail view is reachable without ever having scanned its row, and then it is
the only place on screen that says which worker produced this.

Effort and folder are header-only. You do not choose a task by reasoning level
when scanning a list of runs against the same worker, and the folder is what
tells two otherwise-identical tasks apart once you are already inside one.

**Run details**, behind the header's ellipsis, holds the task ID, the provider
session ID when there is one, and the full copyable folder path — the
identifiers worth copying and nothing else. Worker and model are deliberately
absent: repeating them would make the reader check two places for one fact.

## How it stays fast

A real trace runs to ten thousand events. Three rules keep that from reaching
the main thread.

**A live event stream drives updates, not a fast poll.** The app holds one
`GET /api/events` SSE connection and reacts to what arrives on it instead of
re-asking on a short interval. `GET /api/state?view=summary&compact=1` still
runs as a fallback — every 30 seconds while the stream is connected, every 2
seconds while it isn't — but `view=summary` already strips prompt, output and
attempt history down to task summaries, the response decodes off the
MainActor, and each field is assigned only when it differs from what's on
screen, so a quiet refresh invalidates no views at all.

**The detail pane fetches one task.** Opening a task calls
`GET /api/tasks/:id` for the full row. It refetches whenever the event stream
reports a change for that specific task, or the summary row's `updatedAt` or
`state` moves past the copy on screen — including on the parked loop of a
terminal task, where completion is what lands the output the Response tab
shows. Other open tasks' events never trigger this refetch or recompose the
trace on screen.

**Activity opens on the tail.** The Activity tab fetches the newest 1,500 events
and starts there, rather than paging in a run's whole history. From there, the
same event stream that drives the sidebar tells the open task's pane when to
fetch newer events — a non-blocking `waitMs=0` read, not a standing long poll.
A **Load earlier activity** button pages backwards 1,500 at a time and
disappears when there is nothing older. Composition — turning raw events into
the trace you read — runs off the MainActor, one at a time, with at most one
re-run queued if events land mid-compose; 300 blocks render at once.

## The routes behind it

All loopback-only, all readable with `curl`.

| Route | What it returns |
| --- | --- |
| `GET /api/state?view=summary` | Profiles, task summaries, profile failures, scope grants, memory project counts, spend totals |
| `GET /api/state` | The same envelope with full task rows |
| `GET /api/tasks/:id` | One task in full, including its provider session ID. 404 `{"error":"unknown task"}` on an unknown ID |
| `GET /api/tasks/:id/events?last=N` | The newest `N` events, ascending, with `cursor`, `oldestId` and `hasEarlier` |
| `GET /api/tasks/:id/events?before=<id>&last=N` | The newest `N` events older than `<id>`, same shape |
| `GET /api/tasks/:id/events?after=<cursor>&waitMs=<ms>` | Long poll for events past a cursor |
| `GET /api/events?after=<cursor>` | SSE stream of task move pointers, used to drive the app instead of a fast poll |

`last` clamps to 1–5,000. A tail read asks for what is already there, so
`waitMs` is ignored whenever `last` or `before` is present.

Quota and project memories sit on their own routes — `GET /api/usage` and
`GET /api/memories?cwd=` — because both are expensive enough to slow the poll
that drives the whole interface. One memory value alone can run to sixteen
thousand characters.
