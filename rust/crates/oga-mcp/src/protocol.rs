//! MCP wire types, capability advertisements, and tool schemas.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

pub const MCP_PROTOCOL_VERSION: &str = "2026-07-28";
pub const LEGACY_PROTOCOL_VERSION: &str = "2025-11-25";
pub const EARLIEST_PROTOCOL_VERSION: &str = "2025-06-18";

const MCP_INSTRUCTIONS: &str = concat!(
    "Oga runs agent work on external provider accounts. The loop: `query` locates code in a project, `tasks` finds work already delegated so a second task is not opened on it, `delegate` starts new work, `inspect` reads what a task produced.\n",
    "Nothing waits for a task to finish: `oga watch <taskId>` prints a line when one settles, then `inspect` reads it.\n",
    "By state: `reply` answers needs_input, `steer` redirects a running task, `resume` continues a stopped one, `handoff` moves one to another model or account, `cancel` stops it, `archive` hides it. Every task response carries `next`: the calls that fit its state.\n",
    "Delegation sends the prompt, the directory's memories, and whatever the worker reads on disk to an external provider account. Confirm that destination and data scope with the user before the first delegate in a project, and when a task would widen it.",
);

const DELEGATE_DESCRIPTION: &str = concat!(
    "Start a task on an external provider account, run by another agent in a directory on this machine. ",
    "It answers once the task exists, with its id, state, account and model; the work continues afterwards, so read results with inspect. To continue a task's earlier work, resume it instead. ",
    "Naming neither profile nor model routes by the project's rules; naming either skips them. With dependsOn or startAt the task waits and starts on its own.",
);

const MODELS_DESCRIPTION: &str = concat!(
    "List the models this machine can send work to, and the routing rules (`love`) that pick one when delegate names none. ",
    "Each row has the profile and model ids to pass to delegate, whether it is enabled or preferred, and the effort levels it accepts; `unavailable` says why a worker cannot start. `usage` holds each profile's quota use unless `usage` is false. ",
    "Only preferred, enabled models by default: widen with `onlyPreferred: false` or `onlyEnabled: false`.",
);

const INSPECT_DESCRIPTION: &str = concat!(
    "Read one task's record as it stands: state, what the worker wrote, cost, and how it ended. The only tool that reports what a task produced. ",
    "`fields: [\"attempts\"]` adds earlier runs; `[\"prompt\"]` or `[\"shippedPrompt\"]` return the brief as written and as sent.",
);

const HEALTH_DESCRIPTION: &str = "Report whether this Oga broker is reachable, with its version, MCP contract version and build. It says nothing about accounts or workers; models covers that.";

const TASKS_DESCRIPTION: &str = concat!(
    "Find delegated tasks. With no filters: tasks updated since local midnight, newest first. ",
    "Any filter (state, since, until, parent, profile, archived, query) searches all history, and filters combine. `query` searches title, tldr and prompt; title matches rank first, and `match` says which hit. ",
    "Rows are summaries; inspect has a task's output.",
);

const MEMORY_DESCRIPTION: &str = concat!(
    "Read and write durable facts for one project directory: decisions, conventions, constraints. Every later worker there gets them in its prompt, so keep secrets and running-task state out. ",
    "`set` raises the key's version; pass `expectedVersion` so a change made in between is not overwritten.",
);

const QUERY_DESCRIPTION: &str = concat!(
    "Ask in plain language where something lives in a project, instead of searching the tree. ",
    "Answers with a `path#symbol` anchor, a few candidates, or a miss, which means search instead. `code: true` returns the source under each anchor, so a file read afterwards is often unneeded. ",
    "The index can lag behind disk; read the source it points at before acting on it.",
);

const REPLY_DESCRIPTION: &str = concat!(
    "Answer a task parked in needs_input and let it carry on in the same session, so the brief needs no repeating. ",
    "`scope` replaces what it may touch, which is how a worker stopped by a path outside its scope is let through.",
);

const RESUME_DESCRIPTION: &str = concat!(
    "Continue a stopped task (failed, cancelled, blocked, completed, or waiting to start) in its existing session, keeping its id and everything the worker read. ",
    "Without an instruction it retries; a completed task needs one. It unarchives the task and recreates a removed worktree checkout. ",
    "Refused for accounts running a custom command, which keep no session.",
);

