//! MCP wire types, capability advertisements, and tool schemas.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

pub const MCP_PROTOCOL_VERSION: &str = "2026-07-28";
pub const LEGACY_PROTOCOL_VERSION: &str = "2025-11-25";
pub const EARLIEST_PROTOCOL_VERSION: &str = "2025-06-18";

pub const MCP_INSTRUCTIONS: &str = concat!(
    "Oga runs agent work on external provider accounts. `delegate` creates a task and answers with its id; every other tool addresses a task that already exists by that id.\n",
    "The loop: `query` locates code in a project, `tasks` finds work already delegated so a second task is not opened on the same thing, `delegate` starts new work, `inspect` reads what a task has become.\n",
    "Nothing here waits for a task to finish. The `oga watch <taskId>` command-line watcher prints a line when one settles, and `inspect` is what turns that line into the task's record.\n",
    "Which tool reaches a task depends on the state `inspect` reports: `reply` answers one parked in needs_input, `steer` leaves an instruction for one still running, `resume` continues one that has stopped, `handoff` moves one to another model or account, `cancel` stops it, `archive` drops it out of the active lists.\n",
    "Every task response carries `next`: the calls that fit the state that task is now in.\n",
    "Delegation sends the prompt, the memories stored for the task's directory, and whatever the worker reads on disk to an external provider account. Confirm that destination and that data scope with the user before the first delegate in a project, and again when a task would widen it."
);

const DELEGATE_DESCRIPTION: &str = concat!(
    "Create a task and run it on an external provider account: implementation, research, review, writing, or analysis carried out by another agent in a directory on this machine. ",
    "The call answers as soon as the task exists, with its id, its state, and the account and model it went to; the worker keeps running afterwards, so nothing in the answer describes finished work. inspect reads that later. ",
    "Use it for work no existing task owns — continuing work a task already did is resume on that task's id, which keeps the session and everything that worker read. ",
    "cwd must be an existing absolute directory. Name neither profile nor model and the destination comes from the project's routing rules, read off the prompt and kind; name either one and those rules are skipped. ",
    "A task given dependsOn or startAt is created waiting and starts on its own; every other task starts at once. ",
    "Refused when the caller is itself a task that was delegated without canDelegate."
);

const MODELS_DESCRIPTION: &str = concat!(
    "List the models this machine can send work to, and the rules that pick one when a caller names none. ",
    "Answers `{ love, models }`. Each `models` row carries the profile id and model id to pass to delegate, the model's capabilities, whether it is enabled, preferred, or named by a rule, the effort levels it accepts, and — unless `usage` is off — how much of that account's quota is spent. ",
    "`love` is the routing rules. A rule's `when` names the kinds of work it takes: classes, or subjects such as ui, backend, database, docs, tests, review, research, refactor. An empty `when` takes every kind no other rule claimed, and a subject match outranks a class match. A rule's `models` is the ordered chain tried first to last. ",
    "Only enabled profiles are listed. By default only preferred, enabled models come back: widen with `onlyPreferred: false`, or `onlyEnabled: false` to see what is switched off."
);

const INSPECT_DESCRIPTION: &str = concat!(
    "Read one task's record as it stands now: the state it is in, what the worker wrote, what the run cost, and how it ended. ",
    "This is the only tool that reports what a task produced. The id that delegate, resume, reply, or handoff answered with describes a run that had barely started. ",
    "By default it answers with routing, location, labels, scope, output, completion and spend. `fields: [\"attempts\"]` adds what earlier runs of the same task produced, and `fields: [\"prompt\"]` or `[\"shippedPrompt\"]` return the brief as written and as sent."
);

const HEALTH_DESCRIPTION: &str = concat!(
    "Report whether this Oga broker is reachable and which contract it speaks: its version, its MCP contract version, the build it came from, and whether the source tree that build came from has moved on since. ",
    "A reachable broker always answers `ok`, so the call failing is itself the answer. It says nothing about provider accounts or whether any worker can run — models covers that."
);

