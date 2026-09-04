# Caller cursor

> The MCP attention alert block and its caller-specific notification cursor were
> removed. This document's historical alert-delivery sections no longer describe
> a live surface; task ownership metadata remains independent of notifications.

How a task remembers who dispatched it, and how that identity replaces a
held `oga watch` process as the way an agent caller follows a task to
settlement. A record of decisions, not a tutorial — see `follow-along.md` for
how the alert block itself reads.

## The column

`tasks.caller_id` — nullable `TEXT`, migration 34 in the Rust store.
Additive only: every row dispatched before this migration keeps `caller_id =
NULL` forever, no backfill.

It holds the dispatching consumer's id, already scoped to the project it was
dispatched into (`scopedConsumerId(consumerId, cwd)` — the same scoping
`pendingAlertNotice` applies on every read). Storing the scoped id rather than
the bare one means a later poll's own scoped id can be compared to it with a
plain equality check, with no rescoping at read time.

Written once, at `createTask` (`stateStore().createTask(task, callerId)`,
threaded through `DelegateOptions.callerId` from the MCP `delegate` tool). A
task never changes owners after that — resume, handoff, and steer all act on
the same row, so the caller that dispatched a task stays its caller for the
task's whole lifetime.

Only the MCP `delegate` tool populates it today. The plain HTTP dispatch
endpoint (`POST /api/tasks`, used by `oga` orchestrator spawning) does not
compute a consumer id at all and is out of scope here; its tasks fall back to
the project-scoped broadcast below, same as any pre-migration row.

## Identity derivation

**HTTP is unchanged.** `consumerIdFor` still reads `x-oga-consumer`, then
the `User-Agent` product token, then falls back to a shared id.

**stdio no longer mints a fresh UUID per process.** The old default,
`connectionConsumerId()`, called `crypto.randomUUID()` once per
`createMcpServer()` invocation — a fresh identity every respawn, so anything
that settled while the process was down belonged to an id nothing would ever
ask for again.

The replacement, `stdioConsumerId(clientName)` in the Rust service, derives
from the MCP handshake's `clientInfo.name` instead — the one piece of
information a respawned client reports identically every time, with no header
to read on stdio the way `consumerIdFor` reads one over HTTP. It is read
lazily (`server.server.getClientVersion()?.name`, inside a closure evaluated
per tool call, not a value captured at server construction) because the
handshake completes after `createMcpServer()` returns and before any tool
call reaches it — there is no synchronous default that could see it.

Project scoping is layered on afterward by the same `scopedConsumerId` HTTP
already uses, so two different stdio clients sharing one project still get
separate queues. Only two windows of the *same* client in the *same* project
collide — the exact trade-off `consumerIdFor`'s own comment already accepts
for HTTP, chosen here for the same reason: a client's own name is the most
specific thing that survives across a respawn when nothing else does. Falls
back to a shared `mcp:stdio` bucket before any handshake has happened (tests
calling tool handlers directly, without a transport).

## Resolution order

`listAttentionEventsAfter` and `openQuestionEventIds` in the Rust store both
resolve a task's delivery destination the same way: a task whose `caller_id`
equals the polling consumer's own (already project-scoped) id belongs to that
consumer alone, regardless of what cwd the poll named. A task with `caller_id
IS NULL` falls back to the pre-existing project-scoped broadcast — every
consumer scoped to that cwd sees it, exactly as before this column existed.

This is a delivery-time filter, not a write-time fan-out: no row is
duplicated or targeted at insert. `deliveries` rows are still created lazily,
one per polling consumer, the same as before.

## What the caller pulls

The alert block every MCP tool result already carries (the Rust service,
`pendingAlertNotice`) *is* the pull mechanism — no new surface. An agent
caller reads it on its next tool call instead of holding `oga watch` open;
`oga watch` itself is untouched and stays the human-facing live view.

## Delivery filter

Unchanged by this work. `ATTENTION_EVENT_TYPES` / `ATTENTION_STATES` already
restrict what reaches `deliveries` to `needs_input`, `blocked`, `failed`,
`cancelled`, `completed` — progress and heartbeat events were never
journaled into the ledger. This change narrows *who* receives a delivery; it
does not change *what* qualifies as one.
