# Following a task without paying for it

> The separate attention and alert surfaces were removed. Task state remains on
> each task row and in the `tasks`, `inspect`, and live event-feed responses.

Delegating is cheap, and following is now free too: `oga watch` sleeps in the
background and costs nothing while it waits.

## Background the `watch` subcommand

`make install` links the compiled binary onto your PATH as `oga`, and that
binary carries its own runtime — no Bun, no checkout, no `bunx`:

```sh
oga watch 8f2c1a94-... --timeout 30m &
```

In a checkout that has not been installed, the same command is
`oga watch`. The Rust CLI derives whichever
applies from how the process started, and the usage text and the MCP
instructions print it from there — so neither can name a command that does not
run.

A blocked process costs nothing while it sleeps: no tokens, no turn, no
context, and it never holds the chat turn open — the caller stays free to
handle other requests while several delegated tasks progress at once. Every
client Oga serves has a shell, so starting it needs no client extension and
no research-preview flag.

**Automatic wake-up on exit is a host feature, not a guarantee every client
has.** In Claude Code, a background command re-invokes the agent when it
exits, so backgrounding `oga watch` *is* the notification there. That hook
is not universal: codex's hooks are reactive to its own turn lifecycle only
(`PreToolUse`, `PostToolUse`, `Stop`, and the rest) with no event for "an
external process just finished" — so a backgrounded `oga watch` on codex
still sleeps for free and still
settles correctly, but nothing tells the caller it did. The `needs_input` or
completion is real and sits in the store the whole time; only the
notification is missing, which is how a question once went unanswered until
someone queried the store by hand.

So the news also rides the one channel every host does read: the next MCP tool
result. Whatever the caller calls next — `tasks`, `memory`, `delegate`, even
`health` — the answer carries a second block naming what moved since the caller
last heard:

```text
Oga — 2 delegated tasks moved:
needs your input · 8f2c1a94-... · Port the parser
completed · 3d7e5b02-... · Wire the store
Call inspect with a task id to read the question or the result.
```

One line per task, named in the words Oga uses everywhere else — *needs your
input*, *is blocked*, *failed*, *completed*, *was cancelled*. A question is
listed above a completion whatever order they arrived in, so a finished task can
never bury the one waiting on an answer. A run that failed, was resumed, and
then asked a question is still one line, named by the newest thing that happened
to it.

The service builds it. It is a pointer, never a payload — the question
text and the output stay in `inspect`, per the response-payload rule — and it is
absent entirely when there is nothing new, so a caller that is up to date pays
nothing for it.

The alert block is the one channel that stays strictly pointer-only. `oga
watch` and the event socket it reads are the other: there the pointer grew a
bounded preview — the question, the completed task's TL;DR, the failure reason
and code — cut to a documented cap and flagged when cut, so a caller that reads
the socket can answer a question or act on a failure without the `inspect`
round trip. The full record is still the only home of the output and of the
untruncated text.

The delivery ledger (`consumer_cursors` and `deliveries`, `rust/crates/oga-store`) is
durable for transports that have a durable consumer id. A burst larger than the
block has room for is counted (`and 3 more — use tasks to list them`) and the
rest are named over the calls that follow.

Writing the block is not the delivery. A tool result carries no acknowledgement
back, so a line retired the moment it was written is lost for good when that
result never reaches the model — a dropped connection, a crashed client, a
truncated context — while the ledger records it as told. What the block names is
held instead, and cleared only when that caller calls again: coming back is the
proof the last result landed. Until then the line stays in the queue, so the
worst case is hearing about a task twice rather than never.

The first call from a caller Oga has not heard from before is a baseline rather
than a report: work that had already settled before it ever spoke is work it has
acted on. A task still parked on a question is the exception — it is waiting on
that caller right now, whenever it started waiting, so it is named on the first
call too.

Neither mechanism replaces the other. The backgrounded watch is what wakes a
host that can be woken; the alert block is what reaches a host that cannot.

