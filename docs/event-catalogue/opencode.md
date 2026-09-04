# OpenCode Activity Event Catalogue

This catalogue documents the payload structure for every event type and tool that OpenCode generates, focusing on how each should render as a row in the task activity trace: what verb, what target, and what detail to extract.

## Summary

- **52 distinct tools** in `agent.tool_use` events
- **Payload shapes vary significantly per tool** — there is no single extraction pattern
- **Two main event families**: `agent.tool_use` (52K rows, tool invocations) and other agent events (messaging, lifecycle, session info)
- **Status distribution in tool_use**: 51,351 "completed", 790 "error" (51,140 total)

---

## agent.tool_use Events

The primary carrier of tool invocations. Structure:

```
{
  "type": "tool_use",
  "timestamp": <millis>,
  "sessionID": "<id>",
  "part": {
    "type": "tool",
    "tool": "<tool_name>",
    "callID": "<unique_id>",
    "state": {
      "status": "completed" | "error",
      "input": { ... },        // tool arguments (shape varies per tool)
      "output": "...",         // only on success
      "error": "...",          // only on error
      "title": "..."           // human summary already generated
    },
    "time": { "start": <ms>, "end": <ms> }
  }
}
```

### Row Extraction Rules for agent.tool_use

- **Verb**: Extract from `tool` name via `TOOL_VERBS` (e.g., "read" → "Read/Reading", "bash" → "Ran/Running")
- **Target**: Extract from `part.state.input.<field>` (varies by tool, see per-tool docs below)
- **Detail**: Extract from `part.state.input` for most tools; fall back to truncated `output` for tools with unstructured results
- **Correlation ID**: Always use `part.callID` to pair start/complete/result events
- **Status/Phase**: 
  - `"completed"` → phase="completed"
  - `"error"` → phase="failed", detail = `part.state.error`

---

## Tool-Specific Payload Shapes

### File Operations

#### read (17,152 events)

**Row verb**: Read | Reading  
**Correlation**: `$.part.callID`

**Input fields**:
- `$.part.state.input.filePath` — the file to read (target, required)
- `$.part.state.input.offset` (optional) — line range start
- `$.part.state.input.limit` (optional) — line range length

**Detail**: File path + line range if present. Example: `app/src/main.ts (lines 10–50)`

**Output**: Entire file or snippet in `$.part.state.output` — too large for row detail, leave empty in row.

**Note**: Many read errors pull from `$.part.state.output` instead of `input`, showing the returned error. Extract target from `input.filePath` regardless of status.

#### write (881 events)

**Row verb**: Wrote / Writing  
**Correlation**: `$.part.callID`

**Input fields**:
- `$.part.state.input.filePath` — target file (required)
- `$.part.state.input.content` — file content (very long, not useful in row)

**Detail**: File path. Example: `src/config.ts`

#### edit (4,864 events)

**Row verb**: Edited / Editing  
**Correlation**: `$.part.callID`

**Input fields**:
- `$.part.state.input.filePath` — target file (required)
- `$.part.state.input.oldString` — string to replace (very long)
- `$.part.state.input.newString` — replacement text (very long)

**Detail**: File path only. Example: `web/src/components/Button.tsx`

**Note**: Not useful to show oldString/newString in row (both too long); just name the file.

#### apply_patch (3,364 events)

**Row verb**: Applied / Applying  
**Correlation**: `$.part.callID`

**Input fields**:
- `$.part.state.input.filePath` — target file (required)
- `$.part.state.input.patch` — patch text (large, not useful)

**Detail**: File path.

### Search Operations

#### bash (17,661 events)

**Row verb**: Ran / Running  
**Correlation**: `$.part.callID`

**Input fields**:
- `$.part.state.input.command` — the shell command (required)

**Detail**: Command. For long commands, truncate to ~100 chars. Example: `grep -rn "auth" src --include="*.ts"`

**Output**: Command output in `$.part.state.output` (can be large; show first line or few lines in row if meaningful, or leave empty).

**Note**: Bash exit code is NOT in payload; extraction cannot determine success/failure beyond the status field.

#### grep (4,882 events)

**Row verb**: Searched / Searching  
**Correlation**: `$.part.callID`

**Input fields**:
- `$.part.state.input.pattern` — regex or string to search (required)
- `$.part.state.input.path` — directory to search (required)
- `$.part.state.input.include` (optional) — file glob filter (e.g., `*.ts`)

