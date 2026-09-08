# Task Status Dots

## Behavior

- Completed tasks use green dots.
- Failed tasks use red dots.
- Tasks waiting for an answer use blue dots.
- Waiting and blocked tasks use the existing warning color.
- Queued, running, answered, and cancelled tasks keep distinct status behavior.
- A filled dot marks an outcome that has not been opened.
- An outlined dot marks an outcome that has been viewed.
- Opening a task marks its current outcome viewed.
- A later completion, failure, or input request starts unread again.
- An outcome arriving while its task is open is treated as viewed.

## Persistence

Viewed outcomes use `localStorage` under `taskOutcomeViews`.

Each marker stores the task id, outcome identity, viewed state, and a touch time. The store keeps at most 512 markers, dropping the least recently touched entries when it reaches the limit.

Selection does not write a viewed marker. Direct routes and rapid task switching use the active task detail view instead.

## Accessibility

The dot is decorative. Each task row includes text that names the status and whether it is a new update or has been viewed, so color and fill are not the only cues.

## Proposed Release Text

Task status dots now show which task outcomes are new. Open a task to mark its current outcome viewed; later completions, failures, and requests for input appear as new again.
