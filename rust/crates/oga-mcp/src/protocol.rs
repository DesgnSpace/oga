//! MCP wire types, capability advertisements, and tool schemas.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

pub const MCP_PROTOCOL_VERSION: &str = "2026-07-28";
pub const LEGACY_PROTOCOL_VERSION: &str = "2025-11-25";
pub const EARLIEST_PROTOCOL_VERSION: &str = "2025-06-18";

pub const MCP_INSTRUCTIONS: &str = concat!(
    "The loop: read the cwd's memories, list tasks, and locate code with `oga query \"<what you need>\"`; add `--code` when you want the code back instead of just the location; then delegate the work, watch, inspect. ",
    "Before delegating, search `tasks` with `query` for the same feature, file, or command; resume a match. ",
    "Before any call — locate code with `oga query \"<what you need>\"` rather than glob or grep; add `--code` when you want the code back instead of just the location; resume the task that already owns the same work instead of dispatching a duplicate; keep decisions and conventions in memory, never secrets or task status. ",
    "Use delegate for bounded implementation, research, review, writing, and analysis, and keep goal-setting, architecture, integration, and final review here. ",
    "Delegation sends the prompt, the cwd's memories, and whatever the worker reads to an external account: approve the destination and data scope once per cwd and profile, and ask again only when a task would widen it. ",
    "After delegate, resume, or reply returns a task id, background `oga watch <taskId>` in the caller's terminal; it holds no chat turn, so several tasks run at once. ",
    "That settle line, or the alert block a later Oga result carries, is the required trigger to call inspect before you report a task's state, decide what to do next, or end a turn that tracks it — inspect's answer is the record and a line is not. ",
    "Act on the settled report: the worker already verified its own work. ",
    "Every task response carries `next`, the moves that fit the state the task is now in; take one from there rather than guessing. ",
    "Worker mode: execute the assigned brief here; never delegate it onward or open a child task for the same work."
);

const DELEGATE_DESCRIPTION: &str = concat!(
    "Hand new bounded work — implementation, research, review, writing, analysis — to an external provider, including a second opinion or work past this provider's usage limit. ",
    "Returns an Oga task id, the only handle you get, and the run carries on after the call returns. ",
    "Before delegating, search `tasks` with `query` for the same feature, file, or command; resume a match. ",
    "Give kind whenever you already know it better than the prompt shows; omit profile and model unless this task needs a particular account. Routing otherwise reads the kind of work from the prompt, and a stated kind is what a loved model per kind of work (see models) actually matches against. Give effort when you want to set how hard the model thinks yourself, separately from kind."
);

const MODELS_DESCRIPTION: &str = concat!(
    "Read a project's capacity before naming a destination: preferred, enabled models, plus every model the project's routing rules name. ",
    "Answers `{ love, models }` — `love` is the rules that route work naming no model, where a rule's `when` names the kinds of work it takes: classes, or subjects such as ui, backend, database, docs, tests, review, research, refactor. An empty `when` takes every other kind, and a subject match outranks a class match. A rule's `models` is the ordered chain tried first to last when no destination is named. ",
    "Each model row is ready to pass to delegate and carries a usage summary. ",
    "Widen it with `onlyPreferred: false`, or `onlyEnabled: false` to see what is switched off."
);

const INSPECT_DESCRIPTION: &str = concat!(
    "Read one task's record: output, scope, spend, and how the run ended. ",
    "A watch line, or the alert block a later Oga result carries, is what you call this on — the line is not the record, and state read before this call is never \"still running\". ",
    "Act on the worker's report; `fields: [\"attempts\"]` shows what earlier runs of the same task produced."
);

const HEALTH_DESCRIPTION: &str = "Check whether the Oga broker is running and read its broker and MCP contract versions, plus whether the source tree this build came from holds a newer one. For connection or compatibility diagnosis, not worker availability.";

const TASKS_DESCRIPTION: &str = "Find delegated tasks. No arguments lists active tasks updated since local midnight: id, state, title, cwd, plus originCwd for a worktree. `query` searches active history by title, tldr, or prompt, ranks title matches first then newest, and adds `match`; combine filters. Any since, until, parent, state, profile, or archived searches full history. `fields` replaces the row: `[\"label\"]` adds tldr; `[\"routing\"]`, `[\"spend\"]`, `[\"completion\"]`, `[\"all\"]` add more. Use inspect for one task.";