const STEER_DESCRIPTION: &str = concat!(
    "Leave an instruction for a running task without stopping it. ",
    "A worker that takes input mid-run has it within seconds; otherwise it runs as a follow-up turn once the current run finishes clean. The answer says which.",
);

const HANDOFF_DESCRIPTION: &str = concat!(
    "Move a task to another model, profile, or both, running or not. Use it when the destination is wrong for the work or the account cannot take it: rate limit, auth or billing failure, refusal. ",
    "The task keeps its id, title, scope and history. It keeps its session on the same profile, and starts a fresh one with a written brief on another. ",
    "A wait on the old account is dropped; a wait on a prerequisite or a start time survives.",
);

const CANCEL_DESCRIPTION: &str = concat!(
    "Stop a task and kill its worker. `reason` is stored as its error and shown to the user; the task can still be inspected, resumed, handed off, or archived. ",
    "A completed or failed task is refused. Takes an array of ids, each reported separately.",
);

const COMPLETE_DESCRIPTION: &str = concat!(
    "Mark a task completed on the caller's word, for work that landed though the worker never reported it. ",
    "It applies from any state; a running worker is stopped first. The original outcome is kept beside `assertedBy` and `reason`, and anything waiting on the task then starts. The deliverable is not checked.",
);

const ARCHIVE_DESCRIPTION: &str = concat!(
    "Archive a task, or restore one with `archived: false`: it leaves the active lists but keeps its id and history. A running task is stopped first. ",
    "A worktree task's checkout is removed in the background when no other live task shares it and it has no uncommitted work; the answer reports what happened to the checkout and branch. Takes an array of ids.",
);

const DELETE_WORKTREE_DESCRIPTION: &str = concat!(
    "Delete a worktree task's checkout, keeping the task and, unless `deleteBranch`, its branch. Give `taskId` for one task or `project` for every settled one in a project. ",
    "Busy or shared checkouts are skipped and reported. Archiving already removes an idle checkout; this does it without archiving.",
);

const PROMPT_DESCRIPTION: &str = "The brief the worker runs, in markdown, 1 to 64000 characters, sent as written. inspect `fields: [\"shippedPrompt\"]` shows what was sent.";

/// The `prompt` field's description closed by this scope's brief rules, so
/// the caller reads them only when it is about to write a brief. Empty rules
/// add no trailing blank.
fn prompt_description(caller_prompt: &str) -> String {
    let rules = caller_prompt.trim();
    if rules.is_empty() {
        return PROMPT_DESCRIPTION.to_owned();
    }
    format!("{PROMPT_DESCRIPTION}\n\n{rules}")
}

const SCOPE_DESCRIPTION: &str = concat!(
    "Paths relative to the task's directory: a file, `dir/**`, or `**`. Read never implies write. ",
    "A write entry names a directory, and one that does not exist yet needs `/**`, including output folders the worker's checks write to. Omitted, the worker gets `**` for both.",
);
const SCOPE_REPLACE_DESCRIPTION: &str =
    "Replaces the task's scope; same shape as delegate's. Omitted, the scope stays as it was.";
