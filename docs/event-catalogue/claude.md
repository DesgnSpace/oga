# Claude Code Event Catalogue

Field-level breakdown of how Claude Code activity events must be read for rendering task activity rows.

## Summary

**Distinct payload shapes found**: 8 major shapes across user-visible event types.

**Tools appearing**: Bash, Read, Edit, Write, Skill, ToolSearch, ScheduleWakeup, Agent, TaskUpdate, TaskCreate, mcp__oga__query, WebSearch, WebFetch, Monitor, SendMessage, ReportFindings, ListAgents, and others (41 unique tools identified).

**Hook vs assistant message as row source**: **Use agent.hook PostToolUse/PreToolUse as the authoritative tool-call source**, not agent.assistant. Reasoning:
- Both carry the same tool_use_id and represent the same call.
- PreToolUse captures the input before execution.
- PostToolUse captures the result with duration_ms in the same event.
- agent.assistant tool calls split into assistant message + separate tool_result events, requiring join logic.
- Hooks are simpler: one PreToolUse, one PostToolUse or PostToolUseFailure.
- 36,701 PreToolUse + 35,716 PostToolUse (~72k) vs 35,960 assistant with tool_use (~36k), confirming 2:1 hook coverage.

**Top findings causing wrong rows today**:
1. **agent.system noise** — 127,323 rows are all internal (thinking_tokens=112k, task_progress=3.4k, etc.). None should produce activity rows; filter `event_type = 'agent.system'` entirely from the renderer.
2. **Agent vs hook duplication** — agent.assistant tool calls are already captured by agent.hook PreToolUse/PostToolUse. Using both sources double-renders rows; choose hooks only.
3. **Tool result location** — agent.user events are tool_result wrappers with no actionable content; use PostToolUse.tool_response instead.

---

## Event Types: Detail

### agent.hook (PreToolUse / PostToolUse / PostToolUseFailure)

**Volume**: 73,383 total (36,701 PreToolUse + 35,716 PostToolUse + 690 PostToolUseFailure + 105 SubagentStart + 93 SubagentStop + 121 StopFailure)

**Use for**: Authoritative source for all tool invocations and results.

#### PreToolUse

Fired immediately before a tool is called.

```json
{
  "hook_event_name": "PreToolUse",
  "tool_use_id": "toolu_016UUxrGdAhHA4fjnUYNCpjF",
  "tool_name": "Bash",
  "tool_input": { "command": "grep -rl ...", "description": "..." },
  "session_id": "79a2afee-...",
  "cwd": "/Users/malico/desgn/oga",
  "effort": { "level": "low" },
  "permission_mode": "acceptEdits",
  "prompt_id": "b48eb9cc-...",
  "transcript_path": "..."
}
```

**Row fields**:
- **Tool name**: `$.tool_name` (string: "Bash", "Read", "Edit", "Write", "Skill", etc.)
- **Tool input detail**: Depends on tool type (see tool-specific sections below).
- **Correlation ID**: `$.tool_use_id`
- **Start timestamp**: Use task_events.created_at.
- **Duration**: Obtained from the matching PostToolUse.

#### PostToolUse

Fired after a tool completes successfully.

```json
{
  "hook_event_name": "PostToolUse",
  "tool_use_id": "toolu_016UUxrGdAhHA4fjnUYNCpjF",
  "tool_name": "Bash",
  "tool_input": { "command": "...", "description": "..." },
  "tool_response": { "stdout": "...", "stderr": "", "returnCodeInterpretation": "..." },
  "duration_ms": 1308,
  "session_id": "79a2afee-...",
  ...
}
```

**Row fields**:
- **Tool name**: `$.tool_name`
- **Result detail**: `$.tool_response` — structure varies by tool type.
- **Duration**: `$.duration_ms` (milliseconds)
- **Correlation ID**: `$.tool_use_id`
- **Timestamp**: task_events.created_at

#### PostToolUseFailure

Fired when a tool call fails (permission, syntax, runtime error).

```json
{
  "hook_event_name": "PostToolUseFailure",
  "tool_use_id": "toolu_...",
  "tool_name": "Bash",
  "tool_input": { "command": "..." },
  "error": "permission_denied",
  "error_detail": "Path outside scope: /private/etc",
  "duration_ms": 45,
  ...
}
```

