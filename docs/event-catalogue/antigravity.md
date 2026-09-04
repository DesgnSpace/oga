# Antigravity Event Catalogue

**Dataset scope:** Antigravity profile events from `~/.oga/oga.db`, `task_events` table. Survey covers 689 `agent.event`, 202 `agent.system`, 56 `agent.hook`, 47 `agent.assistant`, 27 `agent.user`, 14 `agent.rate_limit_event`, and 2 `agent.result` rows.

## Summary: The Discriminator

Antigravity's real event taxonomy is **not** in `event_type` — it is hidden inside the `agent.event` payload. The discriminator field is **`payload.event`** (a string). Three sub-kinds exist:

- `step_update` (611 rows) — granular steps within a run, subdivided by `payload.step_update.step_type`
- `result` (50 rows) — final outcome of a conversation
- `init` (28 rows) — run initialization with model and config

## Event Types That Produce Rows

### 1. `agent.event` — the main carrier (689 total)

#### Discriminator: `payload.event` field

**Sub-kind: `step_update` (611 rows)**

The step_type distinguishes the activity:

##### `step_type: agent_response` (319 rows)

**Row text:** `Assistant responded in 6.3s (29k → 818 tokens, cached 24k)`

- **Event name:** `$.event` → `"step_update"`
- **Sub-kind:** `$.step_update.step_type` → `"agent_response"`
- **Target (meaningful context):** `$.step_update.conversation_id` (conversation UUID; no human file/command/resource path for this step type)
- **Row detail fields:**
  - Duration: `$.step_update.duration_seconds` (e.g., 6.270468)
  - Token usage: `$.step_update.usage.output_tokens`, `.input_tokens`, `.thinking_tokens`, `.cache_read_tokens`, `.total_tokens`
  - Cache metrics included in usage object
- **Correlation ID:** `$.step_update.step_index` (0-indexed step in sequence; use with `conversation_id` to order steps)
- **Timing:** `created_at` from task_events row

##### `step_type: tool` (190 rows)

Tool invocations have three lifecycle states per tool_info instance: `ACTIVE` (invoked), `DONE` (result received), and implicit initial state. Each tool has different parameter names for its target.

**Row text examples by tool:**
- `view_file`: `Read docs/dev-tools-in-sandbox.md (224 lines, 13.7 KiB)` [DONE] or `Reading docs/dev-tools-in-sandbox.md` [ACTIVE]
- `run_command`: `Run: gh --version; gh auth status; gh api user; git push --dry-run origin HEAD` [ACTIVE/DONE]
- `write_to_file`: `Write docs/gh-in-sandbox.md` [ACTIVE/DONE]
- `replace_file_content`: `Edit /Users/malico/desgn/pluk/pluk/src/ssh/pending.ts` [ACTIVE/DONE]
- `search_web`: `Search: github cli gh config directory hosts.yml...` [ACTIVE/DONE]
- `grep_search`: `Grep: SSH_AGENT_DENIED_CODE in /Users/malico/desgn/pluk/pluk/src` [ACTIVE/DONE]
- `list_dir`: `List /Users/malico/.gemini/antigravity-cli/mcp` [ACTIVE/DONE]
- `manage_task`: `Task status: d6d90a98-5cc0-4eaf-b131-95b3149d394f/task-76` [ACTIVE/DONE]
- `schedule`: `Schedule check in 90s for task d6d90a98-5cc0-4eaf-b131-95b3149d394f/task-76` [ACTIVE/DONE]
- `list_permissions`: `Check permissions` [ACTIVE/DONE]

**Event name:** `$.event` → `"step_update"`
**Sub-kind:** `$.step_update.step_type` → `"tool"`
**Tool discriminator:** `$.step_update.tool_name`
**Tools found:** `view_file` (52), `run_command` (52), `write_to_file` (30), `replace_file_content` (20), `search_web` (18), `grep_search` (2), `list_dir` (10), `manage_task` (2), `schedule` (2), `list_permissions` (2)