const WORKTREE_DESCRIPTION: &str = concat!(
    "Run the task in its own git checkout on its own branch instead of in cwd; `true` takes every default, or `join` shares another task's checkout. ",
    "The repository needs at least one commit. The worker can read its whole history, so check with the user if it holds anything they would not send to the provider.",
);
const KIND_DESCRIPTION: &str = "The kind of work, matched against the routing rules to pick a model. A subject rule outranks a class rule. Omitted, it is read from the prompt. Ignored when profile or model is named.";
const EFFORT_DESCRIPTION: &str = "How hard the model thinks on this run. Omitted, the routing rule's effort applies, else the model's default. models lists the levels each model accepts.";
const CAN_DELEGATE_DESCRIPTION: &str = "Let this worker create tasks of its own. Off by default, and then it is not served delegate at all.";
const TIMEOUT_DESCRIPTION: &str = "Wall-clock limit in milliseconds, up to a day. On expiry the task fails with code `timeout` and can be resumed. Omitted, no limit.";
const DEPENDS_ON_DESCRIPTION: &str = "Task ids this task waits for; it starts on its own once all have completed. onBlockerFailure decides what happens when one does not.";
const BLOCKER_FAILURE_DESCRIPTION: &str = "When a prerequisite fails, is cancelled, or ends blocked: \"hold\" (default) blocks this task until that one is resumed; \"run\" starts it anyway once all have settled.";
const DELEGATE_START_AT_DESCRIPTION: &str = "Start later: an ISO instant in UTC, or a duration from now such as \"30m\", \"4h\", \"2d\". resume starts it sooner, cancel drops it. With dependsOn, both must be satisfied.";
const RESUME_START_AT_DESCRIPTION: &str = concat!(
    "Hold the resume: \"rate_limit\" waits for the account's limit to clear, or an ISO instant in UTC, or a duration such as \"30m\", \"4h\", \"2d\". ",
    "Refused with model, effort, timeoutMs or scope.",
);
const ISO_DATETIME_PATTERN: &str = r"^(?:(?:\d\d[2468][048]|\d\d[13579][26]|\d\d0[48]|[02468][048]00|[13579][26]00)-02-29|\d{4}-(?:(?:0[13578]|1[02])-(?:0[1-9]|[12]\d|3[01])|(?:0[469]|11)-(?:0[1-9]|[12]\d|30)|(?:02)-(?:0[1-9]|1\d|2[0-8])))T(?:(?:[01]\d|2[0-3]):[0-5]\d(?::[0-5]\d(?:\.\d+)?)?(?:Z))$";
const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

fn described(mut schema: Value, description: &str) -> Value {
    schema
        .as_object_mut()
        .expect("schema is an object")
        .insert("description".into(), json!(description));
    schema
}

#[derive(Debug, Clone, Deserialize)]
pub struct JsonRpcRequest {
    #[serde(default)]
    pub jsonrpc: Option<String>,
    #[serde(default)]
    pub id: Option<Value>,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

#[derive(Debug, Clone, Serialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: &'static str,
    pub id: Value,
    #[serde(flatten)]
    pub body: JsonRpcBody,
}

#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum JsonRpcBody {
    Result { result: Value },
    Error { error: JsonRpcError },
}

#[derive(Debug, Clone, Serialize)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl JsonRpcResponse {
    pub fn result(id: Value, result: Value) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            body: JsonRpcBody::Result { result },
        }
    }

    pub fn error(id: Value, code: i64, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            body: JsonRpcBody::Error {
                error: JsonRpcError {
                    code,
                    message: message.into(),
                    data: None,
                },
            },
        }
    }
}

pub fn initialize_result(protocol_version: &str, server_version: &str) -> Value {
    json!({
        "protocolVersion": protocol_version,
        "capabilities": {
            "tools": { "listChanged": true }
        },
        "serverInfo": { "name": "oga", "version": server_version },
        "instructions": MCP_INSTRUCTIONS,
    })
}

pub fn task_field_schema() -> Value {
    json!({
        "type": "array",
        "items": { "type": "string", "enum": [
            "routing", "context", "label", "location", "scope", "prompt", "shippedPrompt",
            "output", "attempts", "completion", "spend", "all"
        ]},
        "description": "Record groups to return, replacing the default set. `all` for everything; `prompt`, `shippedPrompt`, `output` and `attempts` can be large.",
    })
}

fn object_schema(properties: Map<String, Value>, required: &[&str]) -> Value {
    let mut schema = Map::new();
    schema.insert("type".into(), json!("object"));
    schema.insert("properties".into(), Value::Object(properties));
    if !required.is_empty() {
        schema.insert("required".into(), json!(required));
    }
    Value::Object(schema)
}

fn field_property(properties: &mut Map<String, Value>) {
    properties.insert("fields".into(), task_field_schema());
}

fn scope_schema(description: &str) -> Value {
    let object = object_schema(
        Map::from_iter([
            (
                "read".into(),
                described(
                    json!({ "type": "array", "items": { "type": "string" }, "maxItems": 200 }),
                    "Paths the worker may open; write paths are readable too.",
                ),
            ),
            (
                "write".into(),
                described(
                    json!({ "type": "array", "items": { "type": "string" }, "maxItems": 200 }),
                    "Paths the worker may change. Empty makes the task read-only.",
                ),
            ),
        ]),
        &["read", "write"],
    );
    described(object, description)
}

