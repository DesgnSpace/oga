# Deleting old task activity

Every delegated run writes its whole trace to the database: each tool call, each
hook, each chunk of the worker's output. That trace is what `watch` streams and
what the app's detail view scrolls. It is also, by a wide margin, the whole
database — measured at 167 MB across 422 tasks, of which roughly 70 MB sat in
three event types alone. Task rows themselves are rounding error.

Cleanup deletes that trace for work that is finished and put away. It is
the only operation in Oga that destroys anything. Archive hides a task and
restores it; cancel stops a run and keeps the record; resume picks the same
provider session back up. This one has nothing behind it.

## Preview first

The app's **Storage** settings show the current choices and a live preview. The
preview reports eligible tasks, activity records, estimated bytes, and work held
back by the safety rules below. Nothing is deleted until you choose **Review and
remove now**, confirm, and run it.

The command-line preview also reports and deletes nothing:

```
$ oga cleanup --older-than 30d
Nothing has been deleted. This is what would go at 30 days.

Finished and archived, untouched for 30 days (before 2026-07-06):

  187 tasks   142 completed · 31 failed · 14 cancelled
  71,204 activity records, about 118.4 MB

Each of those tasks keeps its title, prompt, result, cost and lineage, and
still lists and opens as before. Only the step-by-step activity of the runs
goes. Project memories are never touched.

Holding back 12 tasks that fanned work out to runs that have not
finished, so each batch keeps the task it started from.

To delete it permanently, with nothing to restore from:
  oga cleanup --older-than 30d --delete
```

A second pass at the same retention finds those tasks eligible but empty, says
so, and offers no delete.

The app defaults to a 30-day preview. `oga cleanup` on its own previews at 30 days. `--delete` will not run without
`--older-than`, so the number that decides what gets destroyed is always one you
typed.

## What it deletes

The step-by-step activity of a task, and nothing else.

The task row survives every time — its title, prompt, the prompt actually sent,
the result, the error, the cost, the lineage, the provider session id. After a
cleanup the task still appears in every list and still opens; what is gone is
the recording of how the run got there. `resume` still works, because the
session id is on the row.

This is deliberate. The trace is ~97% of the bytes and the least of the value:
it answers "what did the worker do minute by minute" for a run that finished
weeks ago and was already put away. The row answers "what did I ask for and what
came back", which is why anyone opens an old task at all. Deleting rows would
reclaim a few more megabytes and lose that.

## What it never deletes

Four rules, all enforced by the Rust store plan that the preview
and the delete both run, so the two cannot disagree:

1. **Work that is running or waiting on you.** `queued`, `running`,
   `needs_input`, `answered` and `blocked` are ineligible at any age. Only
   `completed`, `failed` and `cancelled` qualify.
2. **Work you have not archived.** Archiving is a hand action that means you are
   done with this. Age on its own is never taken as consent, so a task that
   finished a year ago and was never archived keeps its trace forever.
3. **Work that changed recently.** Anything touched since the cutoff stays.
4. **A fan-out whose children are unfinished.** A task that others were
   delegated under keeps its trace while any of them fails rules 1–3. The origin
   run is how a batch is read, and a child still running has no business losing
   it.

**Project memories are never deleted by cleanup, at any age, with any flag.**
There is no option to include them. Memories are durable project knowledge —
decisions and conventions nothing else records — and age is not a proxy for
whether one still holds; a two-year-old convention binds exactly as hard as
yesterday's. Removing one is a per-fact decision, and the `memory` tool's
`remove` action already makes it, one key at a time, with a version check. A
bulk age-swept memory purge would be the exact irreversible mass loss this
feature exists to avoid.

Scope grants, profiles, and profile failure records are outside the statement
entirely.

## Deleting

```
$ oga cleanup --older-than 30d --delete
Deleted the activity of 187 tasks: 142 completed · 31 failed · 14 cancelled.
71,204 activity records removed. Database file 167.2 MB to 48.9 MB.

Each of those tasks keeps its title, prompt, result, cost and lineage, and
still lists and opens as before. Only the step-by-step activity of the runs
goes. Project memories are never touched.
```

The counts are read inside the delete's own transaction, so the numbers printed
are the numbers the database lost. A mismatch aborts the whole thing rather than
reporting a figure it cannot stand behind.

### Why the file actually shrinks

SQLite keeps freed pages inside the database file and reuses them, so a delete
on its own leaves a 167 MB file at 167 MB. Cleanup runs `VACUUM` afterwards,
which rewrites the file, and then a checkpoint, because in WAL mode the rewrite
lands in the log rather than in the file you can see.

The cost: the rewrite takes an exclusive lock for its duration and needs free
disk equal to the database size. On 167 MB that is a few seconds on an SSD,
during which broker writes queue behind it (they wait, they do not fail — the
busy timeout is 5s). It runs only when something was actually removed, so a pass
that finds nothing is free. A delete that does not shrink the file reads as
broken, which is why this is not opt-out.

## Automatic cleanup

Off. A default install schedules nothing and deletes nothing, ever.

The app's **Storage** settings turn it on explicitly. The broker then runs one
pass five minutes after startup, then daily, at that retention and under exactly
the rules above. The default is off, so a default install schedules nothing and
deletes nothing.

`OGA_CLEANUP_DAYS` remains a command-line fallback for headless installs that
have no saved cleanup settings:

```
OGA_CLEANUP_DAYS=60 oga serve
```

The broker logs when automatic cleanup becomes active, and logs one line per
pass that removed something:

```
automatic cleanup on: finished work older than 60 days; archived work only.
First pass after five minutes, then daily.
cleanup: removed 71,204 activity records from 187 tasks archived before
2026-07-06; file 167.2 MB to 48.9 MB
```

Passes that remove nothing stay quiet, so the line that matters is not buried in
daily noise. The Storage preview remains available whenever you want to inspect
what the current settings would remove.

A value that is not a usable number of days makes the broker refuse to start
rather than silently run with no retention. A retention you believe is in force
but is not is worse than either alternative.

## No MCP tool

Deliberate. Agents get `archive`, which is reversible. The irreversible half
stays with the person at the terminal.