const MEMORY_DESCRIPTION: &str = "Read or update durable project facts shared across Oga callers and delegated workers; delegation ships the cwd's active memories automatically. Store decisions, constraints, and conventions, never secrets or transient task status. Use expectedVersion to prevent concurrent overwrites.";

const QUERY_DESCRIPTION: &str = "Ask where something lives before searching the tree — \"email driver\" can answer `src/adapters.ts#emailDriver`. Answers with one anchor when confident, a few candidates when not, or an honest miss telling you to search instead. The index can lag the code, so read the source it names before acting.";

const REPLY_DESCRIPTION: &str = "Answer the question a task is parked on in needs_input. Answer what is in scope and reversible yourself; escalate product intent, secrets, destructive actions, and requests for new authority. Send the answer alone — the session still holds the brief. Optional scope replaces the task's scope and becomes the cwd's grant.";

const RESUME_DESCRIPTION: &str = concat!(
    "Continue a task that has stopped — failed, cancelled, blocked, pending, or completed — in the session it already has, keeping its task id and everything the worker read. ",
    "This is the normal way to keep going, not a recovery path: a fresh delegation re-reads everything and loses why the work is the way it is. ",
    "With no instruction it picks up where it stopped; a completed task needs one. ",
    "Use steer for a task that is still working, and handoff to change the model or account."
);

const STEER_DESCRIPTION: &str = concat!(
    "Tell a task that is still working something, or switch its model mid-run, without stopping it. ",
    "This is how you reach a running task; resume is for one that has stopped. ",
    "When the provider takes live input the worker gets the instruction on its next turn; when it does not, the instruction waits and runs as a follow-up once this run finishes clean — the response says which happened. ",
    "A model change cannot wait: it is refused, so send the instruction on its own or use handoff."
);

const HANDOFF_DESCRIPTION: &str = concat!(
    "Move a task to another model, another profile, or both, in one call, running or not, with no cancel first. ",
    "Use it when the destination is wrong for the work, or the account failed — rate limit, auth, billing, or a provider that will not take the work. ",
    "Use resume to continue on the same model and account. ",
    "The task keeps its id, title, scope, and attempt history, and it preserves as much of the session as the destination can reopen."
);

const CANCEL_DESCRIPTION: &str = "Stop a delegated task and its worker's process tree, from queued, pending, running, needs_input, or blocked. The task record survives, so a cancelled task can still be resumed, handed off, or archived. A batch reports each id's outcome; one that cannot be cancelled — already settled, or unknown — does not fail the rest.";

const COMPLETE_DESCRIPTION: &str = concat!(
    "Mark a blocked or failed task completed on your word that the work landed, when the worker never attested its own completion. ",
    "Use it only after you checked the deliverable yourself: the original completion survives on the record, and the override permanently carries who asserted it and why. ",
    "Refused while a task is still running, and on one already completed or cancelled."
);

const ARCHIVE_DESCRIPTION: &str = concat!(
    "Archive or restore a delegated task without deleting its history. Archived tasks stay addressable by id and leave active lists. ",
    "A live task is stopped before archive. A clean worktree checkout is removed; uncommitted work keeps it, and `checkout` says where. ",
    "`deleteBranch` also requests safe deletion of its local branch. `branchOutcome` and `branchReason` report whether it was kept or removed. ",
    "Batches report each id."
);

const DELETE_WORKTREE_DESCRIPTION: &str = concat!(
    "Remove a settled worktree task's checkout now — the directory it ran in and any path it linked — without archiving the task or leaving it to cleanup. ",
    "The branch survives unless `deleteBranch` asks for it too, and a linked path is only unlinked. ",
    "A task still running or holding a question keeps its checkout. ",
    "Removing one that is already gone is not an error."
);

