# Moving a task to another model or profile (`handoff`)

`resume` continues a task **on the same profile and model, in the same
provider session**. `handoff` moves it somewhere else — a different model, a
different profile, or both — in one call, running or not, with no cancel
first.

## Why this exists

On 2026-08-03, moving a task off a dying account cost a real run: three
opus/max reviews on `claude-work`, two killed by `You've hit your session
limit · resets 12:40am (America/Chicago)`. One had spent **dozens of turns and
real money**, had read `store.ts`, `events.ts`, `tasks.ts` and the test suite,
and had formed its findings. On disk: nothing — it died mid-write. Profile
`default` sat at 3% session / 8% week the whole time. Recovery was a full
re-dispatch from zero.

The task row was never the problem. It still held the prompt, the attempts,
and the whole event trace. Nothing read them. `handoff` reads them.

A later session hit the same shape of loss from the other direction: a task
running on `claude/opus` needed to move to `claude/fable`, same account,
wrong model. `handoff` refused it outright — a same-profile move reads as "use
resume instead" — so the only path was cancel, then resume with the new
model. The live session, and everything the worker had already read, was
thrown away to change one field.

## Three ways a move can land

| Move | What happens | Session |
| --- | --- | --- |
| Same profile, live worker | `/model` rides the open channel; nothing restarts | kept, mid-turn |
| Same profile, not live (or live control unsupported) | old worker stopped if running; session reopens under the new model | kept, reopened |
| Different profile (model same, different, or omitted) | a session belongs to one account and cannot be opened from another | fresh, seeded with a brief |

The first two together are what makes a same-profile move free: the provider
session is the conversation, and the model is a per-turn setting on top of it,
so switching it costs nothing the run has already paid for. Only a
cross-profile move pays a fresh session's cost — the reason to prefer it is
never "the model is wrong," only "the account is."

## What `handoff` does

```
handoff(taskId, profile?, model?, effort?, scope?)
```

Same Oga task id, same title, same lineage, same attempt history in every
shape. `profile` is optional: omit it to change only the model on the task's
current profile. On a same-profile call `model` is required — the model
change *is* the move, so there is nothing to do without it. Valid from
`failed`, `cancelled` and `blocked` — the dead states — from `pending`, and
from `running`.

On a running task the old worker is stopped before the new one starts, so the
two never overlap and the task is never left with no worker while its state
still claims one — with one exception: a same-profile move onto a live worker
whose provider supports live control switches the model in place and stops
nothing. `resume` accepts the dead states too, and additionally `completed`
when an instruction says what to do next; `handoff` has no such path on a
cross-profile move, because the brief it builds describes a run that ended
and a completed run did not. See [resume.md](resume.md).

`effort` and `scope` are spawn-time settings — a live worker cannot pick them
up mid-turn — so stating either on a same-profile live move forces the
restart path instead of the in-place switch. Omit them to keep the switch in
place.

`model` on a cross-profile move defaults to the destination profile's own
default: the old model id names a model on the old account. `effort` carries
over unless restated.

## Two routes off a dying account

A rate-limited session is paused, not dead. So there are two ways to recover a
task on the account that hit the limit, and the caller should choose
knowingly:

| Route | Cost | Fidelity |
| --- | --- | --- |
| Wait for the window, then `resume` | free | lossless — the same session, with all its context |
| `handoff` to another profile now | a second account's quota | a reconstructed brief |

Which is why every `rate_limit` failure now carries **`completion.resetsAt`**
— the ISO time the window clears. It is parsed from whatever the provider
said: an epoch stamp (`Claude AI usage limit reached|1753999200`), a countdown
(`resets in 48m 15s`), or a wall clock with the zone it was printed in
(`resets 12:40am (America/Chicago)`), and failing all three, from the stream's
own `rate_limit_event`. It rides `completion`, so it appears wherever the
failure already does — `inspect`, the failed event, and the app's trace. The
same time becomes the profile's `retryAt`, replacing a flat ten-minute guess
that made a five-hour window look retryable.

