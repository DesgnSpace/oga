# Message to a task, from the conversation

One message input in the task detail, routed by the task's own state. The person
never picks an operation; the state does.

## Routing

| Task state | Operation | Why |
| --- | --- | --- |
| `running` | live steer | The worker has an open channel; the message folds into the current run |
| `failed`, `cancelled`, `blocked`, `pending` | resume with the message as instruction | Same task id, same session, the message is the next direction |
| `completed` | resume with a required message | A finished run has nothing to retry; the message is the follow-up |
| `queued`, `needs_input`, `answered` | no input | Not started, or parked on a question the starting agent answers |

State is read from the live summary row at the moment of sending. A steer is
offered only when the provider can actually take one — native Claude with an
open live channel. That verdict comes from the broker (`control.steerable` on
`GET /api/tasks/:id`), which knows the live worker, not from the app guessing.

## What the person sees

- The composer sits pinned under the trace, so it stays put while the log
  scrolls, on every section of the detail pane.
- A sent message becomes a row above the input — the person's own words, at the
  point they sent them. The row's status is `Sending…` until the broker accepts
  the request, then `Sent`. Nothing claims success at the moment of pressing
  Send: acceptance is the broker's reply, and the effect on the run arrives
  later through the event stream.
- A refused send lands as `Not sent` with a plain sentence — in the
  conversation, where the person is looking. The broker's technical refusal
  stays on the task's event record.
- A running task whose provider cannot be steered shows a disabled composer
  with a caption that says so before the person types anything. No control that
  silently does nothing.
- A queued or needs-input task shows no composer at all — there is nothing a
  message would do, and nothing to say about it.

## What is recorded

The broker already records the outcome on the task's event stream:
`steer_accepted` / `steer_rejected` when the message steered, `resumed` with
the instruction when it resumed. The event stream is also what tells the app
the run's real response to the message.

## What is not built here

- No second input. The header's Resume/Follow-up buttons leave the header; the
  conversation input is the one place a message is sent.
- No reply path for `needs_input`: the starting agent answers that question.
- No new broker operation. The steer and resume routes already exist; the new
  surface is the app routing a message to one of them.