const SCOPE_DESCRIPTION: &str = "Paths the worker may touch, relative to cwd: literal file paths, dir/** for a subtree, ** for the whole tree. `**` is the recommended read default. Write access takes the directory, not the file, and a path that does not exist yet needs a `/**` suffix; read paths never permit writes, so generated build paths belong in write when checks need them. Stating scope records it as this cwd's grant; omitting it reuses the newest grant for the cwd. The only place permissions belong — never restate them in the prompt, and never treat a grant as a reading plan.";
const WORKTREE_DESCRIPTION: &str = "Give the task its own checkout of the repository at cwd, on a branch of its own, so the worker commits there instead of in the user's working tree — or `join` a checkout another task already has. cwd must be inside a git repository with at least one commit. `true` takes every default: ignored directories and .env* files are seeded from the original at any depth, editor and agent state is not, and the branch is oga/<slug-of-title>, falling back to oga/<taskId>. It also widens the task's read to the whole repository history, so get approval where that history holds anything private.";
const KIND_DESCRIPTION: &str = "What this work is, in the caller's own words, when you already know it better than the prompt shows: a class — mechanical, general, build, context, reasoning — or a subject — ui, ux, backend, database, docs, tests, review, research, refactor. This is what a loved model (see models) is matched against, so name it whenever the prompt's own wording would not tip off a regex — a one-line brief, or words that read as one kind while the work is really another. Some models are simply better at a given kind of work than others; that is what this decides, not how hard the work is. Omit it and the prompt decides.";
const EFFORT_DESCRIPTION: &str = "How hard the model thinks, when you want to set it yourself — the lever for that, separate from kind. Left out, a loved model's own configured effort applies if it has one, else the model's own default. Honoured by claude, codex, opencode, opencode-2 and pi; antigravity bakes its level into the model id.";
const ALLOW_QUESTIONS_DESCRIPTION: &str =
    "Whether the worker may pause in needs_input to ask. False makes it guess or stop.";
const TIMEOUT_DESCRIPTION: &str = "Hard runtime limit. The task lands in failed with code timeout.";
const DEPENDS_ON_DESCRIPTION: &str =
    "Prerequisite task ids: this task waits until every one completes, then starts on its own.";
const BLOCKER_FAILURE_DESCRIPTION: &str = "What a failed prerequisite does: \"hold\" (the default) blocks this task, and puts it back to waiting when that prerequisite is resumed. \"run\" starts it anyway.";
const DELEGATE_START_AT_DESCRIPTION: &str = "Hold the task instead of starting it now: an ISO instant, or a duration like \"30m\", \"4h\" or \"2d\". It sits `pending` showing when it starts, then starts on its own; resume it to start it sooner, or cancel to drop it. A time already past is refused. With dependsOn it becomes the floor: the task waits for both.";
const RESUME_START_AT_DESCRIPTION: &str = "Hold the resume instead of running it now. \"rate_limit\" re-arms a rate-limited task's own wait; the reset time is the hint, the account's live usage is the release rule. Also accepts an ISO instant or a duration like \"30m\", \"4h\" or \"2d\". The task sits `pending` and carries only its instruction: resume it again without startAt to start it now, or cancel to drop it.";
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
        "description": "Response shape. Replaces the default small acknowledgement, never adds to it. `[\"all\"]` is the full record; `prompt`, `shippedPrompt`, `output` and `attempts` cost real context.",
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

fn scope_schema() -> Value {
    object_schema(
        Map::from_iter([
            (
                "read".into(),
                json!({ "type": "array", "items": { "type": "string" }, "maxItems": 200 }),
            ),
            (
                "write".into(),
                json!({ "type": "array", "items": { "type": "string" }, "maxItems": 200 }),
            ),
        ]),
        &["read", "write"],
    )
}