const TASKS_DESCRIPTION: &str = concat!(
    "Find delegated tasks. With no arguments it answers with the tasks updated since local midnight, newest first: id, state, title, the directory each runs in, and originCwd for one with its own checkout. ",
    "`query` is case-insensitive text searched over title, tldr, and prompt across all history; title matches rank first, then tldr, then prompt, and `match` says which one hit. ",
    "Passing any of state, since, until, parent, profile, archived, or query drops the midnight default and searches all history. Filters combine. ",
    "`fields` replaces the columns rather than adding to them: `[\"label\"]` brings back tldr, `[\"routing\"]` the account and model, `[\"spend\"]` the cost, `[\"completion\"]` how it ended, `[\"prompt\"]` the first 200 characters of the brief, `[\"all\"]` everything a row can carry. ",
    "Rows are summaries; one task's output comes from inspect."
);

const MEMORY_DESCRIPTION: &str = concat!(
    "Read and write the durable facts kept for one project directory: decisions, conventions, and constraints that outlive any single task. Delegation ships that directory's memories into the worker's prompt, so what is stored here is what every later worker there starts from. ",
    "`list` answers with every entry for the directory; `get` with one entry or null; `set` writes one, creating the key at version 1 and raising the version by one on each later write; `remove` deletes one and reports whether anything was there. ",
    "Secrets and the state of a running task do not belong here: every entry is handed to every worker in that directory and nothing expires it."
);

const QUERY_DESCRIPTION: &str = concat!(
    "Ask in plain language where something lives in a project, instead of searching the tree for it. ",
    "Answers markdown: one anchor such as `src/adapters.ts#emailDriver` when the index is confident, a few candidates when it is not, and a plain miss when nothing matches, which is the signal to search instead. ",
    "The index is built from the project on first use and reconciled against disk on every call, but it can still name code that has changed since, so read the source it points at before acting on it. ",
    "A cwd that is not an existing absolute directory, or one holding no indexable files, is an error rather than an empty answer."
);

const REPLY_DESCRIPTION: &str = concat!(
    "Answer the question a task asked and let it carry on. Only a task parked in needs_input takes one; any other state is refused. ",
    "The task keeps its id and its session, so the answer lands on top of everything the worker already read and the brief does not need repeating. ",
    "An optional scope replaces the paths the task may touch, which is how a worker stopped by a path outside its scope is let through."
);

const RESUME_DESCRIPTION: &str = concat!(
    "Continue a task that has stopped — failed, cancelled, blocked, completed, or waiting to start — in the session it already has, keeping its id, its history, and everything the worker read. ",
    "With no instruction it picks up where it stopped; a completed task needs one, since there is nothing left to retry. ",
    "It unarchives an archived task, and recreates the checkout of a worktree task whose directory was removed. ",
    "A task still working is refused: steer leaves an instruction for that one, reply answers one parked on a question. ",
    "Refused when the task's account runs a custom command, because those runs capture no session to continue."
);

const STEER_DESCRIPTION: &str = concat!(
    "Leave an instruction for a task that is still running, without stopping it and without losing the work done so far. ",
    "No runner accepts input mid-run, so the instruction waits and runs as a follow-up turn once the current run finishes clean; the answer reports it as queued. A run that fails, asks a question, or ends blocked leaves it waiting untouched, and cancelling the task discards it. ",
    "Only a running task takes one. Anything else is refused: resume continues a stopped task, reply answers one parked on a question."
);

const HANDOFF_DESCRIPTION: &str = concat!(
    "Move a task to another model, another profile, or both, in one call, running or not, with no cancel first. ",
    "Use it when the destination is wrong for the work, or when the account cannot take it: a rate limit, an auth or billing failure, a provider that refused the work. ",
    "The task keeps its id, title, scope, and attempt history. It keeps its session when the profile does not change, and opens a fresh one carrying a written brief when it moves accounts. ",
    "A wait on the account it is leaving is dropped and the task runs; a wait on a prerequisite or a scheduled start survives the move. ",
    "Continuing on the same model and account is resume."
);

const CANCEL_DESCRIPTION: &str = concat!(
    "Stop a task and kill its worker's process tree, from queued, pending, running, needs_input, answered, or blocked. Instructions queued behind the run are discarded with it. ",
    "The record survives, with `reason` stored as the task's error and shown to the user, so a cancelled task can still be inspected, resumed, handed off, or archived. ",
    "A task already cancelled is left as it is; one that already completed or failed is refused. Given an array of ids it answers per id, and an id it cannot cancel does not fail the rest."
);