**Detail**: Pattern + path + include filter. Example: `"auth" in src --include="*.ts"`

**Output**: Match lines and context in `$.part.state.output`. Row detail: show match count if output starts with `Found N matches`.

#### glob (1,157 events)

**Row verb**: Found / Finding  
**Correlation**: `$.part.callID`

**Input fields**:
- `$.part.state.input.pattern` — glob pattern (required)
- `$.part.state.input.cwd` (optional) — working directory (required for interpreting pattern)

**Detail**: Pattern + optional cwd. Example: `src/**/*.ts` or `src/**/*.ts (cwd: app)`

**Output**: File list in `$.part.state.output`; row can show `Found N files`.

### MCP Tools (Oga)

These tools are bridged from the oga MCP server; they use a standardized structure different from user-facing tools.

#### oga_query (170 events)

**Row verb**: Searched / Searching  
**Correlation**: `$.part.callID`

**Input fields**:
- `$.part.state.input.q` — search query (required)
- `$.part.state.input.cwd` (optional) — project working directory

**Detail**: Query string. Example: `"auth middleware"`

**Output**: Ranked matches in `$.part.state.output`; row can show confidence or top match.

#### oga_inspect (90 events)

**Row verb**: Inspected / Inspecting  
**Correlation**: `$.part.callID`

**Input fields**:
- `$.part.state.input.taskId` — task ID to inspect (required)

**Detail**: Task ID.

**Output**: Task summary in `$.part.state.output`.

#### oga_tasks (88 events)

**Row verb**: Queried / Querying  
**Correlation**: `$.part.callID`

**Input fields**: Usually empty or minimal filter fields.

**Detail**: "Task list" or filter criteria if present.

**Output**: Task list in `$.part.state.output`.

#### oga_memory (53 events)

**Row verb**: Loaded / Loading  
**Correlation**: `$.part.callID`

**Input fields**:
- `$.part.state.input.action` — "list" | "get" | "set" | etc.
- `$.part.state.input.key` — memory key (optional)

**Detail**: Action + key. Example: `set worktree-build` or `list`.

**Output**: Memory contents or list in `$.part.state.output`.

#### oga-database-local_query (145 events)

**Row verb**: Queried / Querying  
**Correlation**: `$.part.callID`

**Input fields**:
- `$.part.state.input.sql` — SQL query (required)

**Detail**: SQL query (truncated). Example: `SELECT * FROM tasks WHERE status='running'`

**Output**: Query result in `$.part.state.output` (often large JSON; show row count if available).

#### oga-database-local_describe_table (15 events)

**Row verb**: Inspected / Inspecting  
**Correlation**: `$.part.callID`

**Input fields**:
- `$.part.state.input.table` — table name (required)

**Detail**: Table name.

#### oga-database-local_list_tables (7 events)

**Row verb**: Queried / Querying  
**Correlation**: `$.part.callID`

**Input fields**: None.

**Detail**: "Database tables".

### GitHub CLI Tools

Bridged from `github-cli-local` MCP server.

#### github-cli-local_create_pull_request (78 events)

**Row verb**: Opened / Opening  
**Correlation**: `$.part.callID`

**Input fields**:
- `$.part.state.input.title` — PR title (required)
- `$.part.state.input.head` — head branch (required)
- `$.part.state.input.base` — base branch (required)

**Detail**: Title + `head → base`. Example: `"fix auth middleware" (task/auth → main)`

**Output**: PR URL in `$.part.state.output`. Example: `https://github.com/org/repo/pull/42`

**Row**: Title + branch info in detail; result field can carry PR URL if status is "completed".

#### github-cli-local_get_pull_request (21 events)

**Row verb**: Fetched / Fetching  
**Correlation**: `$.part.callID`

**Input fields**:
- `$.part.state.input.pr` — PR number or URL (required)

**Detail**: PR number/URL.

**Output**: PR JSON in `$.part.state.output`; row can show title or status if parseable.

#### github-cli-local_commit_status (14 events)

**Row verb**: Checked / Checking  
**Correlation**: `$.part.callID`

**Input fields**:
- `$.part.state.input.commit` — commit sha (required)

**Detail**: Commit SHA (truncated). Example: `a1b2c3d`

**Output**: Status check results in `$.part.state.output`.

#### github-cli-local_pr_files (3 events)

**Row verb**: Listed / Listing  
**Correlation**: `$.part.callID`

