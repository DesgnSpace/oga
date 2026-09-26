# Task Status Dots

One indicator, `TaskStatusDot`, and one mapping from task state to its look
(`taskStatusLook`) are shared by the sidebar row, the task header, and the
composer. No surface keeps its own status colors or shapes.

## Behavior

States that mean the same thing to the user share a look:

- Waiting — queued, pending, preparing_checkout, removing_checkout, answered: a
  static, dashed, muted ring. This is the look for any pending task, held or
  not; the reason a held task is waiting (another task, a schedule, the
  network) is carried by its label, not by a separate ring style.
- Running: a filled dot that pulses; hollow once viewed. Same look in the task
  list, the header, and the composer.
- Needs input: a filled, solid accent dot — it wants you.
- Problem — failed, blocked: a filled, solid red dot, including a task blocked
  on another task; it reads as a problem, not as waiting.
- Settled — completed, cancelled: green (completed) or muted (cancelled), same
  treatment otherwise.
- A filled dot marks an outcome that has not been opened.
- An outlined (hollow) dot marks an outcome that has been viewed. The header
  and composer always show the open task, so they always render the viewed
  form.
- Opening a task marks its current outcome viewed.
- A later completion, failure, or input request starts unread again.
- An outcome arriving while its task is open is treated as viewed.

## Ordering

- The default priority list floats new outcomes above viewed tasks of the same priority, so fresh results are easy to find.
- Priority still decides first: a viewed request for input stays above a new completed result.
- Tasks of equal priority keep their existing order inside the new and viewed groups.
- Newest-first and recently-updated sorts keep their selected meaning and do not float new outcomes.
- Grouping, project filters, collapsed groups, and loaded pages all work as before; ordering applies within each group and to every loaded page, including after reload.

## Selection

- Opening a task marks its outcome viewed at once, so its dot updates immediately.
- The opened row keeps its place until selection moves to another task, so it never shifts underneath the pointer.
- New outcomes arriving in other tasks still move to the top immediately.
- A new outcome arriving in the open task counts as viewed while it stays open.

## Persistence

Viewed outcomes use `localStorage` under `taskOutcomeViews`.

Each marker stores the task id, outcome identity, viewed state, and a touch time. The store keeps at most 512 markers, dropping the least recently touched entries when it reaches the limit.

Selection does not write a viewed marker. Direct routes and rapid task switching use the active task detail view instead.

## Accessibility

The dot is decorative. Each task row includes a tooltip and text that names the status and whether it is a new update or has been viewed, so color and fill are not the only cues.

## Proposed Release Text

Task status dots now show which task outcomes are new. The same dot now appears wherever a task's status shows — the task list, its header, and its reply box — so a running, waiting, or blocked task reads the same everywhere.