It blocks until any named task asks a question, fails, is cancelled, or
completes, and prints **NDJSON — one JSON object per line, no prose, no blank
lines** — ready to pipe and parse. By default only lifecycle events stream —
that a task started, changed state, asked a question, or ended:

```json
{"type":"event","kind":"lifecycle","task":"8f2c1a94-...","text":"Worker spawned: antigravity"}
```

`--all` also streams the activity underneath — `tool`, `command`, `file`, and
`error` events — for a long run where every step matters:

```json
{"type":"event","kind":"error","task":"8f2c1a94-...","text":"Agent error: Your workspace is out of credits"}
```

Settled tasks print one line each:

```json
{"type":"settled","task":"8f2c1a94-...","state":"completed","title":"Port the parser","tldr":"Ported the parser to the new store"}
{"type":"settled","task":"8f2c1a94-...","state":"needs_input","question":"Which database should the migration target?","title":"Port the parser"}
{"type":"settled","task":"8f2c1a94-...","state":"failed","error":"timeout after 600000ms","code":"timeout","title":"Port the parser"}
{"type":"settled","task":"8f2c1a94-...","state":"completed","title":"Port the parser","archived":true,"more":true}
```

The `task` field always carries the full id, so a fan-out's lines tell each
other apart without spending an `inspect` per id. `question`, `error`, `tldr`,
`code`, `title`, and `archived` appear only when there is something to say; a
task with no title prints just the id, state, and question or error. A settled
line carries the outcome, not just the state: a completed task's TL;DR, a
`needs_input` question, or a failed task's error plus its `code` (`timeout`,
`rate_limit`, `permission_denied`, ...). `archived: true` marks a task that has
been archived — it still resolves and still settles, unlike an id this store
has never held, which exits `2` and names the database file it searched. Every
string value is collapsed onto one line and cut to a cap — 200 characters for
the outcome, question, and error, 80 for a title — and when a cap cut a value
the line says `"truncated": true`, so a cut is never silent. A line that says
`"more": true` carries no outcome at all: the task settled without a TL;DR or
reason on record, so the outcome lives in `inspect` and nowhere else.

That is the entire output. A task still running prints nothing, because silence
is the point.

A settled line carries enough to act on the common moves: answer the question,
read the completed TL;DR, or tell a failure's code and reason apart. Call
`inspect` when the line says `truncated` or `more`, when you need the full
output or the untruncated text, or when you are about to report a task's state
and the line you read is older than the moment you last checked it. A watch
line is a notification, not the record: never call a task "still running" from
state you read before the line arrived.

**Exit codes carry the news on their own**, so a caller that only reads `$?`
still learns whether it was woken by a task or by the clock:

| Code | Meaning |
| --- | --- |
| `0` | At least one task settled; its line is on stdout |
| `1` | The deadline passed with nothing to report |
| `2` | Bad arguments, or an unknown task id (message on stderr) |

Options:

- Several ids follow a whole fan-out at once: `oga watch <id> <id> <id>`.
- `--timeout` (or `-t`) takes `90s`, `30m`, `2h`, or a bare number of
  milliseconds. It defaults to 30 minutes, so a watch can never hang forever.

Where the harness does not re-invoke on exit, redirect the line to a file the
agent can read for near-zero cost — degraded, but never silent:

```sh
oga watch 8f2c1a94-... > /tmp/task.watch 2>&1 &
```

### Two transports, one output

`watch` connects to the broker's unix event socket at `~/.oga/oga.sock` for
instant push delivery with zero database access and zero polling. When the
socket is absent — broker not running, or started before socket support — it
falls back to reading the same SQLite store the broker writes, so a caller
holding a task id still gets an answer even when nothing is listening on the
port.

Both transports cursor on the same `task_events` rowid, so **a socket that dies
mid-run fails over to the database at the exact event it stopped at** —
nothing replayed, nothing missed. The failover prints one line to stderr and
changes nothing on stdout:

