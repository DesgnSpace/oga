# Oga docs

| File | What |
| --- | --- |
| [routing.md](routing.md) | Choosing a profile, model, and effort — `.oga.yaml`, availability, quota |
| [routing-defaults.md](routing-defaults.md) | What routing does before anyone configures it — the shipped numbers, and what gives way when |
| [fields.md](fields.md) | Response shape selector — per-tool defaults, group table, worked examples |
| [follow-along.md](follow-along.md) | Following a task without paying for it — watch / inspect |
| [caller-cursor.md](caller-cursor.md) | How a task remembers who dispatched it — the caller column, stdio identity, and delivery resolution |
| [complete.md](complete.md) | Asserting completion when the worker never attested it |
| [resume.md](resume.md) | Continuing a task in its own session — retry, and follow-up on finished work |
| [cancel.md](cancel.md) | Stopping a task — where it works from, and what survives |
| [handoff.md](handoff.md) | Moving a task to another model or profile — running or not, one call |
| [pi.md](pi.md) | Pi provider reference — dispatch, resume, and verification |
| [opencode-2.md](opencode-2.md) | OpenCode 2 provider reference — dispatch, resume, and verification |
| [cleanup.md](cleanup.md) | Permanently deleting old task activity — what goes, what never does |
| [worker-rules.md](worker-rules.md) | Setting a project's own worker rules in `.oga.yaml` — and what stays fixed |
| [worktree.md](worktree.md) | Task worktrees — a checkout and branch per task |
| [scope.md](scope.md) | Data scope rules — bare dir vs. `/**` vs. `**`, grants, permission vs. reading, and EPERM gotchas |
| [gotchas.md](gotchas.md) | Failure modes worth knowing before you hit them, and their fixes |
| [context-index.md](context-index.md) | The code index's merged schema — entity identity vs. position, and how learned routes survive code motion |
