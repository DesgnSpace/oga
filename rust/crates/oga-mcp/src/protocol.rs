//! MCP wire types, capability advertisements, and tool schemas.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

pub const MCP_PROTOCOL_VERSION: &str = "2026-07-28";
pub const LEGACY_PROTOCOL_VERSION: &str = "2025-11-25";
pub const EARLIEST_PROTOCOL_VERSION: &str = "2025-06-18";

pub const MCP_INSTRUCTIONS: &str = concat!(
    "Locating code starts with query, every time: ask it for a source anchor before any glob or grep, and phrase it as what you are looking for — it answers concepts, not just names. Fall back to tree search only when query returns nothing, the project is unindexed, or you need every match; read the source before acting. This holds in worker mode too. ",
    "At the start of a new chat, identify the project cwd; call memory with action: list, and get only task-relevant keys; treat durable memory as context, while current code and task state are authoritative; never store secrets or transient task status. ",
    "Before any new task or continuation, list active and recent tasks with tasks; when one already owns the same file, feature, or command, resume it instead of dispatching a duplicate. ",
    "Use delegate for bounded implementation, research, review, writing, and analysis — not limited to coding; route execution here by default, including second opinions, capacity gaps, and work another provider's policy will not take; keep goal-setting, architecture, integration, and final review here. ",
    "Use models as the one capacity read before choosing a destination; its narrow defaults show preferred, enabled models plus the loved one — where work naming no model already goes — and its rows are ready for delegate. ",
    "Act on the settled report: the worker already verified its own work, so do not re-read changed files or re-run checks. ",
    "After delegate, resume, or reply returns a task id, start `oga watch <taskId>` in the caller's terminal and background it; it holds no chat turn, so several tasks run at once. ",
    "Every Oga tool result ends with a second block when a delegated task has settled or asked a question since you last heard — the task id and title, one line each, every move handed over once; it works whether or not the host reports a background job's exit — only Claude Code does. ",
    "A watch line or that block is the required trigger to call inspect with the task id before reporting its state, deciding what to do next, or ending a turn that tracks it; inspect's answer is the record and a line is not, so never report a task as still running from state that predates it. ",
    "Use reply for a needs_input question you can answer within the task's scope. Use resume for failed, blocked, cancelled, or follow-up work, including on a completed task. Use handoff to move a task — running or not — to another model or profile in one call; never cancel first. ",
    "Delegation sends the prompt, the cwd's saved memories, and whatever the worker reads to an external account: get approval for the destination and data once, do not re-ask per dispatch, and ask again only when either moves outside the approved scope. ",
    "Callers hold only the Oga task ID; provider session IDs stay private, and resume continues the same session. ",
    "Answer reversible in-scope questions yourself; escalate product intent, secrets, destructive actions, and requests for new authority. ",
    "Worker mode: you are executing an assigned Oga task; continue the assigned brief directly; do not call Oga, delegate another task, or create a child task for the same work. ",
    "For a clearly separate continuation, state why it is separate, create it without following it, and emit only a compact child-task pointer event with the child task id and title; do not include prompt or output in the pointer. ",
    "If stale child state exists, cancel the child and resume the parent with fresh instructions; do not follow the stale child."
);