**Target by tool:**
- `view_file`: `$.step_update.tool_info.parameters.AbsolutePath`
- `run_command`: `$.step_update.tool_info.parameters.CommandLine`
- `write_to_file`: `$.step_update.tool_info.parameters.TargetFile`
- `replace_file_content`: `$.step_update.tool_info.parameters.TargetFile`
- `search_web`: `$.step_update.tool_info.parameters.query`
- `grep_search`: `$.step_update.tool_info.parameters.SearchPath` (directory), `Query` (search term) — target is the combination
- `list_dir`: `$.step_update.tool_info.parameters.DirectoryPath`
- `manage_task`: `$.step_update.tool_info.parameters.TaskId`, `Action`
- `schedule`: `$.step_update.tool_info.parameters.TimerCondition` (task ID), `DurationSeconds`
- `list_permissions`: no parameters

**Row detail fields (present when DONE):**
- Duration: `$.step_update.duration_seconds`
- Tool output: `$.step_update.tool_info.output` (truncated or full result; may be multiline or empty)

**State progression:** `$.step_update.state` → `"ACTIVE"` or `"DONE"`

**Correlation ID:** `$.step_update.step_index` (orders steps within conversation) + `$.step_update.conversation_id`

##### `step_type: user_input` (28 rows)

User-provided input or message content during the run.

**Row text:** `User input (step 0)` or similar brief marker

- **Event name:** `$.event` → `"step_update"`
- **Sub-kind:** `$.step_update.step_type` → `"user_input"`
- **Target:** `$.step_update.conversation_id` (no file/command/resource; conversation context only)
- **Row detail:** Minimal; step_index is the detail
- **Correlation ID:** `$.step_update.step_index` + `$.step_update.conversation_id`

##### `step_type: checkpoint` (24 rows)

Intermediate evaluation or state save.

**Row text:** `Checkpoint at step 4 (1.0s, 8689 → 5 tokens)`

- **Event name:** `$.event` → `"step_update"`
- **Sub-kind:** `$.step_update.step_type` → `"checkpoint"`
- **Target:** `$.step_update.conversation_id`
- **Row detail:** `duration_seconds`, `usage` (input_tokens, output_tokens, etc.)
- **Correlation ID:** `$.step_update.step_index` + `$.step_update.conversation_id`

##### `step_type: unknown` (25 rows)

Unclassified step; likely provider-specific internal events.

**Row text:** `Unknown step 1` or omit entirely (debatable whether this produces a row)

- **Event name:** `$.event` → `"step_update"`
- **Sub-kind:** `$.step_update.step_type` → `"unknown"`
- **Target:** `$.step_update.conversation_id`
- **Row detail:** minimal; step_index and duration if present
- **Correlation ID:** `$.step_update.step_index` + `$.step_update.conversation_id`
- **Recommendation:** Consider **not producing a row** for `unknown`. A user facing "Unknown step" is confused. If you must show it, explain why in a comment or omit it silently.

##### `step_type: error_message` (18 rows)

Error or exception during execution.

**Row text:** `Error at step 40` (actual message content not in payload sample; truncated)

- **Event name:** `$.event` → `"step_update"`
- **Sub-kind:** `$.step_update.step_type` → `"error_message"`
- **Target:** `$.step_update.conversation_id`
- **Row detail:** step_index, state (DONE)
- **Correlation ID:** `$.step_update.step_index` + `$.step_update.conversation_id`
- **Note:** Full error text not sampled; likely in message content elsewhere in the transcript.

##### `step_type: system_message` (7 rows)

System-generated informational message (e.g., rate limit warning, resource limit).

**Row text:** `System message at step 89`

- **Event name:** `$.event` → `"step_update"`
- **Sub-kind:** `$.step_update.step_type` → `"system_message"`
- **Target:** `$.step_update.conversation_id`
- **Row detail:** step_index
- **Correlation ID:** `$.step_update.step_index` + `$.step_update.conversation_id`

---

**Sub-kind: `result` (50 rows within agent.event)**

Final outcome and summary of a completed conversation.

**Row text:** `Completed "Research GH_CONFIG_DIR bypass" (6.4s, 16 turns, $0.06)` [SUCCESS] or `NEEDS_INPUT: Which natural language?` [requested input] or `Failed: Session limit exceeded` [error]

- **Event name:** `$.event` → `"result"`
- **Status discriminator:** `$.result.status` → `"SUCCESS"` (most common; check data for others)
- **Target (conversation summary):** `$.result.response` (contains conversation outcome, TL;DR text, OGA_NEEDS_INPUT markers, error message; truncated in payload)
- **Row detail fields:**
  - Conversation ID: `$.result.conversation_id`
  - Duration: `$.result.duration_seconds`
  - Turn count: `$.result.num_turns`
  - Token usage: `$.result.usage` (input_tokens, output_tokens, thinking_tokens, cache_read_tokens, total_tokens)