**Input fields**:
- `$.part.state.input.pr` — PR number (required)

**Detail**: PR number.

**Output**: File list in `$.part.state.output`.

#### github-cli-local_search_code (21 events)

**Row verb**: Searched / Searching  
**Correlation**: `$.part.callID`

**Input fields**:
- `$.part.state.input.query` — code search query (required)

**Detail**: Query string.

**Output**: Search results in `$.part.state.output`.

#### github-cli-local_get_file (30 events)

**Row verb**: Fetched / Fetching  
**Correlation**: `$.part.callID`

**Input fields**:
- `$.part.state.input.path` — file path in repo (required)
- `$.part.state.input.ref` (optional) — branch/commit ref

**Detail**: File path + optional ref. Example: `src/main.ts` or `src/main.ts@main`

**Output**: File content in `$.part.state.output`.

#### Other GitHub tools

The following tools follow the same pattern:
- `github-cli-local_get_repo`: Repo name → fetches repo info
- `github-cli-local_list_pull_requests`: Branch/filter → PR list
- `github-cli-local_list_issues`: Filter → issue list

### Web & Network

#### webfetch (183 events)

**Row verb**: Fetched / Fetching  
**Correlation**: `$.part.callID`

**Input fields**:
- `$.part.state.input.url` — URL to fetch (required)
- `$.part.state.input.format` (optional) — "markdown" | "html" | etc.

**Detail**: URL. Example: `https://docs.example.com/api`

**Output**: Page content (large) in `$.part.state.output`; row shows URL only.

#### websearch (6 events)

**Row verb**: Searched the web / Searching the web  
**Correlation**: `$.part.callID`

**Input fields**:
- `$.part.state.input.query` — search query (required)

**Detail**: Query string.

**Output**: Search results in `$.part.state.output`.

### Planning & Delegation

#### skill (486 events)

**Row verb**: Loaded / Loading  
**Correlation**: `$.part.callID`

**Input fields**:
- `$.part.state.input.name` — skill name (required)

**Detail**: Skill name. Example: `refactor`

**Output**: Skill documentation/instructions in `$.part.state.output` (large; row shows name only).

#### todowrite (348 events)

**Row verb**: Planned / Planning  
**Correlation**: `$.part.callID`

**Input fields**:
- `$.part.state.input.todos[]` — checklist items
- `$.part.state.input.todos[].content` — item text
- `$.part.state.input.todos[].status` — `pending`, `in_progress`, or `completed`
- `$.part.state.input.todos[].priority` — optional priority metadata; not shown in the checklist

**Detail**: Show the active item and completion count in the collapsed row.

**Output**: The checklist may also be present in `$.part.state.output`; the renderer reads the authoritative `input.todos` payload.

#### task (19 events)

**Row verb**: Delegated / Delegating  
**Correlation**: `$.part.callID`

**Input fields**:
- `$.part.state.input.description` (optional)
- `$.part.state.input.model` (optional)

**Detail**: Description or "Task delegated".

**Output**: Task details in `$.part.state.output`.

#### oga_delegate, oga_resume, oga_handoff, oga_steer

These are meta-tools for Oga task control:
- `oga_delegate`: Delegate a task. Detail: description + model.
- `oga_resume`: Resume a task. Detail: task ID + instruction.
- `oga_handoff`: Hand off to another model/provider. Detail: source → target.
- `oga_steer`: Steer a running task. Detail: steer instruction.

All have `$.part.callID` for correlation.

### Database Tools (Non-Oga)

#### postgres-local_query (2 events), postgres-local_list_schemas (4 events), etc.

Same structure as oga-database tools:
- Input: SQL or schema name
- Output: Query result
- Detail: Query or schema name

#### laravel-boost_* (2 events)

Application-specific tools; follow the same pattern of input → output extraction.

### Lesser-Used Tools (< 20 events each)

The following tools appear but are not detailed further due to rarity:

- `patch`: Apply a unified patch
- `execute`: Execute a workflow step
- `oga_route`: Routing query
- `oga_profiles`: List profiles
- `oga_health`: Health check
- `oga_wait`: Wait for an event
- `oga_models`: List available models
- `oga_cancel`: Cancel a task
- `oga_map`: Map a codebase
- `shell`: Execute a shell command (generic, like bash)
- `invalid`: Malformed tool call (2 events)