const DELEGATE_DESCRIPTION: &str = concat!(
    "Hand a new bounded task to an external AI provider — a normal way to get work done, not a fallback. ",
    "Also for a second opinion, another model's take, or work past the current provider's usage limit. ",
    "You set the goal, done conditions, model, difficulty, effort, and scope; the worker must verify its work and report each check and result, so re-reading files or re-running checks duplicates cost — open only a broken, uncertain, or surprising item it names, or run a check its sandbox cannot run. ",
    "Give difficulty and leave profile and model out: Oga picks the account, model and thinking level from that, what the project allows, and each account's remaining usage. ",
    "When the project has a loved model, that is where every dispatch naming no model lands, for any kind of work — difficulty still sets the thinking level, and a loved model that is off, rate-limited or out of credits is skipped, with the reason on the response. ",
    "Naming a profile or model always wins — over the loved model too — and the response warns when that account is out of credits or rate-limited, lacks the model, or the project disallows it for this kind of work. ",
    "Approve a destination and data scope once per cwd and profile; that stands for later dispatches, so ask for consent only when none exists or this task would widen it. ",
    "Scope is sandbox-enforced for most tasks, and approval only for an unsandboxed one. ",
    "Grant a directory, not a single file, for write access — a normal edit writes a temporary file beside the target — and give planned output directories a /** suffix, since a path that does not exist yet stays literal. ",
    "Write rules are readable too, read rules never permit writes, so include generated build paths in write scope when checks need them. ",
    "Existing cwd-relative paths named in the prompt join the read grant automatically; writes never do. ",
    "Stating scope records it as this cwd's grant; omitting it reuses the newest grant for that cwd, falling back to ** only when there is none — a task on that fallback is flagged. ",
    "A task denied for missing scope ends with a suggestedScope to pass on resume; reply and resume may each replace scope after fresh approval. ",
    "The scope argument is the only place permissions belong: ** is the recommended read default, so a file the worker finds mid-run is never denied. ",
    "Never restate read or write paths in the prompt, and never script the reading — no file list, no order to open, no instruction to read the repository. ",
    "The worker decides what to open inside the grant, and query-first navigation ships with every dispatch; a prompt that guesses files hands it your guess instead. ",
    "Send prompt as structured markdown (Goal, Context, Required behavior, numbered Instructions, Guardrails, Output Format), never one flattened paragraph. ",
    "Always pass tldr, which the task list shows instead of the prompt. ",
    "Always pass title too: a short imperative label (max 60 chars, no markdown), readable at a glance in a sidebar — tldr is the sentence you read when you stop on the task, title is what you read in the list. ",
    "A worktree supplies what git ignores in the directory — `node_modules`, `vendor`, `.env`, build caches — so the worker starts with dependencies installed instead of spending its run on `bun install`; pass `worktree.link` only to override that list, and `[]` to supply nothing. ",
    "Pass worktree when the work should land as commits, or when more than one task runs against one repository; it also widens the task's read to the whole repository history, so get approval where that history holds anything private. Omit `branch` to use `oga/<slug-of-title>`; a caller-supplied branch wins. The worker commits the branch; after the task settles, the caller reviews it, pushes it, and opens a pull request from `~/.oga/worktrees/<taskId>`. ",
    "The returned Oga task ID is the only continuation handle; provider session IDs are private. ",
    "Dispatch returns immediately: background `oga watch <taskId>`, or take the alert block on the next Oga tool result — either one is the required trigger to call inspect, before reporting the task's state, deciding what to do next, or ending a turn that tracks it."
);

const MODELS_DESCRIPTION: &str = concat!(
    "Read the models available for a project. This is the one capacity read: the default view shows preferred, enabled models, plus the loved model when one is set. ",
    "A row with `loved: true` is where every dispatch that names no model already goes, whatever the work is; read it before naming a destination, and name one only when this task needs a different account or tier. ",
    "Use `onlyPreferred: false` to see every enabled model, or `onlyEnabled: false` when something is not working and you need to see what is turned off. ",
    "Use `query: <text>` to narrow to model ids containing that text, case-insensitive. ",
    "Each flat row gives the profile and model ready to pass to `delegate`, plus a `usage` summary — percent used, the window it resets in, and rate-limited/out-of-credits flags — so a choice between accounts can weigh budget, not just availability. `usage.known: false` means the provider has no usage source or none has been read yet, never a silent omission; set `usage: false` to skip the read entirely."
);

const INSPECT_DESCRIPTION: &str = concat!(
    "Get one task's record: output, scope, grant, spend, and completion. A watch line, or the alert block on the next Oga tool result, is the required trigger to call this before reporting the task's state, deciding what to do next, or ending a turn that tracks it; a line is not the record, and state read before this call is never \"still running\". ",
    "Read the worker's report and act; do not re-read changed files or re-run checks unless it names a broken, uncertain, or surprising item, or the sandbox could not run that check. ",
    "For a completed worktree task, the branch is the deliverable: review it, then push it and open a pull request from `~/.oga/worktrees/<taskId>`."
);

const HEALTH_DESCRIPTION: &str = "Check whether the Oga broker is running and read its broker and MCP contract versions, plus whether the source tree this build came from holds a newer one. For connection or compatibility diagnosis, not worker availability.";

const TASKS_DESCRIPTION: &str = "Find recent delegated tasks by state, a since/until time range, or a fan-out batch. With no arguments it covers tasks updated today (since local midnight) and each row is id, state, title, and cwd — plus originCwd, the project directory, when the task runs in its own checkout and cwd is inside it. Enough to recognise a task and call inspect on it directly. Any explicit since, until, parent, state, profile, or archived drops the today scoping and searches the full history under that filter. `fields` replaces the default row: `[\"label\"]` adds tldr, and `[\"routing\"]`, `[\"spend\"]`, `[\"completion\"]`, or `[\"all\"]` give more. Use inspect for one task in full.";