fn worktree_schema() -> Value {
    let object = object_schema(
        Map::from_iter([
            (
                "from".into(),
                described(
                    json!({ "type": "string", "minLength": 1, "maxLength": 200 }),
                    "Commit, branch or tag to start from. Default HEAD. Not with join.",
                ),
            ),
            (
                "branch".into(),
                described(
                    json!({ "type": "string", "minLength": 1, "maxLength": 200 }),
                    "Branch the work lands on. Default `oga/<slug-of-title>`. Not with join.",
                ),
            ),
            (
                "link".into(),
                described(
                    json!({ "type": "array", "items": { "type": "string", "minLength": 1 }, "maxItems": 32 }),
                    "Untracked paths relative to cwd to copy into the checkout, such as dependency folders. Omitted, git-ignored folders are copied except dependencies, build output and agent state (`node_modules`, `target`, `dist`, `build`, `.venv`, `.claude`); files such as `.env` are not. `[]` copies nothing. Not with join.",
                ),
            ),
            (
                "join".into(),
                described(
                    json!({ "type": "string", "minLength": 1 }),
                    "Task id whose checkout and branch this task works in, as a reviewer or fixer would. Runs sharing a checkout overwrite each other unless dependsOn orders them. Not with from, branch or link.",
                ),
            ),
        ]),
        &[],
    );
    described(
        json!({ "anyOf": [{ "anyOf": [{ "type": "boolean" }, object] }, { "type": "null" }] }),
        WORKTREE_DESCRIPTION,
    )
}

fn tool(name: &str, description: &str, input_schema: Value) -> Value {
    let mut input_schema = input_schema;
    if let Some(schema) = input_schema.as_object_mut() {
        schema.insert(
            "$schema".into(),
            json!("https://json-schema.org/draft/2020-12/schema"),
        );
    }
    json!({ "name": name, "description": description, "inputSchema": input_schema })
}

/// The tools this caller is served. A task that may not hand work onward
/// never sees `delegate`, so no prompt can talk it into one.
pub fn tool_list(can_delegate: bool, caller_prompt: &str) -> Value {
    let mut tools = if can_delegate {
        vec![delegate_tool(caller_prompt)]
    } else {
        Vec::new()
    };
    tools.extend(shared_tools());
    json!({ "tools": tools })
}

