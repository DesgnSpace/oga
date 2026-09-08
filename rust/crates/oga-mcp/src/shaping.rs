//! Public task response shaping shared by MCP tools.

use oga_domain::{BranchOutcome, Task, TaskMatch, TaskSummary};
use serde_json::{Map, Value, json};

use crate::hints;

const GROUPS: &[(&str, &[&str])] = &[
    ("routing", &["profileId", "model", "effort", "effortActual"]),
    (
        "context",
        &[
            "cwd",
            "branch",
            "worktree",
            "worktreeLabel",
            "createdAt",
            "updatedAt",
            "durationMs",
            "title",
            "tldr",
            "parentTaskId",
            "orchestratorId",
        ],
    ),
    ("label", &["title", "tldr"]),
    // Where the task runs: the project directory, and the checkout it was
    // given instead when it was delegated with a worktree.
    ("location", &["cwd", "worktree"]),
    (
        "scope",
        &["scope", "grantId", "allowQuestions", "timeoutMs"],
    ),
    ("prompt", &["prompt"]),
    ("shippedPrompt", &["shippedPrompt"]),
    ("output", &["output"]),
    ("attempts", &["attempts"]),
    ("completion", &["completion", "error", "question"]),
    ("spend", &["costUsd", "turns"]),
];

pub fn task_view(task: &Task, fields: &[String]) -> Value {
    let want = wanted_fields(fields);
    let mut view = Map::from_iter([
        ("id".into(), json!(task.id)),
        ("state".into(), json!(task.state)),
    ]);
    if !task.attempts.is_empty() {
        view.insert("attemptCount".into(), json!(task.attempts.len()));
    }
    if task.queued_follow_ups.is_some_and(|count| count > 0) {
        view.insert(
            "queuedFollowUps".into(),
            json!(task.queued_follow_ups.expect("count is present")),
        );
    }
    if let Some(archived_at) = &task.archived_at {
        view.insert("archivedAt".into(), json!(archived_at));
    }
    if let Some(hold) = &task.hold {
        view.insert("hold".into(), json!(hold));
    }
    if !task.attachments.is_empty() {
        view.insert("attachments".into(), json!(task.attachments));
    }

    if want.contains("profileId") {
        view.insert("profileId".into(), json!(task.profile_id));
    }
    if want.contains("model") {
        view.insert("model".into(), json!(task.model));
    }
    if want.contains("effort")
        && let Some(effort) = &task.effort
    {
        view.insert("effort".into(), json!(effort));
    }
    if want.contains("effortActual")
        && let Some(effort) = &task.effort_actual
    {
        view.insert("effortActual".into(), json!(effort));
    }

    if want.contains("cwd") {
        view.insert("cwd".into(), json!(task.cwd));
    }
    if want.contains("branch")
        && let Some(branch) = task.effective_branch()
    {
        view.insert("branch".into(), json!(branch));
    }
    if want.contains("worktree")
        && let Some(worktree) = &task.worktree
    {
        view.insert("worktree".into(), json!(worktree));
    }
    if want.contains("worktreeLabel")
        && let Some(label) = &task.worktree_label
    {
        view.insert("worktreeLabel".into(), json!(label));
    }
    if want.contains("createdAt") {
        view.insert("createdAt".into(), json!(task.created_at));
    }
    if want.contains("updatedAt") {
        view.insert("updatedAt".into(), json!(task.updated_at));
    }
    if want.contains("durationMs") {
        view.insert("durationMs".into(), json!(task.duration_ms));
    }
    if want.contains("title")
        && let Some(title) = &task.title
    {
        view.insert("title".into(), json!(title));
    }
    if want.contains("tldr")
        && let Some(tldr) = &task.tldr
    {
        view.insert("tldr".into(), json!(tldr));
    }
    if want.contains("parentTaskId")
        && let Some(parent) = &task.parent_task_id
    {
        view.insert("parentTaskId".into(), json!(parent));
    }
    if want.contains("orchestratorId")
        && let Some(orchestrator) = &task.orchestrator_id
    {
        view.insert("orchestratorId".into(), json!(orchestrator));
    }

    if want.contains("scope") {
        view.insert("scope".into(), json!(task.scope));
    }
    if want.contains("grantId")
        && let Some(grant) = &task.grant_id
    {
        view.insert("grantId".into(), json!(grant));
    }
    if want.contains("allowQuestions") {
        view.insert("allowQuestions".into(), json!(task.allow_questions));
    }
    if want.contains("timeoutMs")
        && let Some(timeout) = task.timeout_ms
    {
        view.insert("timeoutMs".into(), json!(timeout));
    }

    if want.contains("prompt") {
        view.insert("prompt".into(), json!(task.prompt));
    }
    if want.contains("shippedPrompt")
        && let Some(prompt) = &task.shipped_prompt
    {
        view.insert("shippedPrompt".into(), json!(prompt));
    }
    if want.contains("output") {
        view.insert("output".into(), json!(task.output));
    }
    if want.contains("attempts") && !task.attempts.is_empty() {
        let attempts = task
            .attempts
            .iter()
            .map(|attempt| {
                let mut value =
                    serde_json::to_value(attempt).expect("task attempt is serializable");
                if let Some(object) = value.as_object_mut() {
                    object.remove("sessionId");
                }
                value
            })
            .collect::<Vec<_>>();
        view.insert("attempts".into(), Value::Array(attempts));
    }
    if want.contains("completion")
        && let Some(completion) = &task.completion
    {
        view.insert("completion".into(), json!(completion));
    }
    if want.contains("error")
        && let Some(error) = &task.error
    {
        view.insert("error".into(), json!(error));
    }
    if want.contains("question")
        && let Some(question) = &task.question
    {
        view.insert("question".into(), json!(question));
    }
    if want.contains("costUsd")
        && let Some(cost) = task.cost_usd
    {
        view.insert("costUsd".into(), json!(cost));
    }
    if want.contains("turns")
        && let Some(turns) = task.turns
    {
        view.insert("turns".into(), json!(turns));
    }
    Value::Object(view)
}