const MEMORY_DESCRIPTION: &str = "Read or update durable project facts shared across Oga callers and delegated workers; delegation ships the cwd's active memories automatically. Store decisions, constraints, and conventions, never secrets or transient task status. Use expectedVersion to prevent concurrent overwrites.";

const MAP_DESCRIPTION: &str = "Ask where a project's source files and top-level symbols live, without searching the tree. Oga builds the map from earlier tasks and re-verifies it against disk the moment you ask, so the answer is fresh — but it is an index, not a specification, so open a file before you act on an entry. The `options` selectors compose; globs are not supported; omit `options` or pass `{}` for the whole map. Refuses when map lookup is off. For a plain-language question ('where is X handled'), use `query`.";

const QUERY_DESCRIPTION: &str = "When you do not know which file or symbol to open, ask here before searching the tree — \"email driver\" can answer `src/adapters.ts#emailDriver`. Answers with one anchor when confident, a few candidates when not, or an honest miss telling you to search instead. Search directly when you need every match. The index can lag the code, so read the source it names before acting. For browsing a directory or an exact symbol or path, use `map`.";

const REPLY_DESCRIPTION: &str = "Answer a question from a task in needs_input state. Pass only its Oga task ID; Oga maps it to the private provider session and returns the same task ID. Optional scope is granted with the answer, replacing the task's scope and becoming the cwd's grant. After reply returns, start watch immediately in the background; its line, or the alert block on the next Oga tool result, is the trigger to call inspect before reporting state or deciding what to do next.";

const RESUME_DESCRIPTION: &str = concat!(
    "Continue a task on the same profile and provider session, keeping the same Oga task ID; when no session was captured, start a fresh one under that id. ",
    "This is the normal way to keep going on work a task already did, completed or not, rather than a recovery path for failures — a fresh delegation re-reads everything and loses why the work is the way it is. ",
    "Retry a failed, cancelled, blocked, or pending run with no instruction and it picks up where it stopped; a pending task force-starts, and a completed task needs an instruction for its follow-up. ",
    "An archived task is resumed in one call: it is unarchived and runs again, keeping its id and history. A worktree task whose checkout was removed with the archive gets it back on the same branch if the branch still exists; only when the branch is gone does the resume fail, naming the missing branch. ",
    "While a worker is still on the task, queue: \"add\" stacks the instruction onto the same session for Oga to send when that run finishes clean. ",
    "The finished run is not overwritten: it becomes an attempt carrying its own output, completion and session, so inspect with fields: [\"attempts\"] still shows what it produced. ",
    "Keep the instruction short — say only what changed and what to do next. The session still holds the goal, the guardrails, the scope and the reporting format from the first dispatch, so restating them wastes the run rather than steering it. ",
    "Optional scope and allowQuestions replace those task settings; get explicit approval before expanding scope. ",
    "Use reply instead when the task needs input, and handoff to move it to another model or profile — a rate-limited task parks itself and resumes once that account has usage again, at completion.resetsAt. ",
    "A session the provider can no longer reopen is not a dead end: resume starts a fresh session under the same task id, seeded with the original prompt and a summary of the prior run. ",
    "If the task never started behind a dependency, resume also restores its dependency-blocked dependents to waiting; they start when their prerequisites complete. ",
    "After resume returns, start `oga watch <taskId>` in the background; its settle line, or the alert block the next Oga tool result carries, is the required trigger to call inspect."
);

const STEER_DESCRIPTION: &str = "Send an instruction to a task that is still working, or switch its model mid-run, without stopping it or starting a new session; it keeps running and the change is recorded in its history. Providers without live control refuse the call and explain why; nothing is silently ignored.";

const HANDOFF_DESCRIPTION: &str = concat!(
    "Move a task to a different model, a different profile, or both, in one call, keeping the same Oga task ID — running or not, with no cancel first. ",
    "It preserves everything the destination can still use: a same-profile model change on a live worker switches in place with nothing restarted; otherwise on the same profile the old worker is stopped and the same provider session reopens under the new model, keeping all its context; a cross-profile move starts a fresh session seeded with a brief built from the previous run — a session belongs to one account and cannot be opened from another. ",
    "Use it whenever the destination changes: the current model is wrong for the work, or the account failed — rate limit, auth, billing, or a provider that will not take the work. ",
    "Use resume, not this, to continue or follow up on the same profile and model. ",
    "A rate-limited task is not dead — Oga parks it and resumes it at completion.resetsAt — so let it, hand off to a cheaper model on the same account, or hand off to a second account's quota for immediate recovery. ",
    "On a running task the old worker is stopped before the new one starts, so the two never overlap, and what it wrote stays on disk. ",
    "The task keeps its id, title, attempt history, and scope; a restarted run files the previous one as an attempt naming the profile, model, and session it ran as, so inspect with fields: [\"attempts\"] shows every run. ",
    "If the task waits on unfinished prerequisites, the move keeps it parked on a dependency hold instead of starting it; the response names what it waits on, and it starts on this destination when they complete. ",
    "State scope to approve a cross-profile destination and it becomes the cwd's grant; omit it and the task keeps the scope it already had — approved for the profile it is leaving, so the response warns about the new destination."
);

