# Codex Activity Event Catalogue

Field-level extraction paths for rendering each Codex activity event as a correct row: a verb, a target, and a short detail.

## Scope

This catalogue covers Codex event types that appear in `~/.oga/oga.db`, table `task_events`, filtered by `profile_id = 'codex'`. Event payloads are stored in the `payload` column as JSON.

Rows in the activity feed should render one meaningful line per event, where:
- **Verb** describes the action (e.g., "ran", "changed", "sent")
- **Target** names what was acted upon (e.g., a command, file path, or message snippet)
- **Detail** provides extra context the row needs to be understood at a glance

---

## Agent Item Events (7,890 rows total; primary rendering surface)

Codex item events follow a pattern: started → [updated*] → completed, identified by item ID.

### Structure

```json
{
  "type": "item.started|item.updated|item.completed",
  "item": {
    "id": "<item_id>",
    "type": "<item_type>",
    "status": "in_progress|completed|failed",
    // item-type-specific fields follow
  }
}
```

**Pairing started/completed rows:**
- Join on `payload → item → id`
- The `started` event holds the initial target and command/query
- The `completed` event holds the result (output, status, exit code, error message)
- The `updated` event (todo_list only) tracks partial progress
- **Decision:** Render the triple as one logical row per item ID, not three rows

**Which event carries what:**
- **Title/verb:** Item type (command_execution → "ran", file_change → "changed", etc.)
- **Target:** Path(s), command, or query (from started event; may be updated in completed)
- **Status:** From completed event (exit_code, completion status, error message)
- **Detail:** Implementation-specific (see item types below)

---

## Item Types (Distinct Payloads)

### 1. Command Execution (1,092 started; 1,081 completed)

**Use:** Bash/shell commands executed by the agent.

**Extraction paths:**

| Field | JSON Path | Example |
|-------|-----------|---------|
| Item ID (pairing key) | `$.item.id` | `"item_2"` |
| Command text | `$.item.command` | `"/bin/zsh -lc \"cd /path && npm test\""` |
| Status | `$.item.status` | `"in_progress"` (started), `"failed"` (completed) |
| Exit code | `$.item.exit_code` | `0` or `1` (null in started event) |
| Output | `$.item.aggregated_output` | multiline string; truncate to ~200 chars |

**Row format:**
- **Verb:** `"ran"`
- **Target:** Command, truncated to reasonable width (show core command, not full flags)
- **Detail:** Exit code + first line of error output if non-zero; success if exit code 0

**Example rows:**
- `ran: npm test` → ✓
- `ran: sed -n '1,240p' SKILL.md` → exit 1: No such file

**Notes:**
- Exit code is definitive for success/failure
- Aggregated output may be empty for simple commands
- Long commands should be summarized for readability (e.g., show the main command, omit repetitive flags)

---

### 2. File Change (204 started; 204 completed)

**Use:** File edits, writes, deletes initiated by the agent.

**Extraction paths:**

| Field | JSON Path | Example |
|-------|-----------|---------|
| Item ID (pairing key) | `$.item.id` | `"item_28"` |
| Changes array | `$.item.changes` | `[{"path": "...", "kind": "update"}, ...]` |
| Status | `$.item.status` | `"in_progress"`, `"completed"` |

**Row format:**
- **Verb:** `"changed"` or `"updated"` or specific verb per kind (create/delete/update)
- **Target:** File paths (show count if multiple, e.g., "2 files")
- **Detail:** Kind of change (update/create/delete), or first changed file if only one

**Example rows:**
- `changed: /app/routes.ts, /app/middleware.ts` → 2 files updated
- `changed: /component.tsx` → created

**Notes:**
- `changes` array lists all modified files in one item
- `kind` indicates create, update, or delete
- Status is always "completed" when event is present (no intermediate updates for file changes)

---

### 3. Web Search (36 started; 36 completed)

**Use:** Web search queries executed by the agent.

**Extraction paths:**

| Field | JSON Path | Example |
|-------|-----------|---------|
| Item ID (pairing key) | `$.item.id` | `"exec-0fbf8e8a-bedf-4666-80bd-d0eaef37de4a"` |
| Query (draft) | `$.item.query` | `""` (empty in started) |
| Query (final) | `$.item.action.queries` | Array of actual search query strings |
| Status | `$.item.status` | `"in_progress"`, `"completed"` |

**Row format:**
- **Verb:** `"searched"` or `"searched for"`
- **Target:** First query string (or original `query` field if populated), truncated
- **Detail:** Count of generated queries (if > 1), or result summary