pub fn summary_view(
    summary: &TaskSummary,
    fields: Option<&[String]>,
    matched: Option<TaskMatch>,
) -> Value {
    let default = fields.is_none();
    let default_fields = [String::from("label"), String::from("location")];
    let want = wanted_fields(fields.unwrap_or(&default_fields));
    let mut view = Map::from_iter([
        ("id".into(), json!(summary.id)),
        ("state".into(), json!(summary.state)),
    ]);
    if let Some(archived_at) = &summary.archived_at {
        view.insert("archivedAt".into(), json!(archived_at));
    }
    if let Some(hold) = &summary.hold {
        view.insert("hold".into(), json!(hold));
    }
    if let Some(matched) = matched {
        view.insert("match".into(), json!(matched));
    }
    if want.contains("profileId") {
        view.insert("profileId".into(), json!(summary.profile_id));
    }
    if want.contains("model") {
        view.insert("model".into(), json!(summary.model));
    }
    if want.contains("cwd") {
        view.insert("cwd".into(), json!(summary.cwd));
    }
    // The summary has no checkout path of its own: `cwd` already sits inside
    // it, and the project the work belongs to is the half that goes missing.
    if want.contains("worktree")
        && let Some(origin_cwd) = &summary.origin_cwd
    {
        view.insert("originCwd".into(), json!(origin_cwd));
    }
    if want.contains("branch")
        && let Some(branch) = &summary.branch
    {
        view.insert("branch".into(), json!(branch));
    }
    if want.contains("worktreeLabel")
        && let Some(label) = &summary.worktree_label
    {
        view.insert("worktreeLabel".into(), json!(label));
    }
    if want.contains("createdAt") {
        view.insert("createdAt".into(), json!(summary.created_at));
    }
    if want.contains("updatedAt") {
        view.insert("updatedAt".into(), json!(summary.updated_at));
    }
    if want.contains("durationMs") {
        view.insert("durationMs".into(), json!(summary.duration_ms));
    }
    if want.contains("title")
        && let Some(title) = &summary.title
    {
        view.insert("title".into(), json!(title));
    }
    if want.contains("tldr")
        && !default
        && let Some(tldr) = &summary.tldr
    {
        view.insert("tldr".into(), json!(tldr));
    }
    if want.contains("parentTaskId")
        && let Some(parent) = &summary.parent_task_id
    {
        view.insert("parentTaskId".into(), json!(parent));
    }
    if want.contains("grantId")
        && let Some(grant) = &summary.grant_id
    {
        view.insert("grantId".into(), json!(grant));
    }
    if want.contains("prompt") {
        view.insert("promptPreview".into(), json!(summary.prompt_preview));
    }
    if want.contains("completion")
        && let Some(completion) = &summary.completion
    {
        view.insert("completion".into(), json!(completion));
    }
    if want.contains("error")
        && let Some(error) = &summary.error
    {
        view.insert("error".into(), json!(error));
    }
    if want.contains("question")
        && let Some(question) = &summary.question
    {
        view.insert("question".into(), json!(question));
    }
    if want.contains("costUsd")
        && let Some(cost) = summary.cost_usd
    {
        view.insert("costUsd".into(), json!(cost));
    }
    Value::Object(view)
}

pub fn task_summary(task: &Task) -> TaskSummary {
    TaskSummary {
        id: task.id.clone(),
        profile_id: task.profile_id.clone(),
        model: task.model.clone(),
        cwd: task.cwd.clone(),
        origin_cwd: task
            .worktree
            .as_ref()
            .map(|worktree| worktree.origin_cwd.clone()),
        branch: task.effective_branch().map(str::to_owned),
        worktree_label: task.worktree_label.clone(),
        state: task.state,
        prompt_preview: task.prompt.chars().take(200).collect(),
        tldr: task.tldr.clone(),
        title: task.title.clone(),
        created_at: task.created_at.clone(),
        updated_at: task.updated_at.clone(),
        duration_ms: task.duration_ms,
        running_since: task.running_since.clone(),
        error: task.error.clone(),
        question: task.question.clone(),
        parent_task_id: task.parent_task_id.clone(),
        orchestrator_id: task.orchestrator_id.clone(),
        grant_id: task.grant_id.clone(),
        session_id: None,
        completion: task.completion.clone(),
        cost_usd: task.cost_usd,
        cost_usd_estimated: task.cost_usd_estimated,
        archived_at: task.archived_at.clone(),
        hold: task.hold.clone(),
    }
}