**Row fields**:
- **Tool name**: `$.tool_name`
- **Error**: `$.error` (enum: permission_denied, syntax_error, runtime_error, etc.)
- **Error detail**: `$.error_detail` (human-readable reason)
- **Duration**: `$.duration_ms`

#### SubagentStart / SubagentStop

Agent spawns subagents. Carries the subagent description, type, and outcome.

```json
{
  "hook_event_name": "SubagentStart",
  "agent_id": "a1e5346465e64c5fe",
  "agent_type": "general-purpose",
  "cwd": "/Users/malico/desgn/oga",
  ...
}
```

**Row**: "Spawned subagent" | subagent type + description from the Agent tool_input.

---

### agent.assistant

**Volume**: 60,345 rows

**Use for**: Text output from the LLM (when not an error).

**Note**: Tool calls in this event are DUPLICATED by agent.hook PreToolUse/PostToolUse. Do not use this as the tool-call source.

```json
{
  "type": "assistant",
  "message": {
    "id": "msg_...",
    "model": "claude-sonnet-5",
    "role": "assistant",
    "content": [
      { "type": "text", "text": "Here is my analysis..." },
      { "type": "tool_use", "id": "toolu_...", "name": "Read", "input": {...} }
    ],
    "stop_reason": "tool_use",
    "usage": {...}
  },
  "session_id": "...",
  "timestamp": "2026-08-31T22:27:58.732Z",
  "error": null
}
```

**Row fields** (text only):
- **Text content**: `$.message.content[]` filtered by `type === "text"` → `$.text`
- **Stop reason**: `$.message.stop_reason` (enum: "tool_use", "end_turn", "max_tokens")
- **Timestamp**: `$.timestamp`

**Tool calls in message.content**: Ignore for activity rows; use agent.hook instead.

**Error variants** (do produce rows):
```json
{
  "error": "server_error",
  "is_api_error_message": true,
  "message": { "content": [{ "type": "text", "text": "API Error: No response from API" }] }
}
```
→ **Row**: "API error" | error message text

```json
{
  "error": "rate_limit",
  "message": { "content": [{ "type": "text", "text": "You've hit your session limit · resets 12:40am ..." }] }
}
```
→ **Row**: "Rate limit" | the message text

---

### agent.tool_progress

**Volume**: 1,779 rows

**Use for**: Heartbeat / elapsed time indicators for in-progress tool calls. Shows the tool is still running.

```json
{
  "type": "tool_progress",
  "tool_use_id": "toolu_013S7fGLRp2VE244c1fc6jLb-heartbeat-0",
  "tool_name": "Bash",
  "parent_tool_use_id": "toolu_013S7fGLRp2VE244c1fc6jLb",
  "elapsed_time_seconds": 30,
  "heartbeat": true,
  "session_id": "79a2afee-..."
}
```

**Row fields**:
- **Tool name**: `$.tool_name`
- **Elapsed time**: `$.elapsed_time_seconds`
- **Heartbeat flag**: `$.heartbeat` (boolean; when true, this is a progress update, not a completion)
- **Correlation ID**: `$.parent_tool_use_id` (if heartbeat) or `$.tool_use_id`
- **Timestamp**: task_events.created_at

**Row format**: "Bash · 30s elapsed" (if heartbeat, append "…")

---

### agent.error

**Volume**: 21 rows

**Use for**: Unexpected errors in the agent/Claude Code system layer.

```json
{
  "type": "error",
  "error": { "type": "provider.invalid-request", "message": "Provider request failed with HTTP 404", "status": 404 },
  "session_id": "...",
  "timestamp": "..."
}
```

**Row fields**:
- **Error type**: `$.error.type`
- **Error message**: `$.error.message`
- **Status** (if HTTP): `$.error.status`

**Row**: "Error" | error message

---

## Tool-Specific Input/Output Shapes

### Bash

**Input**:
```json
{
  "command": "grep -rl 'pattern' src/ 2>/dev/null",
  "description": "Find files with pattern"
}
```