const COMPLETE_DESCRIPTION: &str = concat!(
    "Record a task as completed on the caller's word, for work that landed even though the worker never attested it. ",
    "It applies from any state, including a task still running, whose worker is stopped first; a task already completed comes back unchanged. ",
    "The recorded outcome is not erased: the original completion code is kept beside `assertedBy`, the reason, and the time, so the record shows the completion was asserted rather than reported. Anything waiting on this task then starts as if it had finished normally. ",
    "Nothing in this call inspects the deliverable — the assertion is taken as given."
);

const ARCHIVE_DESCRIPTION: &str = concat!(
    "Archive a task, or restore one, without deleting its history: an archived task leaves the active lists and stays addressable by its id. ",
    "A task still working is stopped first, then archived. A worktree task's checkout is removed when no other live task shares it and it holds no uncommitted work; otherwise it stays, and `checkout` reports which and why. ",
    "`deleteBranch` also asks for the task's own branch; it is refused unless a worktree task is being archived, and the branch is kept anyway whenever the checkout was kept. `branchOutcome` and `branchReason` report whether it was deleted, kept, or already gone. ",
    "`archived: false` restores a task; resume unarchives one too, and starts it running again. Given an array of ids it answers per id."
);

const DELETE_WORKTREE_DESCRIPTION: &str = concat!(
    "Delete the checkout a worktree task ran in, leaving the task and its history in place, and its branch too unless `deleteBranch` asks otherwise. Name one task with `taskId`, or every settled worktree task of one project with `project` — exactly one of the two. ",
    "The original project directory is never touched: the checkout holds its own copies of whatever was seeded into it. ",
    "A task still working, or a checkout another live task shares, is reported as skipped rather than failing, and a checkout already gone is not an error. A task that never ran in a checkout is an error. ",
    "Archiving already removes an idle checkout; this removes one without archiving the task."
);