fn delegate_tool(caller_prompt: &str) -> Value {
    let mut delegate = Map::from_iter([
        (
            "profile".into(),
            described(
                json!({ "type": "string" }),
                "Provider account (profile id) to run on; skips routing and uses its default model. models lists the ids.",
            ),
        ),
        (
            "model".into(),
            described(
                json!({ "type": "string", "minLength": 1, "maxLength": 200 }),
                "Model id to run; skips routing. A unique short name is accepted.",
            ),
        ),
        (
            "preference".into(),
            described(
                json!({ "type": "string", "enum": ["balanced", "quality", "cost", "speed"] }),
                "Ignored. Use kind, or name profile and model.",
            ),
        ),
        (
            "prompt".into(),
            described(
                json!({ "type": "string", "minLength": 1, "maxLength": 64000 }),
                &prompt_description(caller_prompt),
            ),
        ),
        (
            "cwd".into(),
            described(
                json!({ "type": "string", "minLength": 1 }),
                "Absolute, existing directory the worker runs in. Scope paths are relative to it and memories come from it.",
            ),
        ),
        (
            "parent".into(),
            described(
                json!({ "type": "string" }),
                "Task id to group this task under, so tasks with `parent` lists the batch together.",
            ),
        ),
        ("scope".into(), scope_schema(SCOPE_DESCRIPTION)),
        (
            "canDelegate".into(),
            described(
                json!({ "type": "boolean", "default": false }),
                CAN_DELEGATE_DESCRIPTION,
            ),
        ),
        (
            "kind".into(),
            described(
                json!({ "type": "string", "enum": [
                    "mechanical", "general", "build", "context", "reasoning",
                    "ui", "ux", "backend", "database", "docs", "tests", "review", "research", "refactor"
                ] }),
                KIND_DESCRIPTION,
            ),
        ),
        (
            "effort".into(),
            described(
                json!({ "type": "string", "enum": ["minimal", "low", "medium", "high", "xhigh", "max"] }),
                EFFORT_DESCRIPTION,
            ),
        ),
        (
            "tldr".into(),
            described(
                json!({ "type": "string", "minLength": 1, "maxLength": 200 }),
                "One plain sentence in the user's terms: what the task will do and to what. No markdown.",
            ),
        ),
        (
            "title".into(),
            described(
                json!({ "type": "string", "minLength": 1, "maxLength": 60 }),
                "Short imperative label, no markdown. With worktree it also names the branch.",
            ),
        ),
        (
            "timeoutMs".into(),
            described(
                json!({ "type": "integer", "minimum": 1, "maximum": 86400000 }),
                TIMEOUT_DESCRIPTION,
            ),
        ),
        ("worktree".into(), worktree_schema()),
        (
            "dependsOn".into(),
            described(
                json!({ "type": "array", "items": { "type": "string", "minLength": 1 }, "maxItems": 16 }),
                DEPENDS_ON_DESCRIPTION,
            ),
        ),
        (
            "onBlockerFailure".into(),
            described(
                json!({ "type": "string", "enum": ["hold", "run"] }),
                BLOCKER_FAILURE_DESCRIPTION,
            ),
        ),
        (
            "startAt".into(),
            described(
                json!({ "type": "string", "minLength": 1, "maxLength": 64 }),
                DELEGATE_START_AT_DESCRIPTION,
            ),
        ),
        (
            "attachments".into(),
            described(
                json!({ "type": "array", "items": { "type": "string", "minLength": 1 }, "maxItems": 20 }),
                "Files or images shown beside the task for the person following it. Not sent to the worker; name paths it needs in the prompt.",
            ),
        ),
    ]);
    field_property(&mut delegate);
    tool(
        "delegate",
        DELEGATE_DESCRIPTION,
        object_schema(delegate, &["prompt", "cwd", "tldr", "title"]),
    )
}