**Example rows:**
- `searched: site:docs.anthropic.com claude-code hooks` → 4 queries
- `searched for: laravel async queue processing`

**Notes:**
- `query` field may be empty in started; the completed event has the final multi-query plan in `action.queries`
- Queries are intentionally multiple (searching multiple sources); show count as part of detail
- The initial `query` and the expanded `action.queries` may differ substantially

---

### 4. MCP Tool Call (15 started; 15 completed)

**Use:** Calls to Model Context Protocol tools (e.g., reads, edits, code analysis).

**Extraction paths:**

| Field | JSON Path | Example |
|-------|-----------|---------|
| Item ID (pairing key) | `$.item.id` | `"item_11"` |
| Server | `$.item.server` | `"laravel-boost"` |
| Tool name | `$.item.tool` | `"application-info"` |
| Arguments | `$.item.arguments` | `{}` or key-value pairs |
| Result | `$.item.result` | Object with `content` array (started: null) |
| Status | `$.item.status` | `"in_progress"`, `"completed"` |

**Row format:**
- **Verb:** `"called"` or `"invoked"`
- **Target:** `<server>/<tool>` (e.g., "laravel-boost/application-info")
- **Detail:** Result type (e.g., "returned JSON: 6 packages") or error if present

**Example rows:**
- `called: laravel-boost/application-info` → returned JSON: 6 packages, 8 versions
- `called: codex/query-code` → returned 3 matches

**Notes:**
- Server name is meaningful; include it in the target
- Result content is nested in `$.item.result.content[0].text` (may be JSON, markdown, or plain text)
- No arguments shown in the row unless they're the only distinguishing detail

---

### 5. Todo List (4 started; 3 completed; 5 updated)

**Use:** Checkpoints or todo items managed by the agent during a run.

**Extraction paths:**

| Field | JSON Path | Example |
|-------|-----------|---------|
| Item ID (pairing key) | `$.item.id` | `"item_5"` |
| Items array | `$.item.items` | `[{"text": "...", "completed": true/false}, ...]` |
| Status | `$.item.status` | `"in_progress"`, `"completed"` |

**Row format:**
- **Verb:** `"planned"` (started), `"checked"` (updated), or "completed" (final)
- **Target:** Count of items (e.g., "4 steps")
- **Detail:** Next incomplete item text, or "all done" if all true

**Example rows:**
- `planned: 4 steps` → Create semantic Dispatch Board HTML and CSS
- `checked: 4 steps` → Write phase 1 report (2/4 done)
- `completed: 4 steps` → all done

**Notes:**
- Track progress across started → updated* → completed
- Show the first incomplete item as a teaser
- The `completed` field per item is the progress indicator
- Updates may occur between started and completed; render the final state

---

### 6. Agent Message (886 completed; no started)

**Use:** Direct messages or announcements from the agent to the user.

**Extraction paths:**

| Field | JSON Path | Example |
|-------|-----------|---------|
| Item ID (pairing key) | `$.item.id` | `"item_1"` |
| Text | `$.item.text` | `"Loading Laravel, Pest, and refactor guidance"` |
| Status | `$.item.status` | `"completed"` |

**Row format:**
- **Verb:** `"said"` or `"reported"`
- **Target:** Full message text (or first 60 chars if long)
- **Detail:** Empty (message is the detail)

**Example rows:**
- `said: Loading Laravel, Pest, and refactor guidance`
- `said: Found 3 matching patterns. Analyzing...`

**Notes:**
- Agent messages have no started event; they appear only as completed
- These are prose milestones, not work items
- Render as-is; no extraction needed beyond the text field
- If text is very long (>100 chars), show first part + "…"

---

### 7. Error (62 completed; no started)

**Use:** Errors encountered during the agent's work (file not found, permission denied, etc.).

**Extraction paths:**

| Field | JSON Path | Example |
|-------|-----------|---------|
| Item ID (pairing key) | `$.item.id` | `"item_0"` |
| Error message | `$.item.message` | `"Failed to read global AGENTS.md: Operation not permitted"` |
| Status | `$.item.status` | `"completed"` |

**Row format:**
- **Verb:** `"failed"` or `"error"`
- **Target:** Error type (extract class/kind from message, or first phrase)
- **Detail:** Short error message (first line, ~80 chars)

**Example rows:**
- `failed: Permission denied` → operation not permitted on /path/file
- `failed: File not found` → /Users/malico/.codex/AGENTS.md

**Notes:**
- Error items have no started event; they appear only as completed
- Messages vary in structure; extract the most useful phrase
- These are leaf failures, not upstream errors (see agent.turn.failed for turn-level errors)

---