- **Correlation ID:** `$.result.conversation_id` (marks end of conversation started by init event with same ID)
- **Timing:** `created_at` from task_events row

---

**Sub-kind: `init` (28 rows within agent.event)**

Run initialization; configuration and tool availability at start.

**Row text:** `Start gemini-3.6-flash-medium in /Users/malico/desgn/oga` [show model, not full tool list]

- **Event name:** `$.event` → `"init"`
- **Metadata:**
  - Model: `$.init.model`
  - Working directory: `$.init.cwd`
  - Available tools: `$.init.tools` (array of strings)
  - Permission mode: `$.init.permission_mode` (e.g., `"always-proceed"`)
  - Conversation ID: `$.conversation_id` (at top level)
- **Target:** `$.init.cwd` (working directory is the human context; model is detail)
- **Row detail:** Model name, tool count, permission mode
- **Correlation ID:** `$.conversation_id` (start of conversation; matched with result sub-kind having same ID)

---

### 2. `agent.system` (202 rows)

System-level events about provider startup, connection, model availability, and Antigravity harness state.

**Common structure:** `$.type` field discriminates sub-type (e.g., `"system"`, subtype further refined by `$.subtype`).

**Sample found:** `subtype: "init"` → provider initialization event with session_id, cwd, model, list of tools, MCP server statuses, slash commands available.

**Row text:** `System: Initialized with Claude Sonnet 5, 42 tools, 3 MCP servers`

- **Event name:** `$.type` → `"system"`
- **Target:** `$.cwd`
- **Row detail:** `model`, `tools` (count), `mcp_servers` (array with name and status)
- **Correlation ID:** `$.session_id` (session-scoped; distinct from conversation_id in agent.event)

---

### 3. `agent.hook` (56 rows)

Hook-triggered events capturing pre/post tool invocation or permission gates.

**Sample found:** `hook_event_name: "PreToolUse"` → tool use about to happen.

**Row text:** `Hook: Before bash command 'grep -rn SSH_AGENT_DENIED...'`

- **Event name:** `$.hook_event_name` (e.g., `"PreToolUse"`)
- **Tool discriminator:** `$.tool_name`
- **Target:** `$.tool_input` (the command/parameters being invoked; structure varies by tool)
  - Bash: `$.tool_input.command`
  - Others: tool-specific structure
- **Row detail:** tool name, truncated input
- **Correlation ID:** `$.tool_use_id` (pairs with corresponding tool_result in agent.user messages if present)
- **Session context:** `$.session_id`, `$.cwd`

---

### 4. `agent.user` (27 rows)

User-provided responses, tool results, and agent interventions.

**Common structure:** `$.type` → `"user"`, `$.message` contains `role: "user"` and `content` array.