const CANCEL_DESCRIPTION: &str = "Stop a delegated task and its worker process tree. Works on queued, running, needs_input, and blocked tasks, so a task parked on a question you do not want to answer is not a dead end. The task record survives. A batch reports each id's outcome; one that cannot be cancelled — already settled, or unknown — does not fail the rest.";

const COMPLETE_DESCRIPTION: &str = concat!(
    "Mark a blocked or failed task completed on your word that the work demonstrably landed, when the worker never attested its own completion. ",
    "Use it only after you checked the deliverable yourself: the original completion survives on the record, and the override permanently carries who asserted it, why, and the code it replaced. ",
    "Rejected while a task is still running — asserting completion of work in flight is the worse mistake — and on one already completed or cancelled; resume or archive those instead."
);

const ARCHIVE_DESCRIPTION: &str = concat!(
    "Archive or restore a delegated task without deleting its history: it stays addressable by Oga task ID and drops out of active task lists. A task that is not settled is stopped first, so it is never hidden while its worker is alive — its entry carries `stopped: true` and its state reads `cancelled`. ",
    "Archiving a worktree task removes its checkout and keeps the branch the work is on; a checkout holding uncommitted work, or one that could not be removed, stays in place, the entry's `checkout` names the path, and the archive still succeeds. ",
    "A batch reports each id's outcome; one that cannot be archived does not fail the rest."
);

const DELETE_WORKTREE_DESCRIPTION: &str = concat!(
    "Delete a task's git worktree now — the checkout it ran in and any path it linked — without archiving the task or leaving it to cleanup. ",
    "Only a settled worktree task has a checkout to remove; one still running or holding a question keeps it. ",
    "A linked path is only unlinked, and what it pointed at is untouched. ",
    "Removing a checkout that is already gone is not an error; the result says what went and whether the branch survives. ",
    "Resuming an archived worktree task recreates its checkout on the same branch when the branch still exists."
);

const SCOPE_DESCRIPTION: &str = "Paths the worker may touch, relative to cwd: literal file paths, dir/** for a subtree, ** for the whole tree. The only place permissions belong — never restate them in the prompt, and never treat a grant as a reading plan.";
const WORKTREE_DESCRIPTION: &str = "Give the task its own checkout of the repository at cwd, on a branch of its own, so the worker commits there instead of in the user's working tree — or join a checkout another task already has. cwd must be inside a git repository with at least one commit. true takes every default for a new checkout; the object sets them, or names join to enter an existing one instead of making a new one. By default, ignored directories and .env* files are linked at any depth according to gitignore; editor and agent state is skipped. link overrides that list. Omit branch to use oga/<slug-of-title>; it falls back to oga/<taskId> when the title has no slug. The project map is not shipped to a worktree task. The worker commits there; the caller reviews and pushes the branch, then opens a pull request. Oga cleanup removes a checkout, never the branch, and never while another task still joins it.";
const DIFFICULTY_DESCRIPTION: &str = "How hard this work is, which decides the model and how much it thinks. mechanical: fully specified, just apply it. standard: a named target and a clear endpoint, the worker decides how. hard: the answer's shape is part of the work — design, root-cause, a cross-cutting refactor. critical: being wrong is expensive and hard to spot — security, concurrency, migrations, data loss. Omit it and the prompt decides.";
const EFFORT_DESCRIPTION: &str = "Reasoning effort for this run, when you want to set it yourself; left out, Oga reads it off difficulty and the model's own levels. Honoured by claude, codex, opencode, opencode-2 and pi; antigravity bakes its level into the model id.";
const ALLOW_QUESTIONS_DESCRIPTION: &str =
    "Whether the worker may pause in needs_input to ask. False makes it guess or stop.";
const TIMEOUT_DESCRIPTION: &str = "Hard runtime limit. The task lands in failed with code timeout.";
const DEPENDS_ON_DESCRIPTION: &str =
    "Prerequisite task ids: this task waits until every one completes, then starts on its own.";