**Row extraction**: Use tool name as verb, extract arguments from `input`, and show in detail. Treat as generic tool call: `Used invalid` etc.

---

## Other Agent Event Types

### agent.text (11,880 events)

**Structure**:
```
{
  "type": "text",
  "part": {
    "type": "text",
    "text": "..."
  }
}
```

**Row rendering**:
- **Kind**: "message"
- **Title**: "Agent message"
- **Detail**: First ~160 chars of `$.part.text` or `$.text` (truncated)
- **Phase**: "info"

**Should produce a row**: YES, if text is non-empty and not just whitespace. A whitespace-only message (opencode emits `"\n\n"` placeholders) should be marked `minor: true` or skipped.

### agent.message_start (no significant payload)

**Should produce a row**: NO. This is a marker event; the message content arrives in subsequent events. A row here would be redundant with the content events that follow.

### agent.message_end (11,880 events)

**Should produce a row**: NO. The message body is the same as in the leading `agent.message_start` or subsequent content events. Rendering this event creates a duplicate row with no new information. Fold it into the message's starting event or skip it entirely.

### agent.message_update (9,763 events)

**Structure**:
```
{
  "type": "message_update",
  "message": { ... }
}
```

**Should produce a row**: MAYBE. Message updates report partial/streamed content. A renderer can:
- Fold them into one row with the final message content (preferred)
- Show them as separate "Message update" rows with an indicator they are partial (less preferred; creates clutter)

**If rendered**: Show as `minor: true`, with detail showing change or stream progress.

### agent.assistant (1,039 events)

**Structure**:
```
{
  "type": "assistant",
  "message": {
    "role": "assistant",
    "content": [ ... ]
  }
}
```

**Should produce a row**: NO. This is a finished message echo; the content was already rendered when the text/tool parts arrived. A separate row here duplicates what the constituent parts already showed.

### agent.user (578 events)

**Structure**:
```
{
  "type": "user",
  "message": {
    "role": "user",
    "content": [ ... ]
  }
}
```

**Should produce a row**: MAYBE. Depends on renderer philosophy:
- If tool results are folded into their tool call, user messages (which are tool result echoes) should also be folded
- If tool results get separate rows, user messages should too (for symmetry)

**If rendered**: Show first line of content as detail; mark as `minor: true` if it is just a tool result echo (which it usually is in this context).

### agent.step_start (38,872 events)

**Structure**:
```
{
  "type": "step_start",
  "part": {
    "type": "step-start",
    "snapshot": "<hash>"
  }
}
```

**Should produce a row**: NO. This is internal model reasoning boundary marker. Rows for every reasoning step create noise; fold this into a single "Reasoning" pulse line at the turn level.

### agent.step_finish (38,179 events)

**Structure**:
```
{
  "type": "step_finish",
  "part": {
    "type": "step-finish",
    "reason": "tool-calls",
    "tokens": {
      "total": <num>,
      "input": <num>,
      "output": <num>,
      "reasoning": <num>
    }
  }
}
```

**Should produce a row**: NO. Same as `step_start`; internal reasoning boundary.

**Note on tokens**: Token counts are present here but should only be aggregated/reported at the turn level, not per-step.

### agent.error (rare in trace, 790 in tool_use)

**When part of agent.tool_use**: Handled above under tool_use → status="error".

**As standalone event**: Payload is `{ "type": "error", "error": { ... } }`

**Should produce a row**: YES. Error rows are necessary and should show:
- **Kind**: "error"
- **Phase**: "failed"
- **Title**: "Agent error"
- **Detail**: Error message from `$.error.message` or nested `$.error.data.message`

### agent.turn_start (not in provided data, but in schema)

**Should produce a row**: NO. Internal boundary marker.

### agent.turn_end (rare in trace)

**Structure**:
```
{
  "type": "turn_end",
  "message": { ... },
  "usage": { "input": <tokens>, "output": <tokens>, ... }
}
```

**Should produce a row**: MAYBE. Depends on verbosity:
- A "Turn completed" row summarizing token counts is useful for long traces
- Each turn is not usually user-facing, so mark as `minor: true`

**If rendered**: Title "Turn completed", detail showing token counts and duration.

### agent.result (final turn summary)

**Structure**:
```
{
  "is_error": <bool>,
  "usage": { ... },
  "total_cost_usd": <num>,
  "num_turns": <num>,
  "stop_reason": <str>
}
```

**Should produce a row**: YES (marked `minor: true` for minimal trace mode). This is the run summary.

