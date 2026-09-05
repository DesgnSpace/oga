# Message to a task, from the conversation

One message input in the task detail, routed by the task's own state. The person
never picks an operation; the state does.

## Routing

| Task state | Operation | Why |
| --- | --- | --- |
| `running`, steerable | steer, with queueing as the default | The worker has an open channel, so the message can also interrupt it |
| `running`, not steerable | queue | Nothing reaches the worker mid-run; the message waits its turn |
| `queued`, `answered` | queue | The run has not started; the message is there when it does |
| `needs_input` | reply | The message answers the question the task is parked on |
| `failed`, `cancelled`, `blocked`, `pending` | resume with the message as instruction | Same task id, same session, the message is the next direction |
| `completed` | resume with a required message | A finished run has nothing to retry; the message is the follow-up |

State is read from the live summary row at the moment of sending. Whether the
worker can be interrupted comes from the broker (`control.steerable` on
`GET /api/tasks/:id`), which knows the live run, not from the app guessing. It
decides whether **Send now** is offered — not whether the person can type.

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
- A message that is waiting its turn sits above the input as a row of its own,
  with **Remove** on it, so the person can see and undo what is queued.
- On a run that can be interrupted, **Send now** sits beside Queue with a line
  saying it interrupts the worker instead of waiting. Queue is still the
  default: the safer of the two is the one the Enter key does.

## What is recorded

The broker already records the outcome on the task's event stream:
`steer_accepted` / `steer_rejected` when the message steered,
`follow_up_queued` and `follow_up_started` when it waited its turn, `resumed`
with the instruction when it resumed. The event stream is also what tells the
app the run's real response to the message.

## What is not built here

- No second input. The header's Resume/Follow-up buttons leave the header; the
  conversation input is the one place a message is sent.
- No new broker operation. The steer, reply and resume routes already exist;
  the surface is the app routing a message to one of them.