A session or usage limit now classifies as `rate_limit` rather than
`worker_error`; before, the incident's own wording did not match the
rate-limit pattern at all.

## When a session reopens instead of moving

A same-profile handoff that is not an in-place live switch — the task was not
running, or its worker had no open channel — still saves the fresh-session
cost: if the provider session is captured and can be reopened, the row keeps
it, and the new run starts with `# Model change`, a short note naming the new
model, instead of the full rebuilt brief. Only when no session was captured
(the run never started, or the provider never reported one) does a
same-profile move fall back to the rebuilt brief, exactly as a cross-profile
move always does.

## The brief

Built only when a fresh session is actually needed — a cross-profile move, or
a same-profile move whose session cannot be reopened. Deterministic — a
transform over stored rows, no model in the loop.

1. **The in-flight instruction, when the dead run was a resume.** It leads the
   seed as the current job — the original prompt below it is background that
   may already be finished. A first-run failure has none, and the seed starts
   with the original prompt, exactly as before. A queued follow-up
   (`queue: "add"`) that never ran is *not* carried: it was never the work in
   progress, and the queue survives the handoff and feeds it into the handed-
   off session after a clean landing.
2. **The original prompt, verbatim.** The contract the task started with. It is
   never condensed, however large.
3. **Why the previous run ended** — state, completion code, reason, error, the
   reset time, and its final message.
4. **What it left on disk** — every path the trace shows it writing, so the
   next worker reads a half-written file instead of starting it again.
5. **What it said and did**, in one of two tiers:
   - **Verbatim** (default, up to 24,000 characters): the previous worker's
     actual messages and tool calls, in order. This is the point of the
     feature — a review that reached its conclusion at turn 40 carries that
     conclusion across intact.
   - **Digest** (past the cap, ~8,000 characters): a deduplicated tool trace
     plus the **tail** of the assistant messages kept verbatim. Conclusions
     live at the end, so drops come out of the middle and every drop is stated
     (`N earlier messages omitted…`).
6. **A continuation instruction** — continue the current work, do not repeat
   finished work.

Reasoning blocks are left out: the largest thing in a trace and the least
load-bearing once the conclusions are carried. Replies a provider streamed a
chunk at a time (pi, Antigravity) are rejoined into whole messages.

So is everything else that describes the run rather than the work: broker
heartbeats and state transitions, session/step/turn boundaries, thinking and
progress tickers, usage receipts, and CLI hook notifications. The seed carries
only what the next worker needs — the worker's own messages, its tool calls
and their results, and errors — so the size caps spend their budget on work
instead of plumbing.

The brief is the run's `shippedPrompt`, so what the next worker received is
answerable later like any other dispatch. A `handoff_brief` event records the
tier, the size, and how many messages were dropped, when one was built.

## Attempt history

Every run this task id has ever made — including a live worker cut off mid-
turn by a move — files as a `TaskAttempt` naming the profile, the model, and
the session it ran as:

```
inspect(taskId, fields: ["attempts"])
```

A same-profile move that only switched the model in place adds no attempt:
nothing was cut off, so there is nothing to file. Every other move files one.

## Scope and consent

Scope grants are keyed to cwd **and** profile, so moving a task to another
account is a grant question; a same-profile move is not, because the account
never changes. It follows what `delegate` already does:

- **State `scope`** on a cross-profile move → fresh approval for the
  destination; it becomes that profile's grant on the cwd, and there is
  nothing to warn about.
- **Omit it** → the task keeps its own scope, never widened, and the response
  carries a warning naming the destination it was not approved for. The
  warning is suppressed when the destination already holds its own grant for
  that cwd.

## Not built

Automatic failover on `rate_limit`. It spends a second account's quota and
crosses a scope grant without the caller present, so it needs an explicit
opt-in on `delegate` first.

## Complement: write incrementally

The incident that motivated this feature died mid-document — one large final
write that never landed. Prompts for long deliverables should ask for the
file to be created with its first section and appended to. That is a
prompting convention, not code, but it is what turns a 43-turn loss into a
partial recovery, and it makes the brief's "what it left on disk" section
worth reading.