**Kind**: "usage"  
**Phase**: "completed" | "failed" (check `is_error`)  
**Title**: "Run summary"  
**Detail**: Cost + turns + token counts  
**Presentation**: usage type with cost and token fields

---

## Ambiguities & Inconsistencies

### 1. Output vs Input extraction for errors

**Problem**: When `read` fails, opencode puts the error message in `$.part.state.output` instead of `$.part.state.error`. The renderer must extract the target file from `input.filePath` but pull the error text from `output` on error status, not from an `error` field.

**Fix**: Check `status` field. On "error", prefer `state.error` if present, otherwise fall back to first 160 chars of `state.output`.

The command target always comes from `state.input.command`, including error
events whose `state.output` contains a JSON error object. Output is a detail or
expansion value only; it must never replace the command in the collapsed row.

### 2. Tool name normalization

**Problem**: Tool names vary in case and style:
- `read` (lowercase) vs `Read` (capitalized)
- `oga_query` vs `oga-query` vs `OgaQuery`
- `github-cli-local_create_pull_request` (full MCP id) vs shorter names in some contexts

**Fix**: Normalize all tool names to lowercase, underscores. Use `TOOL_VERBS` constant to look up verb. Fold `mcp__server__function` style names into the `function` part after normalizing case.

### 3. File path inconsistency in read errors

**Problem**: When a read fails with "File not found", the error message suggests alternatives:
```
File not found: /path/to/file.txt

Did you mean one of these?
/path/to/file
```

**Fix**: The target is the attempted path in `input.filePath`, not the suggested path in the error. Extract from input; error is just context.

### 4. Command truncation

**Problem**: Bash commands can be very long (1000+ chars), but the caller ran `head -n 80` or similar, so the output is already truncated. The row detail should not show the full command if it spans lines; truncate to ~120 chars.

**Fix**: Truncate `input.command` to 120 chars, replacing newlines with ` ↵ `.

### 5. Plural/Singular in results

**Problem**: `grep` and `glob` outputs say "Found 1 match" or "Found 42 files", but the detail field should be consistent:
- "Found 1 file" (singular)
- "Found 42 files" (plural)
- "Found 0 matches" (zero is plural)

**Fix**: Parse the count from output and apply singular/plural rules.

### 6. MCP tool name length

**Problem**: GitHub CLI tool names are very long: `github-cli-local_create_pull_request`. The row title is too wide.

**Fix**: Abbreviate to logical verb: `create_pull_request` → "Opened" (via TOOL_VERBS), or show as `[GitHub] Opened`.

### 7. Turn and step token counting

**Problem**: Every `step_finish` event has token counts. Summing them up gives turn-level counts. But at the turn level, there's also a `turn_end` with the same or slightly different token numbers. Which is authoritative?

**Fix**: Use the turn-level `turn_end` or final `result` event for aggregate counts. Do not sum step-level counts in the trace; they are internal.

### 8. Whitespace-only agent.text

**Problem**: opencode emits `{ "type": "text", "part": { "text": "\n\n" } }` as a placeholder when the model stops briefly. Rendering this as a row shows an empty "Agent message" line.

**Fix**: Check if text is empty or only whitespace after trimming. If so, mark as `minor: true` or skip entirely.

---

## Rendering Examples

### Read a file (success)

```
Payload:
{
  "type": "tool_use",
  "part": {
    "tool": "read",
    "callID": "call_abc123",
    "state": {
      "status": "completed",
      "input": { "filePath": "src/main.ts", "offset": 10, "limit": 50 }
    }
  }
}

Row:
- verb: "Read"
- target: "src/main.ts"
- detail: "lines 10–60"
- phase: "completed"
```

### Read a file (error)

```
Payload:
{
  "type": "tool_use",
  "part": {
    "tool": "read",
    "callID": "call_abc123",
    "state": {
      "status": "error",
      "input": { "filePath": "src/missing.ts" },
      "output": "File not found: src/missing.ts\n\nDid you mean one of these?\nsrc/missing.tsx"
    }
  }
}

Row:
- verb: "Read"
- target: "src/missing.ts"
- detail: "File not found" (or full error message)
- phase: "failed"
```

### Edit a file