**Row text:** (varies widely; tool results are large; typically don't produce rows, or produce a single row summarizing tool invocation + result)

- **Event name:** `$.type` → `"user"`
- **Message content discriminator:** `$.message.content[*].type`
  - `"tool_result"` — result of a tool invocation
    - Target: `$.message.content[*].tool_use_id` (pairs hook to result)
    - Content: `$.message.content[*].content` (truncated output, e.g., grep result)
  - `"text"` — plain text (if present; not in current samples)
- **Row detail:** tool_use_id, truncated result content
- **Correlation ID:** `$.tool_use_id` or `$.parent_tool_use_id` (chain tool invocations)
- **Session context:** `$.session_id`
- **Recommendation:** `agent.user` with tool_results may not need separate rows — they are already part of the tool step_update rows they respond to. Clarify with renderer author.

---

### 5. `agent.assistant` (47 rows)

Assistant (model) responses containing text, tool calls, and thinking.

**Common structure:** `$.type` → `"assistant"`, `$.message` contains model response with thinking, text, and tool_use objects.

**Row text:** (internal message routing; typically omitted from user-facing activity rows)

- **Event name:** `$.type` → `"assistant"`
- **Message structure:** `$.message.content` (array)
  - `"type": "thinking"` — internal reasoning (may be base64-encoded or empty)
  - `"type": "text"` — response text
  - `"type": "tool_use"` — tool invocation
- **Row detail:** presence of thinking, token usage from usage field
- **Recommendation:** `agent.assistant` messages are intermediate protocol events. **Do not produce user-facing rows** — the actual tool invocations are captured by `step_update.tool`, and the text by agent_response.

---

### 6. `agent.rate_limit_event` (14 rows)

Rate limit warning or status.

**Sample found:** `status: "allowed_warning"`, `utilization: 0.91`, `resetsAt: 1786115400` (Unix timestamp).

**Row text:** `⚠️ Rate limit: 91% used, resets at 4:10pm` or `Rate limit warning`

- **Event name:** `$.type` → `"rate_limit_event"`
- **Status:** `$.rate_limit_info.status` (e.g., `"allowed_warning"`, `"allowed_critical"`)
- **Details:**
  - Utilization: `$.rate_limit_info.utilization` (0.0–1.0 fraction)
  - Rate limit type: `$.rate_limit_info.rateLimitType` (e.g., `"five_hour"`)
  - Resets at: `$.rate_limit_info.resetsAt` (Unix timestamp)
  - Using overage: `$.rate_limit_info.isUsingOverage` (boolean)
  - Threshold surpassed: `$.rate_limit_info.surpassedThreshold` (e.g., 0.9 = 90%)
- **Row detail:** status, utilization percentage, reset time
- **Importance to user:** Rate limits are actionable — if 91% used, the user may want to pause. Show this row.

---

### 7. `agent.result` (2 rows, top-level event_type, distinct from agent.event.result)

Provider-level result metadata (different structure than agent.event.result).

**Sample found:** `is_error: true`, `duration_api_ms`, `num_turns`, `terminal_reason: "api_error"`, `api_error_status: 429`.

**Row text:** `Provider error: Session limit exceeded (session: b6219fac, 16 turns, $0.93)` [for error] or omit [for success]

- **Event name:** `event_type` → `"agent.result"`
- **Status discriminator:** `$.is_error` (boolean)
- **Details:**
  - Session ID: `$.session_id`
  - Duration: `$.duration_api_ms` or `$.duration_ms`
  - Turn count: `$.num_turns`
  - Cost: `$.total_cost_usd`
  - Stop reason: `$.stop_reason`
  - Terminal reason: `$.terminal_reason` (e.g., `"api_error"`, `"stop_sequence"`)
  - Error status: `$.api_error_status` (HTTP status code; 429 = rate limit)
  - Usage: nested `$.usage` with token counts, cache stats, model breakdowns
  - Subtype: `$.subtype` (e.g., `"success"` even if is_error is true — conflicting signals; clarify with Antigravity)
- **Row detail:** terminal reason, cost, turn count, error status if present
- **Correlation ID:** `$.session_id` (session-scoped, not conversation-scoped)
- **Recommendation for rows:** Show only if `is_error: true` or `terminal_reason` indicates an anomaly. Success runs may be silent.

---

## Event Types That Should NOT Produce Rows

The following event types carry metadata and housekeeping information, not user-visible work:

- **`heartbeat`** (235 rows) — keep-alive signal; no user action to display
- **`worker_spawned`** (55 rows) — worker lifecycle; internal routing
- **`started`** (47 rows) — task state transition; separate from event catalogue
- **`created`** (42 rows) — resource creation metadata; show in resource view, not activity log
- **`archived`** (42 rows) — archive action; separate from event catalogue
- **`session_captured`** (20 rows) — session snapshot; internal
- **`failed`**, **`blocked`**, **`completed`**, **`needs_input`**, **`answered`**, **`handed_off`**, **`resumed`**, **`cancelled`**, **`provider_retry`**, **`scope_inherited`**, **`broker_restarted`**, **`session_reused`**, **`handoff_brief`**, **`completion_asserted`** — task state transitions and broker management; use the `tasks` table state column instead

---

## Findings: What Is Causing Wrong Rows Today

### 1. **Missing step_index Ordering**

Tool events and step updates have `step_index` in `payload.step_update.step_index`. If rows are not ordered by this field (and `conversation_id` for tie-break), the activity log shows steps out of sequence. A common mistake: sorting only by `created_at`, which may be non-monotonic for rapid events.

**Fix:** `ORDER BY conversation_id, step_index`.

### 2. **Tool Output Not Extracted When State is DONE**

A tool's result is only present when `step_update.state = "DONE"`. The `tool_info.output` field is omitted for `state: "ACTIVE"`. Rows for ACTIVE tool events show `[running]` or similar, while DONE rows show the actual output (file size, command result, etc.).

**Fix:** Check `$.step_update.state`. For ACTIVE, render as `[in progress]`. For DONE, render the output or a summary of it.

### 3. **Tool Parameter Paths Vary by Tool**

A generic extraction path like `$.step_update.tool_info.parameters.AbsolutePath` fails for tools that use `CommandLine`, `TargetFile`, `Query`, etc. No single JSON path works across all tools.

**Fix:** Dispatch by tool_name first. Each tool has its own target extraction:
```
if tool_name == "view_file": target = parameters.AbsolutePath
if tool_name == "run_command": target = parameters.CommandLine
if tool_name == "write_to_file": target = parameters.TargetFile
[etc.]
```

### 4. **agent.event Treated as One Flat Type**

The current renderer may iterate on `event_type = 'agent.event'` and expect one structure. Instead, three completely different sub-kinds exist: `step_update`, `result`, `init`. If the renderer tries to extract `step_update.step_type` from an `init` event, it gets null, and the row is empty or crashes.

**Fix:** Discriminate on `payload.event` (the string value) as the first step.

### 5. **result Sub-kind Truncated or Confused with agent.result Event Type**

The `agent.event` sub-kind `result` (50 rows) and the top-level `agent.result` event type (2 rows) are two distinct events with different payloads. They may be conflated, causing a row to show the wrong data.

**Fix:** Filter by `event_type = 'agent.event'` AND `json_extract(payload, '$.event') = 'result'` for the sub-kind. Use `event_type = 'agent.result'` for the top-level type (2 rows).

---

## Ambiguities and Inconsistencies

### 1. **agent.result.subtype vs. is_error**

The `agent.result` event type (top-level) has `subtype: "success"` even when `is_error: true`. This is contradictory. Clarify with Antigravity whether `subtype` or `is_error` is the canonical error indicator, or whether both exist for different reasons.

### 2. **agent.user and agent.assistant: Intermediate Protocol Events**

These carry the request/response dialogue. They are not user-visible work; the actual work is captured by `step_update` (tool steps, agent_response) or `result`. A naive iterator may produce duplicate or empty rows. Recommendation: skip these unless you need to reconstruct the full conversation transcript.

### 3. **unknown step_type: Silently Omit or Show?**

25 rows of `step_update.step_type = "unknown"` exist. These are likely internal or provider-specific events with no user-facing meaning. Rendering them as `[Unknown step 42]` confuses users. Options:
- Omit silently.
- Log a warning for debugging, then skip.
- Ask Antigravity what `unknown` means and whether it should appear.

### 4. **error_message and system_message: Missing Content**

Samples show these events have no detail in the payload sample (truncated). Full message text may be elsewhere in the transcript or in a separate field. Verify that the message content is available before rendering.

### 5. **correlation ID Strategy: Conversation vs. Session**

Most Antigravity events use `conversation_id` (UUID, pairs init → step_update → result). System and hook events use `session_id` (larger session scope, one session may contain multiple conversations). Ensure the renderer maintains these two scopes separately; mixing them will orphan rows.

### 6. **Conversation ID Format Inconsistency**

`agent.event` sub-kinds use `payload.conversation_id` or `payload.step_update.conversation_id` (nested). Ensure the path is consistent during extraction.

---

## Absent Shapes: What Antigravity Never Emits

Based on this dataset:

- **No `step_update.step_type` for "message" or "text"** — text responses arrive as `agent_response` with usage stats, not as a separate message step.
- **No nested conversation starts** — conversations are flat; no sub-conversations or forking observed.
- **No explicit "tool_error" step type** — tool errors appear as `error_message` step, not a tool-specific error state.
- **No rate_limit_event with status other than "allowed_warning"** — may exist in other datasets, not observed here. If overage occurs, check whether status escalates to `"limited"` or similar.

---

## Glossary

- **Sub-kind:** Discriminated by `$.event` inside `agent.event` (step_update, result, init).
- **Correlation ID:** Field pairing related events (conversation_id for init/step_update/result; step_index for ordering within conversation).
- **Target:** Human-meaningful artifact — file path, command, search query, conversation summary.
- **State:** Lifecycle stage of a tool (ACTIVE, DONE) or system component.