## Lifecycle Events (Do Not Render as Rows)

The following event types are protocol metadata or system events. **Do not produce activity rows for these.**

### Rationale

| Event Type | Reason | Volume |
|------------|--------|--------|
| `heartbeat` | Lifecycle keepalive; no meaningful action | 2,679 |
| `agent.system` subtypes: `thinking_tokens`, `init`, `hook_started`, `hook_response` | System events and budgeting metadata | 231 |
| `agent.tool_use` | Raw low-level tool invocations; use agent.item instead | 109 |
| `agent.turn.started` | Turn lifecycle marker; no action | 95 |
| `agent.turn.completed` | Token usage report; not user-visible work | 67 |
| `agent.turn.failed` | Turn-level error (API limits, auth); see Errors below | 11 |
| `agent.thread.started` | Session lifecycle; no user-visible action | 95 |
| `agent.step_start` / `agent.step_finish` | Raw Claude protocol; no action mapped | 81 |
| `agent.assistant` / `agent.user` | Raw message protocol; use agent.item | 54 + 33 |
| `agent.text` | Inline narration during turns; use agent.item for meaningful work | 20 |
| `agent.hook` | Hook call/response metadata | 68 |
| `agent.result` | Session result summary; not per-event rendering | 2 |
| `agent.rate_limit_event` | System event; use error items for user-visible failures | 10 |
| `created`, `started`, `completed`, `archived` | Task lifecycle; belongs on the task card, not the feed | Various |
| `worker_spawned`, `session_captured`, `session_reused` | System events | Various |

**Summary:** Render only agent.item.* events. All other Codex events are metadata, housekeeping, or protocol-level detail that does not constitute user-visible work.

---

## Turn-Level Errors (Special Case)

**Event type:** `agent.turn.failed`

**Structure:**
```json
{
  "type": "turn.failed",
  "error": {
    "message": "Your workspace is out of credits. Ask your workspace owner to refill in order to continue."
  }
}
```

**Rendering:**
- **Should appear in feed:** Only if the turn failure directly halted the task
- **Row format:** `error: <message>` (verb = error, target = first phrase, detail = full message)
- **Pairing:** These are not item-based; they float as standalone events if shown

**Note:** Turn failures are distinct from item errors (command exit 1, file not writable). Consider whether turn-level failures should be rendered or absorbed into a "task halted" status elsewhere.

---

## Event Pairing & Sequencing

### Started/Completed Triples (agent.item.*)

For every item type except `agent_message` and `error`:

1. **Identify the logical unit:** All events with the same `$.item.id`
2. **Extract initial state:** From the `started` event
   - Command text, file paths, search query (draft)
3. **Extract final state:** From the `completed` event
   - Exit code, output, result, status
4. **Include intermediate updates:** `updated` events (todo_list only)
   - Show progress from started → completed
5. **Render as one row** with verb, target, detail drawn from both

### Sequencing

Within a single task run, items are rendered in the order their **started** events arrived (by `created_at`). If an `updated` event arrives before the `completed` event, track it but don't render until completed or marked final.

---

## Ambiguities & Traps

### 1. Item Type Collision With Message Types

In Codex, "item" is a work unit (command, file, search, etc.). In other providers, "item" may mean something different. **Always inspect `$.item.type`** before applying extraction rules.

### 2. Query Evolution (Web Search)

The `started` event for web_search has an empty or partial `query` field. The `completed` event has the full set of queries in `action.queries`. **Use the completed payload for the final search strategy**, not the started query.

### 3. File Changes Have No Updates

Unlike todo_list, file_change items do not produce `updated` events. The change is atomic: started → completed. Status will always be "in_progress" in started and "completed" in completed.

### 4. Agent Messages & Errors Are Singletons

`agent_message` and `error` items appear **only** as completed events. There is no started event. Do not wait for a completed to match a started; render these as standalone announcements.

### 5. Exit Code vs Status Field

Command execution items have both `exit_code` (integer or null) and `status` (string: "in_progress", "completed", "failed"). 
- `exit_code: null` → still running (started event only)
- `exit_code: 0` → success
- `exit_code: <nonzero>` → failure
- `status` is a convenience; always trust `exit_code` for success/failure

### 6. MCP Tool Result Structure

Tool results are nested: `$.item.result.content[0].text`. The content may be JSON, YAML, markdown, or plain text. **Do not assume structure**; pass through as opaque text or parse carefully.

### 7. Lifecycle Events May Appear Between Items

System events (thinking_tokens, hook calls) and turn markers (turn.started, turn.completed) may arrive between item started and item completed. **Do not let them interrupt or duplicate the item row.** Filter them out during feed construction.