```
Payload:
{
  "type": "tool_use",
  "part": {
    "tool": "edit",
    "callID": "call_def456",
    "state": {
      "status": "completed",
      "input": {
        "filePath": "web/src/Button.tsx",
        "oldString": "color: blue;",
        "newString": "color: red;"
      }
    }
  }
}

Row:
- verb: "Edited"
- target: "web/src/Button.tsx"
- detail: "" (file path is enough)
- phase: "completed"
```

### Run bash (success)

```
Payload:
{
  "type": "tool_use",
  "part": {
    "tool": "bash",
    "callID": "call_ghi789",
    "state": {
      "status": "completed",
      "input": { "command": "ls -la src" },
      "output": "-rw-r--r-- 1 user user 1234 Aug 31 10:00 main.ts\n..."
    }
  }
}

Row:
- verb: "Ran"
- target: "" (no fixed target; command is inline)
- detail: "ls -la src"
- phase: "completed"
```

### Search with grep

```
Payload:
{
  "type": "tool_use",
  "part": {
    "tool": "grep",
    "callID": "call_jkl012",
    "state": {
      "status": "completed",
      "input": {
        "pattern": "TODO",
        "path": "src",
        "include": "*.ts"
      },
      "output": "Found 5 matches\nsrc/main.ts:10: TODO: fix auth\n..."
    }
  }
}

Row:
- verb: "Searched"
- target: "src"
- detail: "\"TODO\" in src --include=\"*.ts\" · Found 5 matches"
- phase: "completed"
```

### Create a pull request

```
Payload:
{
  "type": "tool_use",
  "part": {
    "tool": "github-cli-local_create_pull_request",
    "callID": "call_mno345",
    "state": {
      "status": "completed",
      "input": {
        "title": "fix(auth): token expiry",
        "head": "task/auth-fix",
        "base": "main"
      },
      "output": "https://github.com/org/repo/pull/123"
    }
  }
}

Row:
- verb: "Opened"
- target: "task/auth-fix → main"
- detail: "fix(auth): token expiry"
- result: "https://github.com/org/repo/pull/123"
- phase: "completed"
```

### Query the database

```
Payload:
{
  "type": "tool_use",
  "part": {
    "tool": "oga-database-local_query",
    "callID": "call_pqr678",
    "state": {
      "status": "completed",
      "input": { "sql": "SELECT * FROM tasks WHERE status='running'" },
      "output": "[{\"id\": \"task_123\", ...}, ...]"
    }
  }
}

Row:
- verb: "Queried"
- target: "oga database"
- detail: "SELECT * FROM tasks WHERE status='running'"
- phase: "completed"
```

---

## Event Types That Should NOT Produce Rows

| Event Type | Reason |
|---|---|
| `agent.message_start` | Marker; content follows in subsequent events. |
| `agent.message_end` | Duplicate of content events; fold into message start. |
| `agent.step_start` | Internal reasoning boundary; use turn-level aggregates. |
| `agent.step_finish` | Internal reasoning boundary; token counts belong at turn level. |
| `agent.assistant` | Echo of finished message; content already rendered. |
| `agent.turn_start` | Marker; activities within the turn have their own rows. |
| `heartbeat` | Keep hidden; shows as `minor: true` at best. |
| `hook_started`, `hook_finished` | Internal scaffolding; keep as `minor: true`. |

---

## Summary for Renderer Implementation

1. **On every `agent.tool_use` event**:
   - Extract `part.callID` as the correlation ID
   - Look up verb from TOOL_VERBS[normalizedToolName]
   - Extract target from tool-specific path in `part.state.input` (see per-tool section)
   - Extract detail from input fields (tool-specific)
   - On error status, detail = error message from `part.state.error` or first line of `part.state.output`
   - On success, optionally show outcome from `part.state.output` (but be selective for large outputs)

2. **For message events** (`agent.text`, `agent.user`, etc.):
   - Render only if content is non-empty and non-whitespace
   - Use first ~160 chars as detail
   - Mark as `minor: true` if they are scaffolding

3. **For lifecycle events** (usage, result, turn summary):
   - Render with aggregated counts and cost
   - Mark as `minor: true` unless verbosity is high

4. **Fold related events**:
   - One tool call (started, in-progress, result) → one row
   - One message (started, content, ended) → one row
   - Use `callID` or `actionId` to match parts belonging together

5. **Test against real data**:
   - Verify target extraction does not pull file paths from output
   - Verify commands are truncated but identifiable
   - Verify error messages are shown, not swallowed
   - Verify no empty or whitespace-only rows appear
