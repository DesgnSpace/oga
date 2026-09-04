# Event translation

Provider CLIs each speak their own JSON-lines vocabulary on stdout. The broker
stores every line as a `TaskEvent`, and the event view in `rust/crates/oga-events`
turns each one into a `TaskEventView` — the row every client (the desktop app and
`oga watch`) renders. Translation lives here, once, not in each client.

## Broker vs agent events

`event.type` starting with `agent.` is worker-originated. Anything else
(`created`, `started`, `heartbeat`, `completed`, `failed`, `handed_off`, ...)
is the broker's own lifecycle bookkeeping and takes a separate, simpler path:
a humanized title from a fixed map, one of a handful of detail strings,
`kind: "lifecycle"` or `"error"`.

## Naming a provider event

`agent.${kind}` is built by the Rust runner from each stdout line's own `type`
field (`agent.result`, `agent.tool_use`, `agent.item.completed`, ...). A line
with no `type` field at all lands as `agent.event` — this is how Antigravity
arrives, since its envelope names itself with an `event` key instead of
`type`. Claude Code hook callbacks are a separate wire path (`POST
/api/hooks/:taskId` in the Rust HTTP service) and are always stamped `agent.hook`.

`taskEventView()` does not switch on this outer type name beyond the
broker/agent split above. It reads the payload shape directly, in this order:

1. **Hook events** — any payload carrying `hook_event_name` (Claude's
   `PreToolUse`/`PostToolUse`/... hook protocol), whatever the outer event
   type says. Extracts the tool name and arguments and builds a
   `file`/`command`/`tool`/`error`/`message` row depending on the hook name
   and outcome.
2. **`knownAgentEvent()`** — payload shapes this file has a name for: Claude's
   `system`/`result` subtypes, Codex's `turn.completed`/`turn.started`/
   `thread.started`, OpenCode's `step_start`/`step_finish`, `rate_limit_event`,
   plus the two providers with an entirely private vocabulary — pi
   (`piEvent()`) and Antigravity (`antigravityEvent()`), each dispatching on
   its own envelope field (`payload.type` for pi, `payload.event` for
   Antigravity) and covering session boot, tool execution, turn/message
   boundaries, and run receipts.
3. **Generic subject-based fallback** — everything else. Pulls whichever of
   `item` (Codex), `part` (OpenCode), or a message `content` block carries the
   payload, picks a `subjectType` off it, and classifies by substring:
   `command`/`file`/`tool`/`usage`/`message`/`reason`/`error`/`tool_result`/
   `progress`. This is what makes Codex's `agent.item.*` events — its native
   representation of a tool call or a message — readable without a dedicated
   per-type handler: `item.type` values (`command_execution`, `agent_message`,
   `mcp_tool_call`, `web_search`, `todo_list`, `file_change`, `error`, ...) all
   fall through this one path and come out the other side typed.
4. **Final fallback** — `kind: "raw"`, title humanized from whatever type name
   was found on the subject, `presentation` still attached where step 3 built
   one. The row never shows the raw event type string — `agent.item.completed`
   never appears as a title, only the inner subject's own type, humanized
   (`"Todo List"`, `"Web Search"`).

## Presentation

A row's `presentation` is the chip the app draws beside the title: a file path
and diff, a command and exit code, a todo count, token/cost usage, a warning
signal. `toolPresentation()` builds it from a tool call's arguments for every
provider that names a tool by string (`bash`, `edit`, `todowrite`, an MCP
name). `subjectPresentation()` covers the item/part shapes that don't name a
tool directly — Codex's `command_execution`, `file_change`, `web_search`. Both
are consulted regardless of which classification arm a row lands in, so even a
`raw` row usually still carries a usable chip.

## MCP tool titles

`mcp__<server>__<function>` (Claude), OpenCode's flattened `server_function`,
and Codex's separate `item.server` field are all reduced to `"Server:
Function"` (`mcpToolTitle()` / `qualifiedToolTitle()`), so the same MCP call
reads the same regardless of which provider made it.

## Retryable tool failures

A content‑tool mismatch the model repairs by re‑reading and retrying — an edit
whose `oldString` no longer matches, an `apply_patch` whose context moved on,
a write to a path the disk does not hold — is not a failure of the run. In
`taskEventView()` it becomes a `retry` row: `phase: "info"`, titled by the tool
alone, with the reason carried as the result, so the trace reads it as an
ordinary step. The same tool reporting a permission, a spent budget, a rate
limit, or an auth failure stays `error`, because no re‑read of the file fixes
those; and a `read` miss is left alone, since its message is the model's own
cue to look around rather than to re‑issue the call. Oga cannot push the
miss back into the provider session, so the model's own retry loop is the
recovery; this classification only changes how the step reads.

The full‑trace endpoints (`GET /api/tasks/:id/events`) run the repeated‑failure
pass over a task's views before returning. Once the same file misses
`RETRY_REPEAT_THRESHOLD` (3) times, that retry — and every later one on the
same file — becomes an `error` row with a `nth failed attempt` detail, so a
pattern no reader should have to count for themselves surfaces as the signal it
is.

## Folding and noise

Providers report structure the reader doesn't need a row for: per-token
streaming deltas, turn/step boundaries with no payload, tool progress pings,
hook start/response plumbing. These are marked `minor: true` at the point
they're built, not filtered client-side. The desktop activity view folds them
into one line ("Thinking · N updates") off that flag. New events do not need a
legacy `plumbing` title list.

Rows that describe one action — a tool call, a subagent spawn, an MCP request
— carry an `actionId` (Claude's `tool_use_id`, OpenCode's `callID`, Codex's
`item.id`, Antigravity's `conversation_id:step_index`, pi's `toolCallId`), so
the several events one action produces (the call, a progress tick, the
result) fold onto a single line instead of stacking as separate rows.

## Provider vocab, one line each

| Provider | Native shapes | Handled by |
|---|---|---|
| Claude | hook protocol (`hook_event_name`), `system` subtypes, `result`, `tool_use_meta` | hook branch, `knownAgentEvent()` |
| OpenCode | `part`-shaped `tool`/`tool_use`, `step_start`/`step_finish` | generic fallback, `knownAgentEvent()` |
| Codex | `agent.item.*` (`command_execution`, `agent_message`, `mcp_tool_call`, `web_search`, `todo_list`, `file_change`, `error`), `turn.completed`, `thread.started` | generic fallback, `knownAgentEvent()` |
| pi | `session`, `message_update` deltas, `tool_execution_*`, `message_start`/`message_end`, `turn_end`, `agent_settled` | `piEvent()` |
| Antigravity | `init`, `step_update` (`tool`/`agent_response`/`checkpoint`/`user_input`), `result` | `antigravityEvent()` |

The `retry` kind is produced by the shared retryable‑tool classifier and is
not provider‑specific; any provider's edit or patch miss routes through it.

## Adding a new provider or event shape

Give a new shape a named branch instead of letting it fall through — the
generic fallback exists to keep an unrecognized event visible, never silently
dropped, not as its intended long-term home. Every field added to
`TaskEventPresentation` or `TaskEventView` needs matching fields in the desktop
app's snapshot types: a field the broker sends with nothing to decode it into
is silently dropped, not an error.