pub fn wanted_fields(fields: &[String]) -> std::collections::BTreeSet<&'static str> {
    let mut result = std::collections::BTreeSet::new();
    for field in fields {
        if field == "all" {
            for (_, members) in GROUPS {
                result.extend(members.iter().copied());
            }
            continue;
        }
        if let Some((_, members)) = GROUPS.iter().find(|(name, _)| *name == field) {
            result.extend(members.iter().copied());
        }
    }
    result
}

pub fn default_inspect_fields() -> Vec<String> {
    [
        "routing",
        "context",
        "label",
        "location",
        "scope",
        "output",
        "completion",
        "spend",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}

/// Adds the moves that fit the task's current state. Nothing is added when
/// nothing applies, so an empty `next` never has to be read.
pub fn with_next(mut value: Value, task: &Task, action: hints::Move) -> Value {
    let next = hints::next(task, action);
    if !next.is_empty()
        && let Some(object) = value.as_object_mut()
    {
        object.insert("next".into(), Value::Array(next));
    }
    value
}

pub(crate) struct TaskActionSuccess {
    pub(crate) task: Task,
    pub(crate) stopped: Option<String>,
    pub(crate) checkout: Option<String>,
    pub(crate) branch_outcome: Option<BranchOutcome>,
    pub(crate) branch_reason: Option<String>,
    pub(crate) branch_gone: bool,
}

type TaskActionOutcome = Result<TaskActionSuccess, (String, String)>;

pub fn task_action_response(
    ids: &[String],
    outcomes: Vec<TaskActionOutcome>,
    fields: &[String],
    action: hints::Move,
) -> Value {
    let entries = outcomes
        .into_iter()
        .map(|outcome| match outcome {
            Ok(TaskActionSuccess {
                task,
                stopped,
                checkout,
                branch_outcome,
                branch_reason,
                branch_gone,
            }) => {
                let branch_gone = branch_gone
                    || branch_outcome.is_some_and(|outcome| {
                        matches!(outcome, BranchOutcome::Deleted | BranchOutcome::AlreadyGone)
                    });
                let mut value = task_view(&task, fields);
                if let Some(object) = value.as_object_mut() {
                    if stopped.is_some() {
                        object.insert("stopped".into(), json!(true));
                    }
                    if let Some(checkout) = checkout {
                        object.insert("checkout".into(), json!(checkout));
                    }
                    if let Some(branch) = branch_outcome {
                        object.insert("branchOutcome".into(), json!(branch));
                    }
                    if let Some(reason) = branch_reason {
                        object.insert("branchReason".into(), json!(reason));
                    }
                }
                let action = match action {
                    hints::Move::Archived { .. } => hints::Move::Archived { branch_gone },
                    hints::Move::Settled { .. } => hints::Move::Settled { branch_gone },
                    action => action,
                };
                with_next(value, &task, action)
            }
            Err((id, error)) => json!({ "id": id, "error": error }),
        })
        .collect::<Vec<_>>();
    if ids.len() == 1 {
        entries
            .into_iter()
            .next()
            .unwrap_or_else(|| json!({ "id": ids[0] }))
    } else {
        Value::Array(entries)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oga_domain::TaskState;

    #[test]
    fn branch_outcome_does_not_replace_the_task_branch() {
        let task = Task {
            id: "task".into(),
            state: TaskState::Completed,
            branch: Some("oga/ship-it".into()),
            ..Default::default()
        };
        let task_id = task.id.clone();

        let value = task_action_response(
            std::slice::from_ref(&task_id),
            vec![Ok(TaskActionSuccess {
                task,
                stopped: None,
                checkout: None,
                branch_outcome: Some(BranchOutcome::Kept),
                branch_reason: Some("branch has unmerged commits".into()),
                branch_gone: false,
            })],
            &["context".into()],
            hints::Move::Settled { branch_gone: false },
        );

        assert_eq!(value["branch"], "oga/ship-it");
        assert_eq!(value["branchOutcome"], "kept");
    }

    #[test]
    fn deleted_branch_does_not_offer_resume() {
        let task = Task {
            id: "task".into(),
            state: TaskState::Completed,
            ..Default::default()
        };
        let task_id = task.id.clone();

        let value = task_action_response(
            std::slice::from_ref(&task_id),
            vec![Ok(TaskActionSuccess {
                task,
                stopped: None,
                checkout: None,
                branch_outcome: Some(BranchOutcome::Deleted),
                branch_reason: None,
                branch_gone: false,
            })],
            &[],
            hints::Move::Archived { branch_gone: false },
        );

        assert!(
            value["next"]
                .as_array()
                .is_none_or(|next| { next.iter().all(|hint| hint["tool"] != "resume") })
        );
    }
}
