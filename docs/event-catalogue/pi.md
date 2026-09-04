# Pi Activity Event Catalogue

Field-level reference for how Pi streaming events must be read and collapsed into activity rows.

## Event Types Overview

Pi emits ~45 distinct event types. This catalogue covers the 15 most frequent, which account for ~98% of volume. The remaining ~30 types (`heartbeat`, `started`, `worker_spawned`, `blocked`, `needs_input`, etc.) are non-activity events; see [Non-Activity Events](#non-activity-events).

**Volume summary (rows as of 2026-08-31):**

- `agent.message_update`: 35,394 (streaming deltas)
- `agent.message_start`: 2,943
- `agent.message_end`: 2,929
- `agent.tool_execution_update`: 2,221 (streaming deltas)
- `agent.tool_execution_start`: 1,609
- `agent.tool_execution_end`: 1,594
- `agent.turn_start`: 1,282
- `agent.turn_end`: 1,245
- `agent.agent_start`: 81
- `agent.agent_end`: 36
- `agent.agent_settled`: 58
- `agent.session`: 69
- `agent.auto_retry_start`: 13
- `agent.auto_retry_end`: 9
- `agent.event`: 10
- `agent.tool_use`: 4

---

## Core Pattern: Message Streaming

Messages are delivered in three phases: **start** (setup), **update** (streaming deltas), **end** (completion or interruption).

**Collapse rule:** One message produces one row. The row opens on `message_start`, updates on every `message_update`, and closes on `message_end` or task interruption.

### agent.message_start

**Payload structure:**
```json
{
  "type": "message_start",
  "message": {
    "role": "user" | "assistant" | "toolResult",
    "content": [/* content array */],
    // ... metadata if role is "assistant"
  }
}
```

**JSON paths:**
- **Event name:** `$.type` → `"message_start"`
- **Message role:** `$.message.role` → determines row type (prompt, response, tool result)
- **Content preview:** `$.message.content[0].text` or first non-empty content item

**Row behavior:**
- Opens a new row with `title` derived from role and content preview
- Sets `verb` and `target` based on message context (e.g., for a user prompt containing a tool name, extract the tool name as target)

**Example row title:** "User message" or "Assistant response started"

---

### agent.message_update

**Payload structure:**
```json
{
  "type": "message_update",
  "assistantMessageEvent": {
    "type": "thinking_end" | "thinking_start" | "text_delta" | "toolcall_start" | "toolcall_delta" | "toolcall_end",
    "content": "...", // partial content
    "contentIndex": 0, // index into message.content array (for streaming)
    "delta": "..." // incremental text for toolcall_delta
  }
}
```

**JSON paths:**
- **Event name:** `$.type` → `"message_update"`
- **Delta type:** `$.assistantMessageEvent.type` → tells renderer what kind of delta this is
- **Delta content:** `$.assistantMessageEvent.delta` or `$.assistantMessageEvent.content`

**Row behavior:**
- Updates the most recent open message row (matched by title or actionId)
- For streaming UI: accumulate deltas into the row's live content display
- The row's detail grows as updates arrive; renderer appends or reformats based on delta type

**Streaming delta types:**
- `thinking_start` / `thinking_end`: Marks reasoning boundaries
- `text_delta`: Append text to output
- `toolcall_start`: Starts a tool invocation block
- `toolcall_delta`: Accumulates tool call JSON (arguments)
- `toolcall_end`: Closes the tool invocation block

**Example sequence:**
1. `message_start` with role=assistant, empty content
2. Multiple `message_update` with `thinking_start`, `thinking_end`, `text_delta` events
3. `message_end` with complete message or no more updates (interrupted)

**Critical:** If a run is interrupted (broker restarts, worker dies), the stream may stop at any `message_update`. A renderer that only sees deltas without `message_end` must close the row as incomplete.

---

### agent.message_end

**Payload structure:**
```json
{
  "type": "message_end",
  "message": {
    "role": "user" | "assistant" | "toolResult",
    "content": [/* complete final content array */],
    // ... full metadata for assistant or toolResult roles
  }
}
```

**JSON paths:**
- **Event name:** `$.type` → `"message_end"`
- **Full content:** `$.message.content` → complete accumulated content
- **Role:** `$.message.role` → determines message type

**Row behavior:**
- Closes the open message row by patching with final content
- Sets `complete: true` on the row
- If no `message_end` arrived (run was interrupted), the row remains incomplete based on the last `message_update`

**Example row closure:** "Assistant response completed" or "Tool result delivered"

---

## Core Pattern: Tool Execution

Tool calls stream in three phases: **start**, **update** (streaming deltas), **end** (completion or interruption).

**Collapse rule:** One tool execution produces one row. The row opens on `tool_execution_start`, updates on every `tool_execution_update`, and closes on `tool_execution_end` or task interruption. The `toolCallId` pairs all three events.

### agent.tool_execution_start

**Payload structure:**
```json
{
  "type": "tool_execution_start",
  "toolCallId": "call_00_XXX...",
  "toolName": "bash" | "read" | "edit" | "write" | "mcp" | "mcp_script",
  "args": {
    // Tool-specific arguments; structure depends on toolName
    "path": "...",        // read, edit, write
    "command": "...",     // bash
    "server": "...",      // mcp
    "edits": [...]        // edit (array of oldText/newText pairs)
  }
}
```

**JSON paths:**
- **Event name:** `$.type` → `"tool_execution_start"`
- **Tool ID:** `$.toolCallId` → pairs with update and end events; use as `actionId` in row
- **Tool name:** `$.toolName` → determines verb and row type
- **Target:** varies by tool:
  - `read`, `write`, `edit`: `$.args.path`
  - `bash`: `$.args.command` (truncate to ~100 chars for title)
  - `mcp`: `$.args.server` + method name
  - `mcp_script`: `$.args.script`

**Row behavior:**
- Opens with title: `{verb} {target}` where verb comes from TOOL_VERBS and target from args
- Sets `verb` (e.g., "Reading", "Running", "Editing"), `target` (file path or command), `kind` ("tool")
- Sets `actionId` to `toolCallId` and `sourceId` to `toolCallId`

**Example rows:**
- "Read `src/main.ts`"
- "Run `ls /tmp`"
- "Edit `src/app.ts` (3 changes)"

---

### agent.tool_execution_update

**Payload structure:**
```json
{
  "type": "tool_execution_update",
  "toolCallId": "call_00_XXX...",
  "toolName": "bash" | "read" | "...",
  "args": { /* same as start */ },
  "partialResult": {
    "content": [ /* accumulated output so far */ ]
  }
}
```

**JSON paths:**
- **Event name:** `$.type` → `"tool_execution_update"`
- **Tool ID:** `$.toolCallId` → use to match against the open row (via `actionId`)
- **Streaming output:** `$.partialResult.content[0].text` (first text block of accumulated result)

**Row behavior:**
- Updates the row opened by `tool_execution_start` (matched via `toolCallId` / `actionId`)
- Accumulates output into the row's `detail` or presentation
- For streaming UI: append new output chunks to the live display

**Note:** `tool_execution_update` only appears for streaming tools (bash, mcp). File operations (read, edit, write) typically have start → end with no updates, or updates with empty `partialResult`.

---

### agent.tool_execution_end

**Payload structure:**
```json
{
  "type": "tool_execution_end",
  "toolCallId": "call_00_XXX...",
  "toolName": "bash" | "read" | "...",
  "result": {
    "content": [ /* final result */ ]
  }
}
```

**JSON paths:**
- **Event name:** `$.type` → `"tool_execution_end"`
- **Tool ID:** `$.toolCallId` → matches start/update events
- **Final output:** `$.result.content[0].text` (first text block of final result)
- **Exit code (bash only):** may be in structured result; look for `exitCode` field in result metadata

**Row behavior:**
- Closes the row opened by `tool_execution_start` (matched via `actionId`)
- Patches the row with final result; sets `complete: true`
- If no `tool_execution_end` arrived (run was interrupted), the row remains incomplete based on last `tool_execution_update` or `tool_execution_start`

**Exit code detail:** For bash results, extract exit code if present and include in row detail (e.g., "exit 1" or "success").

---

## Tool Types: Detailed Cases

Each tool has a distinct row presentation. These are the tools actually observed in Pi data:

### bash

**start args:**
```json
{ "toolCallId": "...", "toolName": "bash", "args": { "command": "ls -la /tmp" } }
```

**Payload paths:**
- Target: `$.args.command` (full command, truncate to ~100 chars in title)
- Output: `$.partialResult.content[0].text` (bash stdout)

**Row detail:** Command and first line or line count of output
**Example:** "Run `curl -s https://example.com` (200 chars output)"

---

### read

**start args:**
```json
{ "toolCallId": "...", "toolName": "read", "args": { "path": "/path/to/file.ts" } }
```

**Payload paths:**
- Target: `$.args.path` (file path)
- Content: `$.result.content[0].text` (file contents, truncate preview to ~200 chars)

**Row detail:** File path and line count or first few lines
**Example:** "Read `src/types.ts` (150 lines)"

---

### edit

**start args:**
```json
{
  "toolCallId": "...",
  "toolName": "edit",
  "args": {
    "path": "/path/to/file.ts",
    "edits": [
      { "oldText": "...", "newText": "..." },
      { "oldText": "...", "newText": "..." }
    ]
  }
}
```

**Payload paths:**
- Target: `$.args.path` (file path)
- Number of edits: `$.args.edits.length`
- Change summary: derive from edits (e.g., "3 replacements")

**Row detail:** File path and number of changes
**Example:** "Edit `src/app.ts` (3 changes)"

---

### write

**start args:**
```json
{ "toolCallId": "...", "toolName": "write", "args": { "path": "/path/to/file.ts" } }
```

**Payload paths:**
- Target: `$.args.path`
- Content: `$.result.content[0].text` (file contents written)

**Row detail:** File path
**Example:** "Write `/tmp/output.json`"

---

### mcp

**start args:**
```json
{
  "toolCallId": "...",
  "toolName": "mcp",
  "args": { "server": "oga-database-local", "tool": "list_tools" }
}
```

**Payload paths:**
- Server: `$.args.server`
- Tool/method: `$.args.tool` or similar field indicating the MCP method

**Row detail:** Server and tool name
**Example:** "Call MCP `oga-database-local`.`list_tools`"

---

### mcp_script

**start args:**
```json
{
  "toolCallId": "...",
  "toolName": "mcp_script",
  "args": { "script": "..." }
}
```

**Payload paths:**
- Script excerpt: `$.args.script` (first line or entire if short)

**Row detail:** Script name or first statement
**Example:** "Run script"

---

## Structural Events: Turn, Agent, Session

These events mark boundaries and lifecycle transitions. They do not produce activity rows (they are metadata events), but renderers should understand their roles:

### agent.turn_start

**Payload:** `{ "type": "turn_start" }`

**Meaning:** A new turn (round of agent work) has begun.

**Row behavior:** Do not produce a row. This is a boundary marker. A renderer may use it to group or separate activity.

---

### agent.turn_end

**Payload:**
```json
{
  "type": "turn_end",
  "message": {
    "role": "assistant",
    "content": [ /* final turn output, including all tool calls made */ ]
  }
}
```

**Meaning:** The turn has completed. The `message` contains the agent's final output for this turn.

**Row behavior:** Do not produce a separate row. The turn's work is already captured by the individual message and tool execution rows that occurred during the turn.

---

### agent.agent_start

**Payload:** `{ "type": "agent_start" }`

**Meaning:** An agent (e.g., a subagent or delegated worker) has started running.

**Row behavior:** Do not produce a row. This marks a lifecycle boundary.

---

### agent.agent_end

**Payload:**
```json
{
  "type": "agent_end",
  "messages": [ /* agent's final state */ ]
}
```

**Meaning:** An agent has finished (without necessarily settling fully).

**Row behavior:** Do not produce a row. Use as context for understanding work grouping.

---

### agent.agent_settled

**Payload:** `{ "type": "agent_settled" }`

**Meaning:** An agent has settled — all work is complete, result is final.

**Row behavior:** Do not produce a row. This is a completion marker.

---

### agent.session

**Payload:**
```json
{
  "type": "session",
  "id": "019fcf62-9131-7ce9-b40e-919c79c73c69",
  "version": 3,
  "timestamp": "2026-08-05T00:46:11.249Z",
  "cwd": "/Users/malico/desgn/oga"
}
```

**Meaning:** A provider session was captured or initialized.

**Row behavior:** Do not produce a row. This is a session boundary marker.

---

## Retry Events

### agent.auto_retry_start

**Payload:**
```json
{
  "type": "auto_retry_start",
  "attempt": 1,
  "maxAttempts": 3,
  "delayMs": 2000,
  "errorMessage": "Stream ended without finish_reason"
}
```

**Meaning:** An automatic retry has begun due to a failure.

**Row behavior:** Render as a minor informational row: "Retry attempt 1/3: Stream ended without finish_reason" if detail is needed, or omit as `minor: true`.

---

### agent.auto_retry_end

**Payload:**
```json
{
  "type": "auto_retry_end",
  "success": true,
  "attempt": 1
}
```

**Meaning:** The retry completed (success or failure depends on `success` field).

**Row behavior:** Do not produce a separate row if `success: true`. If `success: false`, emit a warning row or set `failed: true` on the preceding attempt row.

---

## Rarely-Occurring Events

### agent.event

**Payload:** Varies by sub-type. Often contains init information for a Gemini or other LLM session.

**Meaning:** A low-level provider event that does not map to a standard Pi action.

**Row behavior:** Mark as `kind: "raw"` or `kind: "error"` depending on content. Emit with low detail unless explicitly requested; this is reference data, not activity.

---

### agent.tool_use

**Payload:** Contains `type`, `timestamp`, `sessionID`, `part` (with `tool`, `callID`, `state`).

**Meaning:** A tool was invoked by the agent. This is a raw event format that differs from `tool_execution_*` structure.

**Row behavior:** Treat as a variant of `tool_execution_start`. Extract tool name from `$.part.tool` and call ID from `$.part.callID`. The `state` field contains input/output.

---

## Non-Activity Events

The following event types should **never** produce rows:

| Event Type | Reason |
| --- | --- |
| `heartbeat` | Periodic keep-alive, no activity |
| `started` | Task lifecycle marker |
| `created` | Task lifecycle marker |
| `completed` | Task lifecycle marker (broker handles settle) |
| `failed` | Task lifecycle marker (broker handles settle) |
| `cancelled` | Task lifecycle marker (broker handles settle) |
| `blocked` | Task state change (broker handles) |
| `needs_input` | Task state change (broker handles) |
| `answered` | Task state change (broker handles) |
| `worker_spawned` | Lifecycle marker |
| `session_captured` | Session boundary marker |
| `session_reused` | Session boundary marker |
| `event_dropped` | Size overflow marker |
| `events_truncated` | Trace truncation marker |
| `resumed` | Lifecycle marker |
| `handed_off` | Handoff boundary marker |
| `handoff_brief` | Handoff boundary marker |
| `broker_restarted` | Broker lifecycle |
| `completion_asserted` | Completion marker |
| `scope_refusal` | Permission marker |
| `effort_mismatch` | Config marker |

---

## Streaming Collapse Algorithm

When rendering the event trace, a renderer must collapse streaming deltas into single rows:

### Messages

1. When `message_start` arrives: create a new row with title, verb, target (extracted from prompt if present).
2. For each `message_update`: patch the most recent incomplete message row (matched by `actionId` or title proximity). If no row exists, this is an orphaned delta — log as error.
3. When `message_end` arrives: mark the row complete and patch with final content.
4. If run interrupts (no more events after `message_update`): check task state. If task is settled or cancelled, close the row as incomplete.

### Tool Executions

1. When `tool_execution_start` arrives: create a new row with title (verb + target), `actionId` = `toolCallId`.
2. For each `tool_execution_update`: find the row by `actionId` and patch with partial output.
3. When `tool_execution_end` arrives: mark the row complete and patch with final result.
4. If run interrupts: close the row as incomplete based on last update.

### Matching Open Rows

- **Primary:** Use `actionId` / `sourceId` field. If both events carry the same ID, they refer to the same action.
- **Fallback:** If IDs are absent or conflict, match by creation time (newest incomplete row with matching type).
- **Title proximity:** When matching messages, prefer rows whose title contains a keyword from the new event payload (e.g., if a tool call comes next, the message title might name the tool).

---

## Ambiguities and Gotchas

### 1. Interrupted Runs: No message_end

If a task is interrupted (worker crash, broker restart, timeout), the event stream may stop during `message_update` events with no `message_end`. A renderer seeing only deltas must:

- Track the row as incomplete.
- Check the task's final state: if settled or cancelled, close the row and mark incomplete.
- If task is still running, leave the row open (more may arrive).

**What NOT to do:** Never emit an extra "message timed out" or "interrupted" row — the absence of `message_end` itself is the signal.

### 2. Tool Output Across Multiple Content Blocks

Some tools (especially bash) may return output split across multiple content blocks in the result:

```json
{
  "result": {
    "content": [
      { "type": "text", "text": "First block\n" },
      { "type": "text", "text": "Second block\n" }
    ]
  }
}
```

**Rule:** Concatenate all text blocks in order. Show first N lines (e.g., first 3 lines or 200 chars) in the row detail; offer a "show more" or full-text link.

### 3. Message Role Changes: toolResult Roles

When a tool completes, the next message's role is often `toolResult`. The content is the result from that tool execution:

```json
{ "type": "message_start", "message": { "role": "toolResult", "toolCallId": "call_00_...", "content": [...] } }
```

**Row title:** Derive from the tool call ID — match back to the tool row and say "Result from `{tool} {target}`" rather than a generic "Tool result".

### 4. Edit vs. Write vs. Read Ambiguity

All three manipulate files; they differ in directionality:

- `read`: Input. Target is the file being read.
- `write`: Output. Target is the file being written (created or replaced).
- `edit`: Output. Target is the file being modified. Payload includes oldText/newText pairs; emit change count.

**Row titles must distinguish them:**
- "Read `file.ts`"
- "Write `file.ts`"
- "Edit `file.ts` (2 changes)"

### 5. bash: Command vs. Exit Code vs. Output

A bash tool row includes:

- **Verb:** "Running"
- **Target:** The command (truncate to ~100 chars for legibility)
- **Detail:** Output preview or exit code
  - If exit code is 0: "success" or omit detail
  - If exit code is non-zero: "exit {code}" and possibly error output
  - If output is long: first N lines or character count

**Common case:** A failed command may produce no stdout but writes to stderr in the result. Prioritize stderr in error cases.

### 6. Agent Subagent Work

When the agent spawns a subagent (delegated task), Pi emits:

- `agent.agent_start` (subagent started)
- Multiple message/tool rows from the subagent's work
- `agent.agent_end` (subagent finished)
- `agent.agent_settled` (subagent fully settled)

**Row organization:** Group these rows as a collapsible section or nested timeline; title it by the delegated task's summary.

---

## Summary: Distinct Payload Shapes Found

**Streaming vs. Non-Streaming:**

- Streaming: `message_update` (35,394 rows), `tool_execution_update` (2,221 rows) — collapse these into parent message/tool rows.
- Non-streaming: All others — map directly to rows or skip (per type).

**Tools appearing:**

- `bash` (802 starts): Commands with output and exit code.
- `read` (546 starts): File reads.
- `edit` (193 starts): File edits with change counts.
- `write` (38 starts): File writes.
- `mcp_script` (16 starts): Script execution.
- `mcp` (14 starts): MCP tool invocation.

**Incomplete messages observed:**

- ~98% of tasks have matching `message_start` and `message_end` counts.
- ~2% have more starts than ends (e.g., 47 vs. 45), indicating interrupted streams.
- No orphaned `message_end` without a prior `message_start` was found.

**Actionable findings causing wrong rows today:**

1. **message_update deltas are rendered as separate rows.** Solution: Accumulate deltas under the most recent `message_start` row, not as standalone rows.
2. **tool_execution_update has no display logic.** Solution: Route to the parent `tool_execution_start` row (via `toolCallId` matching).
3. **Incomplete messages (interrupted runs) are displayed as unclosed rows.** Solution: Check task state and explicitly close incomplete rows marked by missing `message_end`.
4. **Tool output is not truncated or summarized.** Solution: Show first N lines or byte summary in detail; link to full output.
5. **Different tool types use inconsistent row titles.** Solution: Enforce "verb target (detail)" pattern per tool name (use TOOL_VERBS table).