---

## Payload Size Constraints

Codex payloads can be large (the MCP tool result for "application-info" is 2KB+ of JSON). When querying the database:
- Always slice: `substr(payload, 1, 2000)` to avoid memory issues
- Never `SELECT payload` bare
- For rendering, truncate large outputs (commands, messages) to ~200 chars

---

## Delivery Status (as of 2026-08-31)

| Event Type | Rows | Item Types | Started/Updated/Completed | Next |
|------------|------|------------|---------------------------|------|
| `agent.item.*` | 3,648 | command_execution, file_change, web_search, mcp_tool_call, todo_list, agent_message, error | ✓ triple logic | Implement feed grouping |
| `agent.tool_use` | 109 | 8 varieties | N/A (protocol, not rendered) | — |
| `agent.turn.*` | 173 | — | ✓ (errors only) | Handle turn.failed rendering |
| `agent.thread.started` | 95 | — | — | Filter out |
| `agent.system` | 231 | thinking_tokens, init, hook_* | — | Filter out |
| `agent.step_*` | 162 | — | — | Filter out |
| `agent.*` (text, assistant, user, hook, result) | 109 | — | — | Filter out |
| Other (heartbeat, lifecycle, etc.) | 3,864 | — | — | Filter out |
| **Total** | **7,890** | **7 rendered** | — | — |

---

## Implementation Checklist

- [ ] Parse `event_type` and dispatch to handler per type
- [ ] For `agent.item.*`: extract item type from `$.item.type`
- [ ] Implement started/completed pairing by item ID
- [ ] Generate row verb, target, detail per item type
- [ ] Handle updated events (todo_list) in the sequence
- [ ] Render agent_message and error as standalone rows
- [ ] Truncate long outputs and text (200 char ceiling)
- [ ] Filter out lifecycle events (turn, system, step, tool_use, etc.)
- [ ] Handle edge cases: null exit codes, empty queries, error message parsing
- [ ] Test on a full task run (100+ rows) to verify sequencing and pairing

---

## Examples: Complete Row Renderings

### Command Execution
- **Started:** `{"type":"item.started","item":{"id":"item_2","type":"command_execution","command":"npm test","status":"in_progress"}}`
- **Completed:** `{"type":"item.completed","item":{"id":"item_2","type":"command_execution","command":"npm test","exit_code":0,"status":"completed","aggregated_output":"✓ 42 tests pass"}}`
- **Row:** `ran: npm test` → ✓

### File Change
- **Started:** `{"type":"item.started","item":{"id":"item_28","type":"file_change","changes":[{"path":"/app/routes.ts","kind":"update"},{"path":"/app/middleware.ts","kind":"update"}],"status":"in_progress"}}`
- **Completed:** `{"type":"item.completed","item":{"id":"item_28","type":"file_change","changes":[...],"status":"completed"}}`
- **Row:** `changed: 2 files` → updated

### Web Search
- **Started:** `{"type":"item.started","item":{"id":"exec-abc","type":"web_search","query":"","status":"in_progress"}}`
- **Completed:** `{"type":"item.completed","item":{"id":"exec-abc","type":"web_search","query":"","action":{"type":"search","queries":["site:docs.anthropic.com hooks","site:github.com/openai hooks"]},"status":"completed"}}`
- **Row:** `searched: hooks stream-json` → 2 queries

### Todo List (Progress)
- **Started:** `{"type":"item.started","item":{"id":"item_5","type":"todo_list","items":[{"text":"Step 1","completed":false},{"text":"Step 2","completed":false}],"status":"in_progress"}}`
- **Updated:** `{"type":"item.updated","item":{"id":"item_5","type":"todo_list","items":[{"text":"Step 1","completed":true},{"text":"Step 2","completed":false}],...}}`
- **Completed:** `{"type":"item.completed","item":{"id":"item_5","type":"todo_list","items":[{"text":"Step 1","completed":true},{"text":"Step 2","completed":true}],...}}`
- **Row (progressive):** `planned: 2 steps` → `checked: Step 2` → `completed: all done`

### Agent Message
- **Completed:** `{"type":"item.completed","item":{"id":"item_1","type":"agent_message","text":"Analysis complete. Found 3 patterns.","status":"completed"}}`
- **Row:** `said: Analysis complete. Found 3 patterns.`

### Error
- **Completed:** `{"type":"item.completed","item":{"id":"item_0","type":"error","message":"Failed to read /path: Operation not permitted","status":"completed"}}`
- **Row:** `failed: Permission denied` → operation not permitted on /path