**Output (success)**:
```json
{
  "stdout": "src/file1.ts\nsrc/file2.ts",
  "stderr": "",
  "interrupted": false,
  "returnCodeInterpretation": "Success",
  "noOutputExpected": false
}
```

**Output (failure)**:
```json
{
  "stdout": "",
  "stderr": "Permission denied",
  "interrupted": false,
  "returnCodeInterpretation": "Permission denied"
}
```

**Row**: `[command-name-or-description]` | stdout (first 100 chars) or error

---

### Read

**Input**:
```json
{
  "file_path": "/Users/malico/desgn/oga/src/main.ts",
  "offset": 10,
  "limit": 50,
  "pages": "1-5"
}
```

**Output (success)**:
```json
{
  "type": "text",
  "file": {
    "filePath": "/Users/malico/desgn/oga/src/main.ts",
    "content": "...",
    "numLines": 500,
    "startLine": 1,
    "totalLines": 500
  }
}
```

**Row**: `Read` | file_path + line range (if offset/limit given)

---

### Edit

**Input**:
```json
{
  "file_path": "/Users/malico/desgn/oga/src/main.ts",
  "old_string": "const x = 1;",
  "new_string": "const x = 2;",
  "replace_all": false
}
```

**Output**:
```json
{
  "filePath": "/Users/malico/desgn/oga/src/main.ts",
  "oldString": "const x = 1;",
  "newString": "const x = 2;",
  "originalFile": "...",
  "newFile": "..."
}
```

**Row**: `Edit` | file_path + old/new snippets (first 50 chars each)

---

### Write

**Input**:
```json
{
  "file_path": "/Users/malico/desgn/oga/new-file.ts",
  "content": "export const x = 1;"
}
```

**Output**:
```json
{
  "filePath": "/Users/malico/desgn/oga/new-file.ts",
  "newFile": "export const x = 1;"
}
```

**Row**: `Write` | file_path

---

### Agent

**Input**:
```json
{
  "description": "Audit parity between Rust and React UIs",
  "prompt": "Compare the two implementations...",
  "subagent_type": "general-purpose",
  "run_in_background": false,
  "model": "claude-sonnet-5"
}
```

**Output**:
```json
{
  "agent_id": "a1e5346465e64c5fe",
  "subagent_type": "general-purpose"
}
```

**Row**: `Spawn agent` | description (first 60 chars)

---

### Skill

**Input**:
```json
{
  "skill": "refactor",
  "args": "clean"
}
```

**Output**:
```json
{
  "success": true,
  "skill": "refactor"
}
```

**Row**: `Skill` | skill name + args

---

### TaskCreate

**Input**:
```json
{
  "subject": "Refactor export",
  "description": "Share logic via new exported function"
}
```

**Output**:
```json
{
  "task": {
    "id": "abc123xyz",
    "subject": "Refactor export"
  }
}
```

**Row**: `Create task` | subject (first 60 chars)

---

### TaskUpdate

**Input**:
```json
{
  "taskId": "abc123xyz",
  "status": "in_progress"
}
```

**Output**:
```json
{
  "success": true,
  "taskId": "abc123xyz",
  "updatedFields": ["status"],
  "statusChange": { "from": "pending", "to": "in_progress" }
}
```

**Row**: `Update task` | taskId + statusChange (from→to)

---

### ToolSearch

**Input**:
```json
{
  "query": "select:mcp__oga__query",
  "max_results": 3
}
```

**Output**:
```json
{
  "matches": ["mcp__oga__query"],
  "query": "select:mcp__oga__query",
  "total_deferred_tools": 35
}
```

**Row**: `ToolSearch` | query string (first 50 chars)

---

### WebSearch / WebFetch / Monitor / Others

Similar structure to tools above. Extract:
- Tool name from `$.tool_name`
- Input from `$.tool_input`
- Result summary from `$.tool_response` (structure varies)
- Correlation ID from `$.tool_use_id`

---

## Events That Should NOT Produce Rows

### agent.system (127,323 rows)

**All subtypes are internal/noise. Do not render any as activity rows.**