```
event socket lost: ...; falling back to database
```

Printed lines, exit codes, and flags are identical across socket mode, fallback
mode, and mid-run failover. A quiet task still produces a keepalive every 30
seconds, so silence on the socket means a quiet task, not a wedged broker.

`OGA_SOCK` overrides the path. It has to exist: macOS caps a socket path at
103 bytes, and a longer one disables the socket at both ends rather than
failing at bind time — the broker warns and skips it, the client falls back.

The database path is only ever opened as an **observer**: read-only, never
created, never migrated. The broker's startup duties — profile seeding and
interrupted-task recovery — stay with the broker, because a second process
running recovery would fail every task it came to watch. A missing database, a
file that is not an Oga database, or a schema newer than the binary is exit
`2` naming the path, rather than a silently empty watch. When the socket was
skipped too, the message says why for both.

## Telling connections apart

Protocol `2026-07-28` removed MCP sessions and `Mcp-Session-Id`. Bun exposes no
socket handle through a plain HTTP `Request`, and the HTTP handler creates a
fresh MCP server for every POST. A plain POST is therefore the only connection
lifetime Oga can observe: it gets a minted consumer id and never shares a
queue with another request. No header or client configuration is required.

A `subscriptions/listen` response is different. Its open SSE stream is a real
connection owned by the SDK. Every such stream receives its own doorbell, so two
windows of the same client both hear the move and neither can consume the
other's notification.

A new connection is a new consumer. It starts at the log head and receives no
historical settlements, but every task still parked on a question is seeded
into its inbox. A stateless POST has no identity that a later POST can resume,
so only an open subscription or `oga watch` can carry non-question moves
across requests.

The tool result drains the `mcp` channel of the ledger. The desktop app drains
`app` over `GET /api/consumers/:id/inbox`, so the two never take each other's
copies, and a question waiting on both is delivered to both.

## Being told, instead of finding out

Waiting for the next tool call has one cost: a task can ask a question while the
caller is mid-turn on something else, and the answer sits there until the caller
happens to call Oga again. A client speaking protocol revision `2026-07-28`
can close that gap. It opens one subscription:

```json
{
  "method": "subscriptions/listen",
  "params": { "notifications": { "resourceSubscriptions": ["oga://attention"] } }
}
```

and from then on a task that stops arrives as a plain
`notifications/resources/updated` on that stream, the moment it happens. The
broker rings it for every question, block, failure, cancellation, and
completion, and for nothing else — progress never rings it.

The notification is a doorbell, not a message. It carries the URI and nothing
else: no task id, no title, no question text. The caller reads
`oga://attention`, and what it reads is the same block the tool result
carries, in the same words:

```text
Oga — 1 delegated task moved:
needs your input · 8f2c1a94-... · Port the parser
Call inspect with a task id to read the question or the result.
```

When there is nothing outstanding it reads `Oga — no delegated task is
waiting on you.`

**Each stream gets the doorbell.** The SDK fans a resource update to every open
subscription, including two streams with identical client metadata. A later
plain resource read is a new stateless connection: it baselines at the head and
still returns any question that remains unanswered.

**Nothing is required of the caller.** No process to run, no turn to hold, no
polling loop to write. A client that never sends `subscriptions/listen` — every
2025-era client, and `oga --stdio`, whose SDK entry exposes no publish side —
still receives open questions on its next tool call. Dropping a stream never
changes the task.

Everything on the wire here is core MCP: `subscriptions/listen`,
`resources/subscribe` capability, `notifications/resources/updated`, and a
server-defined resource URI. The SDK owns the stream, the acknowledgement, the
subscription-id stamp, and the teardown. Oga publishes one event kind and
invents no method, no notification name, and no field.

The service holds the doorbell and resource text; the Rust CLI
registers the resource and arms the doorbell on the first MCP request.