const PROMPT_DESCRIPTION: &str = concat!(
    "The brief the worker runs, as markdown, 1 to 64000 characters. It is the only account of the work the worker gets, since it cannot see this conversation, so anything it needs to know has to be in it. ",
    "It is sent as written: a one-line brief arrives as one line, a pasted bug report arrives as that report. Headings and numbered steps earn their length on work with several parts, and get in the way on work with one. ",
    "Oga wraps it with the directory's memories, the scope, and its own reporting protocol before sending it; inspect `fields: [\"shippedPrompt\"]` returns the result."
);
const SCOPE_DESCRIPTION: &str = concat!(
    "Paths the worker may read and write, relative to the task's directory: a literal file path, `dir/**` for a subtree, or `**` for the whole tree. Both lists are required, at most 200 entries each. ",
    "Read access never carries write access. A write entry names the directory written into rather than the file, and a directory that does not exist yet needs the `/**` suffix — so an output directory that checks or builds write into has to be listed there for those checks to run. ",
    "On delegate, leaving scope out gives the worker `**` for both. On reply, resume, and handoff it replaces the scope the task is running under, and leaving it out keeps that scope as it was."
);
const WORKTREE_DESCRIPTION: &str = concat!(
    "Run the task in its own git checkout of the repository at cwd, on a branch of its own, so the work lands there instead of in the user's working tree — or `join` a checkout another task already has. ",
    "Omitted, `false`, or null runs the task directly in cwd. `true` takes every default: the branch is `oga/<slug-of-title>`, falling back to `oga/<taskId>`, and git-ignored directories and `.env*` files are copied in from the original at any depth while editor and agent state is not. ",
    "cwd must sit inside a git repository holding at least one commit; a repository with no commits is refused. ",
    "The checkout carries the repository's whole history, so the worker can read every commit in it — get the user's approval where that history holds anything they would not send to the provider."
);
const KIND_DESCRIPTION: &str = concat!(
    "The kind of work this is, matched against the project's routing rules to pick a model: a class — mechanical, general, build, context, reasoning — or a subject — ui, ux, backend, database, docs, tests, review, research, refactor. ",
    "A rule naming the subject outranks one naming the class. Omitted, the kind is read from the wording of the prompt. Ignored when profile or model names the destination outright."
);
const EFFORT_DESCRIPTION: &str = concat!(
    "How hard the model is asked to think on this run, weakest to strongest: minimal, low, medium, high, xhigh, max. ",
    "Omitted, a routing rule's own configured effort applies if it has one, else the model's default. ",
    "Passed through by claude, codex, opencode, opencode-2 and pi; antigravity ignores it, since its model ids carry the level. models reports the levels each model accepts in `efforts`."
);
const ALLOW_QUESTIONS_DESCRIPTION: &str = concat!(
    "Whether the worker may stop and ask. True, the default, lets it park in needs_input with a question and wait there until reply answers. ",
    "False tells it to report a blocked result instead of asking, so the task settles without a turn from the caller."
);
const CAN_DELEGATE_DESCRIPTION: &str = concat!(
    "Whether this worker may create tasks of its own. Off by default: the delegate tool is not served to it at all, so no prompt can talk it into fanning work out. ",
    "Turn it on for a task whose job is to split work up."
);
const TIMEOUT_DESCRIPTION: &str = concat!(
    "Wall-clock limit for the run, in milliseconds, from 1 to 86400000 — a day. ",
    "When it expires the worker is stopped and the task lands in failed with completion code `timeout`, from where it can still be resumed. Omitted, the run has no time limit."
);
const DEPENDS_ON_DESCRIPTION: &str = concat!(
    "Task ids this task waits for, at most 16. It is created waiting and starts on its own once every one of them has completed. ",
    "An unknown id, this task's own id, or a chain that loops back here is refused. What happens when a prerequisite does not complete is onBlockerFailure."
);
const BLOCKER_FAILURE_DESCRIPTION: &str = concat!(
    "What a prerequisite that ended failed, cancelled, or blocked does to this task. ",
    "\"hold\", the default, leaves this task blocked naming that prerequisite as the reason, and puts it back to waiting if that prerequisite is resumed. ",
    "\"run\" starts this task anyway, once every prerequisite has settled, however each one ended."
);
const DELEGATE_START_AT_DESCRIPTION: &str = concat!(
    "Hold the task instead of starting it now: an ISO instant in UTC, or a duration counted from now written as whole minutes, hours, or days — \"30m\", \"4h\", \"2d\". ",
    "It waits showing when it starts, then starts on its own; resume starts it sooner and cancel drops it. ",
    "An instant already past is refused, and so is \"rate_limit\", which belongs to resuming a run that hit one. With dependsOn it becomes a floor under that wait: the task starts once both are satisfied."
);
const RESUME_START_AT_DESCRIPTION: &str = concat!(
    "Hold the resume instead of running it now. ",
    "\"rate_limit\" waits for the account's own limit to clear: the reset time the failed run reported is the hint, the account's live usage is what releases it, and a run that named no reset time is checked again after ten minutes. ",
    "Otherwise an ISO instant in UTC, or a duration counted from now — \"30m\", \"4h\", \"2d\". ",
    "The held task carries only its instruction, so model, effort, timeoutMs, scope and allowQuestions are refused alongside it. Resume again without startAt to start now, or cancel to drop it."
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
        "description": "Which parts of the task record come back. It replaces this tool's own default set rather than adding to it, so name every group you want. `[\"all\"]` is the whole record. `prompt`, `shippedPrompt`, `output` and `attempts` can each be large.",
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
    let object = object_schema(
        Map::from_iter([
            (
                "read".into(),
                described(
                    json!({ "type": "array", "items": { "type": "string" }, "maxItems": 200 }),
                    "Paths the worker may open. Every write path is readable too, so a path listed there does not need repeating here.",
                ),
            ),
            (
                "write".into(),
                described(
                    json!({ "type": "array", "items": { "type": "string" }, "maxItems": 200 }),
                    "Paths the worker may create, edit, or delete. An empty list makes the task read-only.",
                ),
            ),
        ]),
        &["read", "write"],
    );
    described(object, SCOPE_DESCRIPTION)
}