Breakdown:
- `thinking_tokens` (112,198) — Extended thinking metadata, not user-visible action.
- `task_notification` (3,403) — Internal task delegation metadata.
- `task_started` (3,386) — Internal.
- `task_progress` (2,183) — Internal.
- `commands_changed` (1,666) — Internal capability refresh.
- `init` (1,080) — Session initialization.
- `hook_started` / `hook_response` (737 each) — Hook execution metadata.
- `api_retry` (432) — Retry logic metadata.
- `background_tasks_changed` (418) — Internal state.

**Filter out entirely**: `WHERE event_type != 'agent.system'`

### agent.user (36,161 rows)

Tool results arriving back from the LLM API. These are captured in `agent.hook PostToolUse.tool_response` already. Do not render separately.

Structure:
```json
{
  "type": "user",
  "message": {
    "role": "user",
    "content": [
      { "type": "tool_result", "tool_use_id": "toolu_...", "content": [...] }
    ]
  }
}
```

---

### agent.tool_use, agent.step_start / agent.step_finish (legacy)

These come from older OpenAI provider sessions (sessionID/callID format). Not part of current Claude Code event stream. Skip them.

---

### agent.text (91 rows, legacy)

Old OpenAI format with sessionID/messageID. Not in current Claude Code. Skip.

---

## Ambiguities & Gotchas

1. **Tool result duplication**: A single tool execution produces:
   - `agent.hook PreToolUse` (call + inputs)
   - `agent.hook PostToolUse` (call + inputs + result + duration)
   - `agent.assistant` text output (if the response includes text)
   - `agent.user` tool_result (the result feeding back into the LLM)
   
   **Fix**: Render from `agent.hook PreToolUse + PostToolUse` only; ignore agent.assistant tool calls and agent.user entirely.

2. **Heartbeat tool_use_id mangling**: Progress events for a Bash command have:
   - Original call: `toolu_013S7fGLRp2VE244c1fc6jLb`
   - Progress: `toolu_013S7fGLRp2VE244c1fc6jLb-heartbeat-0`, `-heartbeat-1`, etc.
   
   **Fix**: Match on `$.parent_tool_use_id` for progress events, or strip the `-heartbeat-N` suffix.

3. **Error messages as assistant content**: When the API returns an error (rate limit, server error), it arrives as an `agent.assistant` event with `error` field set, and the message.content contains the error text. This IS renderable as a row, even though it's technically an error, not a response.

4. **Timestamp vs created_at**: Payloads carry `$.timestamp` (ISO string), but also trust `task_events.created_at` as authoritative. They should match; if they diverge, investigate.

5. **MCP tool names**: Tools from MCP servers (e.g., `mcp__oga__query`, `mcp__oga__decide`) appear in the tool_name field as-is. Display user-friendly names (e.g., "Query task DB" for `mcp__oga__query`). A static name map is needed in the renderer.

---

## Recommended Renderer Logic

1. **Filter**:
   ```sql
   WHERE event_type IN ('agent.hook', 'agent.assistant', 'agent.tool_progress', 'agent.error')
   AND NOT (event_type = 'agent.hook' AND hook_event_name IN ('SubagentStart', 'SubagentStop', 'StopFailure'))
   ```

2. **Sort**: Order by `task_events.created_at ASC`.

3. **Group** (optional): Group consecutive tool calls and results by session_id + window of 1-2 seconds, if the UI wants "call + result" as a single compound row.

4. **Render**:
   - `agent.hook.PreToolUse`: "→ Tool: [name]" + input summary (show immediately if no PostToolUse follows in <2s)
   - `agent.hook.PostToolUse`: "[Tool name]" | result summary + duration
   - `agent.hook.PostToolUseFailure`: "[Tool name] — Error" | error detail
   - `agent.assistant` (text only): "[LLM]" | text content
   - `agent.assistant` (error): "[Error]" | error message
   - `agent.tool_progress`: "[Tool name] · Xs elapsed…" (no final result until PostToolUse arrives)
   - `agent.error`: "[System Error]" | error message

---

## Payload Size Note

Payloads can exceed 10KB (full file contents in Read/Edit outputs, long command output in Bash). Slice to 2000–5000 chars in UI summaries; link to full transcript for details.