/// Everything every caller gets, delegating or not.
fn shared_tools() -> Vec<Value> {
    let mut tools = Vec::new();
    tools.push(tool(
        "models",
        MODELS_DESCRIPTION,
        object_schema(
            Map::from_iter([
                (
                    "onlyPreferred".into(),
                    described(
                        json!({ "type": "boolean", "default": true }),
                        "Answer with only the models marked preferred. False returns every model the enabled accounts offer.",
                    ),
                ),
                (
                    "onlyEnabled".into(),
                    described(
                        json!({ "type": "boolean", "default": true }),
                        "Only models switched on for this project. False also lists the switched-off ones, which delegate refuses.",
                    ),
                ),
                (
                    "profile".into(),
                    described(
                        json!({ "type": "string" }),
                        "Restrict to one profile id. A profile that does not exist, or is disabled, is an error rather than an empty answer.",
                    ),
                ),
                (
                    "provider".into(),
                    described(
                        json!({ "type": "string", "enum": ["claude", "codex", "opencode", "opencode-2", "antigravity", "pi", "fx", "cursor"] }),
                        "Restrict to the accounts of one provider.",
                    ),
                ),
                (
                    "refresh".into(),
                    described(
                        json!({ "type": "boolean" }),
                        "Re-read each account's model catalog instead of the cache, e.g. after a model was added.",
                    ),
                ),
                (
                    "query".into(),
                    described(
                        json!({ "type": "string" }),
                        "Restrict to model ids containing this text, case-insensitive.",
                    ),
                ),
                (
                    "cwd".into(),
                    described(
                        json!({ "type": "string" }),
                        "Project directory whose enabled models and routing rules apply. Omitted, the user-level configuration answers instead.",
                    ),
                ),
                (
                    "usage".into(),
                    described(
                        json!({ "type": "boolean", "default": true }),
                        "Include each account's quota use. Set false to skip the extra read per account.",
                    ),
                ),
                (
                    "limit".into(),
                    described(
                        json!({ "type": "integer", "minimum": 1, "default": 50 }),
                        "How many model rows come back; `moreRows` counts the rest.",
                    ),
                ),
            ]),
            &[],
        ),
    ));

    let mut inspect = Map::from_iter([(
        "taskId".into(),
        described(
            json!({ "type": "string" }),
            "Oga task id, as answered by delegate, resume, reply, or handoff.",
        ),
    )]);
    field_property(&mut inspect);
    tools.push(tool(
        "inspect",
        INSPECT_DESCRIPTION,
        object_schema(inspect, &["taskId"]),
    ));

    tools.push(tool(
        "health",
        HEALTH_DESCRIPTION,
        object_schema(Map::new(), &[]),
    ));

    let mut tasks = Map::from_iter([
        (
            "limit".into(),
            described(
                json!({ "type": "integer", "minimum": 1, "maximum": 100, "default": 20 }),
                "How many rows come back.",
            ),
        ),
        (
            "state".into(),
            described(
                json!({ "anyOf": [
                    { "type": "string", "enum": ["queued", "preparing_checkout", "removing_checkout", "pending", "running", "needs_input", "answered", "blocked", "completed", "failed", "cancelled"] },
                    { "type": "array", "minItems": 1, "items": { "type": "string", "enum": ["queued", "preparing_checkout", "removing_checkout", "pending", "running", "needs_input", "answered", "blocked", "completed", "failed", "cancelled"] } }
                ]}),
                "Only tasks sitting in this state right now; pass an array to match any of several.",
            ),
        ),
        (
            "since".into(),
            described(
                json!({ "type": "string", "format": "date-time", "pattern": ISO_DATETIME_PATTERN }),
                "Only tasks updated at or after this ISO instant in UTC.",
            ),
        ),
        (
            "until".into(),
            described(
                json!({ "type": "string", "format": "date-time", "pattern": ISO_DATETIME_PATTERN }),
                "Only tasks updated strictly before this ISO instant in UTC.",
            ),
        ),
        (
            "order".into(),
            described(
                json!({ "type": "string", "enum": ["newest", "oldest"], "default": "newest" }),
                "Order by when each task was last updated. Newest first by default.",
            ),
        ),
        (
            "profile".into(),
            described(
                json!({ "type": "string" }),
                "Only tasks sent to this profile id.",
            ),
        ),
        (
            "parent".into(),
            described(
                json!({ "type": "string" }),
                "One fan-out batch: the task with this id, plus every task delegated with it as parent.",
            ),
        ),
        (
            "archived".into(),
            described(
                json!({ "type": "string", "enum": ["active", "only", "include"] }),
                "`active` (default) skips archived tasks, `only` returns just them, `include` both.",
            ),
        ),
        (
            "query".into(),
            described(
                json!({ "type": "string", "minLength": 1 }),
                "Case-insensitive text to find in title, tldr, or prompt.",
            ),
        ),
    ]);
    field_property(&mut tasks);
    tools.push(tool("tasks", TASKS_DESCRIPTION, object_schema(tasks, &[])));

    tools.push(tool(
        "memory",
        MEMORY_DESCRIPTION,
        object_schema(
            Map::from_iter([
                (
                    "action".into(),
                    described(
                        json!({ "type": "string", "enum": ["list", "get", "set", "remove"] }),
                        "What to do: read every entry for the directory, read one by key, write one, or delete one.",
                    ),
                ),
                (
                    "cwd".into(),
                    described(
                        json!({ "type": "string", "minLength": 1 }),
                        "Absolute project directory, matched exactly. Use the cwd delegate runs work in.",
                    ),
                ),
                (
                    "key".into(),
                    described(
                        json!({ "type": "string" }),
                        "Name of the fact, unique in the directory. Required by get, set and remove.",
                    ),
                ),
                (
                    "value".into(),
                    described(
                        json!({ "type": "string" }),
                        "The fact in plain text. Required by set; replaces what the key held.",
                    ),
                ),
                (
                    "expectedVersion".into(),
                    described(
                        json!({ "type": "integer", "minimum": 0, "maximum": MAX_SAFE_INTEGER }),
                        "The version last read. A different stored version refuses the write. Omitted, it overwrites.",
                    ),
                ),
            ]),
            &["action", "cwd"],
        ),
    ));

    tools.push(tool(
        "query",
        QUERY_DESCRIPTION,
        object_schema(
            Map::from_iter([
                (
                    "cwd".into(),
                    described(
                        json!({ "type": "string", "minLength": 1 }),
                        "Absolute path of the project to search.",
                    ),
                ),
                (
                    "q".into(),
                    described(
                        json!({ "type": "string", "minLength": 1 }),
                        "What you are looking for, in plain words. A path or `path#name` goes straight there.",
                    ),
                ),
                (
                    "in".into(),
                    described(
                        json!({
                            "oneOf": [
                                { "type": "string" },
                                { "type": "array", "items": { "type": "string" } },
                            ],
                        }),
                        "Folders or files to answer from, relative to the project. Leave out to search the whole project.",
                    ),
                ),
                (
                    "code".into(),
                    described(
                        json!({ "type": "boolean", "default": false }),
                        "Return the source under each anchor.",
                    ),
                ),
                (
                    "limit".into(),
                    described(
                        json!({ "type": "integer", "minimum": 1, "maximum": 20, "default": 7 }),
                        "How many hits to return.",
                    ),
                ),
            ]),
            &["cwd", "q"],
        ),
    ));

    let mut reply = Map::from_iter([
        (
            "taskId".into(),
            described(
                json!({ "type": "string" }),
                "Oga task id of the task parked in needs_input.",
            ),
        ),
        (
            "answer".into(),
            described(
                json!({ "type": "string", "minLength": 1 }),
                "The answer to the task's question.",
            ),
        ),
        ("scope".into(), scope_schema(SCOPE_REPLACE_DESCRIPTION)),
    ]);
    field_property(&mut reply);
    tools.push(tool(
        "reply",
        REPLY_DESCRIPTION,
        object_schema(reply, &["taskId", "answer"]),
    ));

    let mut resume = Map::from_iter([
        (
            "taskId".into(),
            described(
                json!({ "type": "string" }),
                "Oga task id of the stopped task to continue.",
            ),
        ),
        (
            "instruction".into(),
            described(
                json!({ "type": "string", "minLength": 1, "maxLength": 64000 }),
                "What to do next, carrying only what changed. Optional when retrying; required after a completed run.",
            ),
        ),
        (
            "timeoutMs".into(),
            described(
                json!({ "type": "integer", "minimum": 1, "maximum": 86400000 }),
                "Wall-clock limit for this run in milliseconds. Omitted, the old limit stays.",
            ),
        ),
        ("scope".into(), scope_schema(SCOPE_REPLACE_DESCRIPTION)),
        (
            "model".into(),
            described(
                json!({ "type": "string", "minLength": 1, "maxLength": 200 }),
                "Model for this run, on the same account. Not with startAt or queue; another account is handoff.",
            ),
        ),
        (
            "effort".into(),
            described(
                json!({ "type": "string", "enum": ["minimal", "low", "medium", "high", "xhigh", "max"] }),
                "Effort for this run. Not with startAt or queue.",
            ),
        ),
        (
            "startAt".into(),
            described(
                json!({ "type": "string", "minLength": 1, "maxLength": 64 }),
                RESUME_START_AT_DESCRIPTION,
            ),
        ),
        (
            "queue".into(),
            described(
                json!({ "type": "string", "enum": ["add", "clear"] }),
                "\"add\" queues the instruction to run after the current run finishes clean (only instruction allowed); \"clear\" drops the queue.",
            ),
        ),
    ]);
    field_property(&mut resume);
    tools.push(tool(
        "resume",
        RESUME_DESCRIPTION,
        object_schema(resume, &["taskId"]),
    ));

    let mut steer = Map::from_iter([
        (
            "taskId".into(),
            described(
                json!({ "type": "string" }),
                "Oga task id of the running task.",
            ),
        ),
        (
            "instruction".into(),
            described(
                json!({ "type": "string", "minLength": 1, "maxLength": 64000 }),
                "A course correction in a sentence or two, not a new brief.",
            ),
        ),
        (
            "model".into(),
            described(
                json!({ "type": "string", "minLength": 1, "maxLength": 200 }),
                "Refused; use handoff to change model.",
            ),
        ),
    ]);
    field_property(&mut steer);
    tools.push(tool(
        "steer",
        STEER_DESCRIPTION,
        object_schema(steer, &["taskId"]),
    ));

    let mut handoff = Map::from_iter([
        (
            "taskId".into(),
            described(
                json!({ "type": "string" }),
                "Oga task id of the task to move.",
            ),
        ),
        (
            "profile".into(),
            described(
                json!({ "type": "string", "minLength": 1 }),
                "Destination profile id. Omitted, only the model changes.",
            ),
        ),
        (
            "model".into(),
            described(
                json!({ "type": "string", "minLength": 1, "maxLength": 200 }),
                "Model on the destination. Required on the same profile; defaults to the new account's default model.",
            ),
        ),
        (
            "effort".into(),
            described(
                json!({ "type": "string", "enum": ["minimal", "low", "medium", "high", "xhigh", "max"] }),
                "Effort for the new run. Naming one for a running task restarts it.",
            ),
        ),
        ("scope".into(), scope_schema(SCOPE_REPLACE_DESCRIPTION)),
    ]);
    field_property(&mut handoff);
    tools.push(tool(
        "handoff",
        HANDOFF_DESCRIPTION,
        object_schema(handoff, &["taskId"]),
    ));

    let mut cancel = Map::from_iter([
        (
            "taskId".into(),
            described(
                json!({ "anyOf": [
                    { "type": "string" },
                    { "type": "array", "minItems": 1, "items": { "type": "string" } }
                ]}),
                "Oga task id, or an array of ids.",
            ),
        ),
        (
            "reason".into(),
            described(
                json!({ "type": "string", "minLength": 1, "maxLength": 500 }),
                "Why it was stopped, shown to the user. Default \"cancelled by caller\".",
            ),
        ),
    ]);
    field_property(&mut cancel);
    tools.push(tool(
        "cancel",
        CANCEL_DESCRIPTION,
        object_schema(cancel, &["taskId"]),
    ));

    let mut complete = Map::from_iter([
        (
            "taskId".into(),
            described(
                json!({ "type": "string" }),
                "Oga task id of the task to record as completed.",
            ),
        ),
        (
            "assertedBy".into(),
            described(
                json!({ "type": "string", "minLength": 1, "maxLength": 200 }),
                "Who or what checked the work landed.",
            ),
        ),
        (
            "reason".into(),
            described(
                json!({ "type": "string", "minLength": 1, "maxLength": 500 }),
                "How it was established that the work landed.",
            ),
        ),
    ]);
    field_property(&mut complete);
    tools.push(tool(
        "complete",
        COMPLETE_DESCRIPTION,
        object_schema(complete, &["taskId", "assertedBy", "reason"]),
    ));

    let mut archive = Map::from_iter([
        (
            "taskId".into(),
            described(
                json!({ "anyOf": [
                    { "type": "string" },
                    { "type": "array", "minItems": 1, "items": { "type": "string" } }
                ]}),
                "Oga task id, or an array of ids.",
            ),
        ),
        (
            "archived".into(),
            described(
                json!({ "type": "boolean", "default": true }),
                "False restores an archived task.",
            ),
        ),
        (
            "deleteBranch".into(),
            described(
                json!({ "type": "boolean", "default": false }),
                "Also delete the task's branch once its checkout is gone. Kept if it has unmerged commits or is checked out elsewhere.",
            ),
        ),
    ]);
    field_property(&mut archive);
    tools.push(tool(
        "archive",
        ARCHIVE_DESCRIPTION,
        object_schema(archive, &["taskId"]),
    ));

    tools.push(tool(
        "worktree-remove",
        DELETE_WORKTREE_DESCRIPTION,
        object_schema(
            Map::from_iter([
                (
                    "taskId".into(),
                    described(
                        json!({ "type": "string" }),
                        "Worktree task whose checkout to remove. Give this or project.",
                    ),
                ),
                (
                    "project".into(),
                    described(
                        json!({ "type": "string" }),
                        "Project whose settled worktree checkouts to remove. Give this or taskId.",
                    ),
                ),
                (
                    "deleteBranch".into(),
                    described(
                        json!({ "type": "boolean", "default": false }),
                        "Also delete the branch, unless it has unmerged commits or is checked out elsewhere.",
                    ),
                ),
            ]),
            &[],
        ),
    ));

    tools
}

pub fn text_content(value: impl Into<String>) -> Value {
    json!({ "type": "text", "text": value.into() })
}