const BLOCKER_FAILURE_DESCRIPTION: &str = "What a failed prerequisite does: \"hold\" (the default) blocks this task, and puts it back to waiting when that prerequisite is resumed. \"run\" starts it anyway.";
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
                    "Untracked paths, relative to cwd, the checkout is seeded from. Omit it: ignored directories and .env* files are seeded at any depth according to gitignore; .claude, .agents, .DS_Store, .plans, .malico and *.bun-build state is skipped. Pass it only to override that, and [] seeds nothing. Each path becomes the checkout's own directory rather than a symlink, so everything under it resolves inside the checkout and the original is never written through. A path that is missing or that git tracks is refused by name, and removing the checkout leaves the originals alone.",
                ),
            ),
            (
                "join".into(),
                described(
                    json!({ "type": "string", "minLength": 1 }),
                    "Oga task id whose checkout to enter instead of making a new one: same directory, same branch, no second copy of the tree. Use it to put a reviewer or a fixer in the exact tree another task already wrote to, reading or committing alongside it. Refused together with from, branch or link — the checkout named already decided all three. A task with write scope joining a checkout waits behind every other unsettled writer already in it (declare that with dependsOn, or the dispatch is refused); a task with no write scope may join and run alongside anything else there.",
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
                "Model id for that profile. Omit to use the profile's default.",
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
            "difficulty".into(),
            described(
                json!({ "type": "string", "enum": ["mechanical", "standard", "hard", "critical"] }),
                DIFFICULTY_DESCRIPTION,
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
            json!({ "type": "string", "minLength": 1, "maxLength": 60 }),
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
                        "Include each row's usage summary (remaining/limit, reset window, rate-limited/out-of-credits flags). Costs a network call per profile beyond the cache; set false to skip it.",
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
        "map",
        MAP_DESCRIPTION,
        object_schema(
            Map::from_iter([
                (
                    "cwd".into(),
                    described(
                        json!({ "type": "string", "minLength": 1 }),
                        "Absolute path of the project whose map is asked for.",
                    ),
                ),
                (
                    "options".into(),
                    described(object_schema(
                        Map::from_iter([
                            (
                                "path".into(),
                                described(
                                    json!({ "type": "array", "items": { "type": "string" } }),
                                    "An exact file or directory. A directory limits the map to that subtree. Repeatable.",
                                ),
                            ),
                            (
                                "symbol".into(),
                                described(
                                    json!({ "type": "array", "items": { "type": "string" } }),
                                    "An exact symbol name, or a prefix ending in `*`. Repeatable.",
                                ),
                            ),
                            (
                                "q".into(),
                                described(
                                    json!({ "type": "string", "minLength": 1 }),
                                    "Deprecated — use the `query` tool. A plain-language question, answered with a ranked anchor rather than a listing.",
                                ),
                            ),
                            (
                                "tier".into(),
                                described(
                                    json!({ "type": "string", "enum": ["full", "skeleton"] }),
                                    "full shows symbols; skeleton shows paths and line counts. Directory prefixes default to skeleton.",
                                ),
                            ),
                            (
                                "depth".into(),
                                described(
                                    json!({ "type": "integer", "minimum": 0, "maximum": MAX_SAFE_INTEGER }),
                                    "Maximum directory levels below each requested directory. Depth 0 keeps files directly inside it.",
                                ),
                            ),
                        ]),
                        &[],
                    ), "Optional map selection. Omit it or pass {} for the default map."),
                ),
            ]),
            &["cwd"],
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
                "Model for the continued run on this same profile. The session is the conversation and the model is per run, so the change keeps what the worker already read and decided. Omit to keep the task's model. Refused together with startAt or queue.",
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
                "Hold the resume instead of running it now. \"rate_limit\" re-arms a rate-limited task's automatic hold; the reset time is the hint, the account's live status is the release rule. Also accepts an ISO instant or a duration like \"45m\" / \"4h\". The task sits `pending`; resume it again without startAt to start it, or cancel to drop it.",
            ),
        ),
        (
            "queue".into(),
            described(
                json!({ "type": "string", "enum": ["add", "clear"] }),
                "\"add\" queues the instruction to run after the current run finishes clean. Instruction is required and nothing else is: timeoutMs, scope, allowQuestions, model, effort and startAt describe a run, not an instruction. Items run oldest first, and a run that fails, asks a question, or ends blocked leaves them untouched rather than sending more work into a session in trouble; continue that run and the rest follow. Cancelling the task discards them, \"clear\" drops them, and on a finished task \"add\" does nothing and the resume runs now.",
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
                        "Also delete the task's own branch. The branch survives by default.",
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