fn worktree_schema() -> Value {
    let object = object_schema(
        Map::from_iter([
            (
                "from".into(),
                described(
                    json!({ "type": "string", "minLength": 1, "maxLength": 200 }),
                    "Commit, branch or tag the checkout starts from. Omit to start from the repository's current HEAD.",
                ),
            ),
            (
                "branch".into(),
                described(
                    json!({ "type": "string", "minLength": 1, "maxLength": 200 }),
                    "Branch the work lands on, which the user reviews afterwards. Omit for oga/<slug-of-title>; it falls back to oga/<taskId> when the title has no slug.",
                ),
            ),
            (
                "link".into(),
                described(
                    json!({ "type": "array", "items": { "type": "string", "minLength": 1 }, "maxItems": 32 }),
                    "Untracked paths, relative to cwd, the checkout is seeded from — dependencies and build caches the worker would otherwise install. Omitted, gitignored directories and .env* files are seeded at any depth and editor and agent state is skipped; `[]` seeds nothing. Each path becomes the checkout's own directory, so the original is never written through. A path that is missing or that git tracks is refused by name.",
                ),
            ),
            (
                "join".into(),
                described(
                    json!({ "type": "string", "minLength": 1 }),
                    "Oga task id whose checkout to enter instead of making a new one: same directory, same branch, no second copy of the tree — how a reviewer or a fixer works in the exact tree another task wrote to. Refused together with from, branch or link. A task with write scope must declare every unsettled writer already in that checkout with dependsOn, or the dispatch is refused; a task with no write scope may run alongside anything there.",
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

pub fn tool_list() -> Value {
    let mut tools = Vec::new();

    let mut delegate = Map::from_iter([
        (
            "profile".into(),
            described(
                json!({ "type": "string" }),
                "Profile id to run on. Omit for automatic routing; use models to choose explicitly.",
            ),
        ),
        (
            "model".into(),
            described(
                json!({ "type": "string", "minLength": 1, "maxLength": 200 }),
                "Model id for that profile. Naming one overrides the project's routing rules. Omit to use the profile's default.",
            ),
        ),
        (
            "preference".into(),
            described(
                json!({ "type": "string", "enum": ["balanced", "quality", "cost", "speed"] }),
                "Bias for automatic routing. Ignored when profile is set.",
            ),
        ),
        (
            "prompt".into(),
            described(
                json!({ "type": "string", "minLength": 1, "maxLength": 64000 }),
                "Structured markdown: Goal, Context, Required behavior, numbered Instructions, Guardrails, Output Format. No scope paths and no reading plan — the worker decides what to open.",
            ),
        ),
        (
            "cwd".into(),
            described(
                json!({ "type": "string", "minLength": 1 }),
                "Absolute path the worker runs in. Scope and grants are keyed to it.",
            ),
        ),
        (
            "parent".into(),
            described(
                json!({ "type": "string" }),
                "Task id of the first task in a fan-out, so the batch groups together.",
            ),
        ),
        ("scope".into(), described(scope_schema(), SCOPE_DESCRIPTION)),
        (
            "allowQuestions".into(),
            described(
                json!({ "type": "boolean", "default": true }),
                ALLOW_QUESTIONS_DESCRIPTION,
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
                "One plain sentence, in the user's terms, saying what the task will do and to what. No markdown.",
            ),
        ),
        (
            "title".into(),
            described(
                json!({ "type": "string", "minLength": 1, "maxLength": 60 }),
                "Short imperative label, no markdown — what a sidebar shows at a glance.",
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
                "Absolute paths to files or images to hand the worker alongside the prompt, shown next to the request in the task detail screen.",
            ),
        ),
    ]);
    field_property(&mut delegate);
    tools.push(tool(
        "delegate",
        DELEGATE_DESCRIPTION,
        object_schema(delegate, &["prompt", "cwd", "tldr", "title"]),
    ));

    tools.push(tool(
        "models",
        MODELS_DESCRIPTION,
        object_schema(
            Map::from_iter([
                (
                    "onlyPreferred".into(),
                    json!({ "type": "boolean", "default": true }),
                ),
                (
                    "onlyEnabled".into(),
                    json!({ "type": "boolean", "default": true }),
                ),
                (
                    "profile".into(),
                    described(json!({ "type": "string" }), "Restrict to one profile id."),
                ),
                (
                    "provider".into(),
                    described(
                        json!({ "type": "string", "enum": ["claude", "codex", "opencode", "opencode-2", "antigravity", "pi"] }),
                        "Restrict to one provider.",
                    ),
                ),
                (
                    "refresh".into(),
                    described(json!({ "type": "boolean" }), "Bypass the model catalog cache."),
                ),
                (
                    "query".into(),
                    described(
                        json!({ "type": "string" }),
                        "Restrict to model ids containing this case-insensitive substring.",
                    ),
                ),
                (
                    "cwd".into(),
                    described(
                        json!({ "type": "string" }),
                        "Project directory to resolve against; omit for user-level config.",
                    ),
                ),
                (
                    "usage".into(),
                    described(
                        json!({ "type": "boolean", "default": true }),
                        "Include each row's usage summary: percent used, the window it resets in, and rate-limited or out-of-credits flags. `known: false` means no usage source has been read, never a silent omission. Costs a network call per profile beyond the cache; set false to skip it.",
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
            "Oga task id returned by delegate, reply, or resume.",
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
            json!({ "type": "integer", "minimum": 1, "maximum": 100, "default": 20 }),
        ),
        (
            "state".into(),
            described(
                json!({ "anyOf": [
                    { "type": "string", "enum": ["queued", "pending", "running", "needs_input", "answered", "blocked", "completed", "failed", "cancelled"] },
                    { "type": "array", "minItems": 1, "items": { "type": "string", "enum": ["queued", "pending", "running", "needs_input", "answered", "blocked", "completed", "failed", "cancelled"] } }
                ]}),
                "Only tasks currently in this state; pass an array to match any of those.",
            ),
        ),
        (
            "since".into(),
            described(
                json!({ "type": "string", "format": "date-time", "pattern": ISO_DATETIME_PATTERN }),
                "Only tasks updated at or after this ISO timestamp.",
            ),
        ),
        (
            "until".into(),
            described(
                json!({ "type": "string", "format": "date-time", "pattern": ISO_DATETIME_PATTERN }),
                "Only tasks updated strictly before this ISO timestamp.",
            ),
        ),
        (
            "order".into(),
            described(
                json!({ "type": "string", "enum": ["newest", "oldest"], "default": "newest" }),
                "Sort by the updated timestamp. Newest first by default.",
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
                "A fan-out batch: the task with this id plus every task delegated with it as parent.",
            ),
        ),
        (
            "archived".into(),
            described(
                json!({ "type": "string", "enum": ["active", "only", "include"] }),
                "Defaults to active (non-archived) tasks.",
            ),
        ),
        (
            "query".into(),
            described(
                json!({ "type": "string", "minLength": 1 }),
                "Case-insensitive text to find in a task title, tldr, or prompt. Searches active history and ranks title matches first.",
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
                    json!({ "type": "string", "enum": ["list", "get", "set", "remove"] }),
                ),
                ("cwd".into(), json!({ "type": "string", "minLength": 1 })),
                ("key".into(), json!({ "type": "string" })),
                ("value".into(), json!({ "type": "string" })),
                (
                    "expectedVersion".into(),
                    json!({ "type": "integer", "minimum": 0, "maximum": MAX_SAFE_INTEGER }),
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
                        "A plain-language question, e.g. 'where is the token refresh handled'.",
                    ),
                ),
            ]),
            &["cwd", "q"],
        ),
    ));

    let mut reply = Map::from_iter([
        ("taskId".into(), json!({ "type": "string" })),
        (
            "answer".into(),
            described(
                json!({ "type": "string", "minLength": 1 }),
                "The answer to the question it asked, and nothing else. The session still has the brief; re-sending it buries the answer.",
            ),
        ),
        ("scope".into(), scope_schema()),
    ]);
    field_property(&mut reply);
    tools.push(tool(
        "reply",
        REPLY_DESCRIPTION,
        object_schema(reply, &["taskId", "answer"]),
    ));

    let mut resume = Map::from_iter([
        ("taskId".into(), json!({ "type": "string" })),
        (
            "instruction".into(),
            described(
                json!({ "type": "string", "minLength": 1, "maxLength": 64000 }),
                "What the continued session should do next, in a few sentences. Optional retrying a dead run; required to follow up on a completed one. Not a second brief: the session already has the goal, guardrails, scope and reporting format, so send only what changed.",
            ),
        ),
        (
            "timeoutMs".into(),
            json!({ "type": "integer", "minimum": 1, "maximum": 86400000 }),
        ),
        ("scope".into(), scope_schema()),
        ("allowQuestions".into(), json!({ "type": "boolean" })),
        (
            "model".into(),
            described(
                json!({ "type": "string", "minLength": 1, "maxLength": 200 }),
                "Model for the continued run on this same profile; the session keeps what the worker already read. Omit to keep the task's model. Refused together with startAt or queue.",
            ),
        ),
        (
            "effort".into(),
            described(
                json!({ "type": "string", "enum": ["minimal", "low", "medium", "high", "xhigh", "max"] }),
                "Reasoning effort for the continued run. Omit to keep the task's own. Refused together with startAt or queue.",
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
                "\"add\" queues the instruction to run after the current run finishes clean; \"clear\" drops what is waiting. With \"add\", instruction is required and timeoutMs, scope, allowQuestions, model, effort and startAt are refused. Items run oldest first, and a run that fails, asks a question, or ends blocked leaves them untouched. Cancelling the task discards them.",
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
                "Oga task id returned by delegate, reply, or resume.",
            ),
        ),
        (
            "instruction".into(),
            described(
                json!({ "type": "string", "minLength": 1, "maxLength": 64000 }),
                "Instruction to act on while the task keeps working. A course correction in a sentence or two, not a restatement of the brief it is already running.",
            ),
        ),
        (
            "model".into(),
            described(
                json!({ "type": "string", "minLength": 1, "maxLength": 200 }),
                "Model to use on the next turn, when the provider supports live model changes.",
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
                "Oga task id of the failed, cancelled, blocked, or waiting task to move — or one still running whose worker belongs on a better model or profile.",
            ),
        ),
        (
            "profile".into(),
            described(
                json!({ "type": "string", "minLength": 1 }),
                "Destination profile id; see profiles for capacity. Omit it to change only the model on the task's current profile.",
            ),
        ),
        (
            "model".into(),
            described(
                json!({ "type": "string", "minLength": 1, "maxLength": 200 }),
                "Model on the destination. Required when the profile stays the same — the model change is then the move. Omit on a cross-profile move for that profile's default; the task's own model id names a model on the old account.",
            ),
        ),
        (
            "effort".into(),
            described(
                json!({ "type": "string", "enum": ["minimal", "low", "medium", "high", "xhigh", "max"] }),
                "Reasoning effort for the new run. Omit to keep what the task already asked for. On a live worker a stated effort forces a restart — effort is set at spawn — so omit it to keep the switch in place.",
            ),
        ),
        ("scope".into(), scope_schema()),
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
                "Oga task id, or an array of ids, to cancel in one call.",
            ),
        ),
        (
            "reason".into(),
            described(
                json!({ "type": "string", "minLength": 1, "maxLength": 500 }),
                "Stored as the task error and shown to the user. Applies to every id in a batch.",
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
        ("taskId".into(), json!({ "type": "string" })),
        (
            "assertedBy".into(),
            described(
                json!({ "type": "string", "minLength": 1, "maxLength": 200 }),
                "Who or what verified the work landed: your name, the client, the integration.",
            ),
        ),
        (
            "reason".into(),
            described(
                json!({ "type": "string", "minLength": 1, "maxLength": 500 }),
                "Why the work demonstrably landed despite the recorded outcome. Required; an empty reason is rejected.",
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
                "Oga task id, or an array of ids, to archive or restore in one call.",
            ),
        ),
        (
            "archived".into(),
            json!({ "type": "boolean", "default": true }),
        ),
        (
            "deleteBranch".into(),
            described(
                json!({ "type": "boolean", "default": false }),
                "Also delete the local branch when it has no unmerged commits and is not checked out elsewhere. The branch stays when either safety check refuses it.",
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
                        "Oga task id of the worktree task whose checkout to remove.",
                    ),
                ),
                (
                    "project".into(),
                    described(
                        json!({ "type": "string" }),
                        "Project directory whose settled task checkouts to remove.",
                    ),
                ),
                (
                    "deleteBranch".into(),
                    described(
                        json!({ "type": "boolean", "default": false }),
                        "Also safely delete the task's own local branch when it has no unmerged commits and is not checked out elsewhere. The branch survives by default.",
                    ),
                ),
            ]),
            &[],
        ),
    ));

    json!({ "tools": tools })
}

pub fn text_content(value: impl Into<String>) -> Value {
    json!({ "type": "text", "text": value.into() })
}