fn worktree_schema() -> Value {
    let object = object_schema(
        Map::from_iter([
            (
                "from".into(),
                described(
                    json!({ "type": "string", "minLength": 1, "maxLength": 200 }),
                    "Commit, branch or tag the new branch starts from. Omitted, it starts from the repository's current HEAD. Refused together with join.",
                ),
            ),
            (
                "branch".into(),
                described(
                    json!({ "type": "string", "minLength": 1, "maxLength": 200 }),
                    "Branch the work lands on, which the user reviews afterwards. Omitted, it is `oga/<slug-of-title>`, falling back to `oga/<taskId>` when the title yields no slug. Refused together with join.",
                ),
            ),
            (
                "link".into(),
                described(
                    json!({ "type": "array", "items": { "type": "string", "minLength": 1 }, "maxItems": 32 }),
                    "Untracked paths, relative to cwd, copied into the checkout — the dependency and build directories the worker would otherwise have to install again. Omitted, git-ignored directories and `.env*` files are copied at any depth, skipping `.claude`, `.agents`, `.plans`, `.malico`, `.DS_Store` and `*.bun-build`; `[]` copies nothing. Each path becomes the checkout's own copy, so the original is never written through. A path is refused by name when it is absolute, escapes cwd, does not exist, is tracked by git, is named twice, or sits inside another entry. Refused together with join.",
                ),
            ),
            (
                "join".into(),
                described(
                    json!({ "type": "string", "minLength": 1 }),
                    "Oga task id whose checkout this task enters instead of getting one of its own: same directory, same branch, no second copy of the tree — how a reviewer or a fixer works in the exact tree another task wrote. Refused together with from, branch or link, and refused when that task is archived, has no checkout, or belongs to another project. Nothing serialises the runs that share a checkout: two tasks writing there at the same time overwrite each other unless dependsOn orders them.",
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
pub fn tool_list(can_delegate: bool) -> Value {
    let mut tools = if can_delegate {
        vec![delegate_tool()]
    } else {
        Vec::new()
    };
    tools.extend(shared_tools());
    json!({ "tools": tools })
}

fn delegate_tool() -> Value {
    let mut delegate = Map::from_iter([
        (
            "profile".into(),
            described(
                json!({ "type": "string" }),
                "Profile id — one configured provider account — to run the task on. Naming it skips the project's routing rules, and the model then defaults to that account's own default model. models lists the ids.",
            ),
        ),
        (
            "model".into(),
            described(
                json!({ "type": "string", "minLength": 1, "maxLength": 200 }),
                "Model id to run. Naming one skips the project's routing rules. It is resolved against the named profile's catalog, or against every enabled profile when profile is omitted, and a trailing short name is accepted when exactly one model matches it. A name matching several is refused, and one the catalog does not list is passed through and refused at dispatch if the account does not offer it.",
            ),
        ),
        (
            "preference".into(),
            described(
                json!({ "type": "string", "enum": ["balanced", "quality", "cost", "speed"] }),
                "Validated against these four values, then ignored: it does not change which account or model the task runs on. Steer the destination with kind, or name profile and model outright.",
            ),
        ),
        (
            "prompt".into(),
            described(
                json!({ "type": "string", "minLength": 1, "maxLength": 64000 }),
                PROMPT_DESCRIPTION,
            ),
        ),
        (
            "cwd".into(),
            described(
                json!({ "type": "string", "minLength": 1 }),
                "Absolute path of the directory the worker runs in; it must already exist. It is resolved through symlinks, and that resolved path is what scope paths are relative to and what memories are read from. With worktree set the worker runs in a checkout of this repository instead, and this path is kept as the task's project.",
            ),
        ),
        (
            "parent".into(),
            described(
                json!({ "type": "string" }),
                "Task id to group this task under, so a fan-out reads as one batch: `tasks` with `parent` set answers with that task plus everything delegated under it. It must name a delegated task that exists.",
            ),
        ),
        ("scope".into(), scope_schema()),
        (
            "allowQuestions".into(),
            described(
                json!({ "type": "boolean", "default": true }),
                ALLOW_QUESTIONS_DESCRIPTION,
            ),
        ),
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
                "One plain sentence, in the user's own terms, saying what the task will do and to what. Required, at most 200 characters, no markdown. It is what the task screen shows as the task's summary.",
            ),
        ),
        (
            "title".into(),
            described(
                json!({ "type": "string", "minLength": 1, "maxLength": 60 }),
                "Short imperative label for the task, required, at most 60 characters, no markdown — what a sidebar shows at a glance. With worktree it also becomes the default branch name, slugified.",
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
                "Absolute paths to files or images to record alongside the request, at most 20, shown next to it in the task screen for the person following the task. They are not read or sent to the worker, and they are not checked for existence: a path the worker needs has to be named in the prompt.",
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
                        "Answer with only the models switched on for this project. False also returns the ones switched off, each carrying `enabled: false`; they cannot be passed to delegate.",
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
                        json!({ "type": "string", "enum": ["claude", "codex", "opencode", "opencode-2", "antigravity", "pi"] }),
                        "Restrict to the accounts of one provider.",
                    ),
                ),
                (
                    "refresh".into(),
                    described(
                        json!({ "type": "boolean" }),
                        "Ask each account's CLI for its catalog again instead of reading the cached copy. Slower, and what to use when a model was added on the account since the last read.",
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
                        "Include each row's usage summary: percent used, when the window resets, and the rate-limited and out-of-credits flags. `known: false` means no usage source has been read, with `reason` saying why — never a silent omission. It costs a read per account beyond the cache, so set it false to skip that.",
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
                "How many rows come back, 1 to 100. The rows kept are the first in the requested order.",
            ),
        ),
        (
            "state".into(),
            described(
                json!({ "anyOf": [
                    { "type": "string", "enum": ["queued", "pending", "running", "needs_input", "answered", "blocked", "completed", "failed", "cancelled"] },
                    { "type": "array", "minItems": 1, "items": { "type": "string", "enum": ["queued", "pending", "running", "needs_input", "answered", "blocked", "completed", "failed", "cancelled"] } }
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
                "Which side of the archive to search: `active` (the default) skips archived tasks, `only` returns nothing else, `include` searches both.",
            ),
        ),
        (
            "query".into(),
            described(
                json!({ "type": "string", "minLength": 1 }),
                "Text to find in a task's title, tldr, or prompt, matched case-insensitively as a substring. Title matches rank first, then tldr, then prompt, and each row reports which in `match`.",
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
                        "Absolute path of the project directory these facts belong to, matched exactly. Use the same path delegate runs the work in, so a worker there is shipped what is written here.",
                    ),
                ),
                (
                    "key".into(),
                    described(
                        json!({ "type": "string" }),
                        "Name of the fact, unique within the directory. Required by get, set and remove. Reading a key that does not exist answers null rather than failing.",
                    ),
                ),
                (
                    "value".into(),
                    described(
                        json!({ "type": "string" }),
                        "The fact itself, in plain text. Required by set, stored trimmed, and refused when it is empty. It replaces whatever the key held.",
                    ),
                ),
                (
                    "expectedVersion".into(),
                    described(
                        json!({ "type": "integer", "minimum": 0, "maximum": MAX_SAFE_INTEGER }),
                        "The version last read for this key. When the stored version differs the write is refused as a revision conflict, so a change another caller made in between is not silently lost. Omitted, the write overwrites whatever is there.",
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
                        "Absolute path of the project to search; it must exist and hold indexable files. A caller that is itself a delegated task always searches its own project, whatever this names.",
                    ),
                ),
                (
                    "q".into(),
                    described(
                        json!({ "type": "string", "minLength": 1 }),
                        "What is being looked for, in plain language — 'where is the token refresh handled'. It matches on meaning, so it need not be a symbol name.",
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
                "The answer to the question the task asked. It is added to the session the worker already has, which still holds the brief, the scope and the reporting format.",
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
                "What the continued run should do next, in a few sentences, up to 64000 characters. Optional when retrying a run that failed, was cancelled, ended blocked, or has not started; required to follow up on one that completed. It is not a second brief: the session already holds the original one and everything the worker read, so it carries only what changed.",
            ),
        ),
        (
            "timeoutMs".into(),
            described(
                json!({ "type": "integer", "minimum": 1, "maximum": 86400000 }),
                "Wall-clock limit for the continued run, in milliseconds, up to a day. Omitted, the task keeps the limit it already had.",
            ),
        ),
        ("scope".into(), scope_schema()),
        (
            "allowQuestions".into(),
            described(
                json!({ "type": "boolean" }),
                "Whether the continued run may park in needs_input to ask. Omitted, the task keeps what it was dispatched with.",
            ),
        ),
        (
            "model".into(),
            described(
                json!({ "type": "string", "minLength": 1, "maxLength": 200 }),
                "Model for the continued run, on the account the task already runs on; the session keeps everything the worker read. Omitted, the task keeps its model. Refused together with startAt or queue. Moving to a different account is handoff.",
            ),
        ),
        (
            "effort".into(),
            described(
                json!({ "type": "string", "enum": ["minimal", "low", "medium", "high", "xhigh", "max"] }),
                "How hard the model thinks on the continued run. Omitted, the task keeps the level it already had. Refused together with startAt or queue.",
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
                "Work on the task's queue of waiting instructions instead of continuing it now. \"add\" puts the instruction behind the current run, to be taken up once that run finishes clean; instruction is then required and timeoutMs, scope, allowQuestions, model, effort and startAt are refused. \"clear\" drops everything waiting. Queued instructions run oldest first; a run that fails, asks a question, or ends blocked leaves them waiting, and cancelling the task discards them.",
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
                "What the worker should take up when its current run finishes. A course correction in a sentence or two, not a restatement of the brief it is already running.",
            ),
        ),
        (
            "model".into(),
            described(
                json!({ "type": "string", "minLength": 1, "maxLength": 200 }),
                "Refused: no runner changes model in the middle of a run. Move a running task to another model with handoff.",
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
                "Oga task id of the task to move: one that failed, was cancelled, ended blocked, or is waiting, or one still running whose work belongs on another model or account.",
            ),
        ),
        (
            "profile".into(),
            described(
                json!({ "type": "string", "minLength": 1 }),
                "Destination profile id; models lists the ids and what capacity each account has left. Omitted, the task stays on its current account and only the model changes.",
            ),
        ),
        (
            "model".into(),
            described(
                json!({ "type": "string", "minLength": 1, "maxLength": 200 }),
                "Model to run on the destination. Required when the profile stays the same, since the model change is then the whole move. Omitted on a move between accounts, the destination's default model is used — the task's own model id names a model on the account it is leaving.",
            ),
        ),
        (
            "effort".into(),
            described(
                json!({ "type": "string", "enum": ["minimal", "low", "medium", "high", "xhigh", "max"] }),
                "How hard the model thinks on the new run. Omitted, the task keeps the level it already had. Effort is fixed when a worker starts, so naming one for a task that is still running restarts it.",
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
                "Oga task id, or an array of ids to cancel in one call. Each id is reported separately, and one that cannot be cancelled does not stop the others.",
            ),
        ),
        (
            "reason".into(),
            described(
                json!({ "type": "string", "minLength": 1, "maxLength": 500 }),
                "Why it was stopped. It is stored as the task's error and shown to the user, and it applies to every id in a batch. Omitted, the record reads \"cancelled by caller\".",
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
                "Who or what checked the work landed — a person, this client, the integration. Required, at most 200 characters, and kept on the record for good.",
            ),
        ),
        (
            "reason".into(),
            described(
                json!({ "type": "string", "minLength": 1, "maxLength": 500 }),
                "How it was established that the work landed despite the outcome the run recorded. Required, at most 500 characters, and kept on the record beside the original completion code.",
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
                "Oga task id, or an array of ids to archive or restore in one call. Each id is reported separately.",
            ),
        ),
        (
            "archived".into(),
            described(
                json!({ "type": "boolean", "default": true }),
                "True, the default, archives the task. False restores an archived one to the active lists, leaving its state as it was.",
            ),
        ),
        (
            "deleteBranch".into(),
            described(
                json!({ "type": "boolean", "default": false }),
                "Also delete the task's local branch, once its checkout has been removed. The branch stays when it holds unmerged commits or is checked out somewhere else, and `branchReason` says which. Refused unless the task is being archived.",
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
                        "Oga task id of the worktree task whose checkout to remove. Give this or project, never both and never neither.",
                    ),
                ),
                (
                    "project".into(),
                    described(
                        json!({ "type": "string" }),
                        "Absolute path of a project whose worktree task checkouts to remove in one call: every settled one goes, and the rest are reported as skipped. Give this or taskId, never both and never neither.",
                    ),
                ),
                (
                    "deleteBranch".into(),
                    described(
                        json!({ "type": "boolean", "default": false }),
                        "Also delete the task's own local branch. It survives by default, and it survives anyway when it holds unmerged commits or is checked out somewhere else, with `branchReason` saying which.",
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
