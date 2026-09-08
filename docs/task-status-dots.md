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

## Persistence

Viewed outcomes use `localStorage` under `taskOutcomeViews`.

Each marker stores the task id, outcome identity, viewed state, and a touch time. The store keeps at most 512 markers, dropping the least recently touched entries when it reaches the limit.

Selection does not write a viewed marker. Direct routes and rapid task switching use the active task detail view instead.

## Accessibility

The dot is decorative. Each task row includes a tooltip and text that names the status and whether it is a new update or has been viewed, so color and fill are not the only cues.

## Proposed Release Text

Task status dots now show which task outcomes are new. Amber dashed rings identify tasks waiting on another task or a scheduled retry, while blue marks a request for your input.
