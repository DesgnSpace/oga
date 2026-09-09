# Task Status Dots

## Behavior

- Completed tasks use green dots.
- Failed tasks use red dots.
- Tasks waiting for an answer use blue dots.
- Tasks held on a dependency or scheduled retry use a static amber dashed ring.
- Other blocked tasks keep their blocked status and do not become waiting.
- Queued, running, answered, and cancelled tasks keep distinct status behavior.
- A filled dot marks an outcome that has not been opened.
- An outlined dot marks an outcome that has been viewed.
- Waiting rings stay amber, empty, and dashed in both read states.
- Waiting rings have no animation.
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

Task status dots now show which task outcomes are new. Amber dashed rings identify tasks waiting on another task or a scheduled retry, while blue marks a request for your input.
