//! What a caller can do next, given the state the task is now in.
//!
//! Every task action answers with these instead of spending a tool description
//! on advice that only applies once: the moves are read off this task's state,
//! its checkout, and how its last run ended.

use std::path::Path;

use oga_domain::{CompletionCode, HoldViewKind, Task, TaskState};
use serde_json::{Value, json};

/// The call the hints ride on. Actions that leave a task in the same shape
/// share one arm: what the caller can do next comes from the state, not from
/// which tool got it there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Move {
    /// delegate, resume, reply, handoff — a run is under way or waiting to be.
    Started,
    /// steer left the instruction waiting for the current run to finish.
    Queued,
    /// cancel, complete, archive-restore, inspect — read off the state.
    Settled { branch_gone: bool },
    /// archive — the task left the active lists; the branch may be unavailable.
    Archived { branch_gone: bool },
    /// worktree-remove — the checkout is gone; the branch may not be.
    CheckoutRemoved { branch_kept: bool },
}

pub fn next(task: &Task, action: Move) -> Vec<Value> {
    let mut hints = Vec::new();
    match action {
        Move::Started => started(task, &mut hints),
        Move::Queued => {
            hints.push(hint(
                "inspect",
                "the queued instruction runs when this run finishes clean",
            ));
            hints.push(hint("resume", "queue: \"clear\" drops what is waiting"));
            hints.push(hint(
                "cancel",
                "stops the run and discards what is queued behind it",
            ));
        }
        Move::Settled { branch_gone } => settled(task, &mut hints, branch_gone),
        Move::Archived { branch_gone } => {
            if !branch_gone {
                hints.push(hint(
                    "resume",
                    "unarchives and runs again, keeping the id and history",
                ));
            }
            if let Some(path) = checkout(task) {
                hints.push(hint(
                    "worktree-remove",
                    format!(
                        "removes the checkout at {path}; {}",
                        if branch_gone {
                            "the branch is unavailable"
                        } else {
                            "the branch stays"
                        }
                    ),
                ));
            }
        }
        Move::CheckoutRemoved { branch_kept } => {
            if let Some(branch) = task.effective_branch().filter(|_| branch_kept) {
                hints.push(hint(
                    "resume",
                    format!("recreates the checkout on {branch} and continues there"),
                ));
            }
            hints.push(hint(
                "archive",
                "drops the task out of active lists; its history stays",
            ));
        }
    }
    hints
}

/// The refusal for a task whose state cannot take a resume, with the tool that
/// can. Returns nothing when resume is the right call.
pub fn resume_refusal(task: &Task) -> Option<(String, Vec<Value>)> {
    let hint = match task.state {
        TaskState::Running => hint(
            "steer",
            "tells a running task something — delivered live, or queued for when the run finishes",
        ),
        TaskState::Queued | TaskState::Answered => hint(
            "steer",
            "leaves the instruction for the run that is about to start",
        ),
        TaskState::NeedsInput => hint("reply", "answers the question it is parked on"),
        _ => return None,
    };
    Some((
        format!(
            "task cannot be resumed from state {}: {}",
            task.state.as_str(),
            task.id
        ),
        vec![hint],
    ))
}

fn started(task: &Task, hints: &mut Vec<Value>) {
    if task.state == TaskState::Pending {
        if task
            .hold
            .as_ref()
            .is_some_and(|hold| hold.kind == HoldViewKind::ProfileAvailable)
        {
            hints.push(hint(
                "handoff",
                "another model or account takes it now instead of waiting",
            ));
        }
        hints.push(hint("resume", "starts it now instead of waiting"));
        hints.push(hint("cancel", "drops it before it starts"));
        return;
    }
    watch(task, hints);
    hints.push(hint(
        "steer",
        "tell the worker something while it works, without stopping it",
    ));
    hints.push(hint(
        "cancel",
        "stops the worker; the task can be resumed afterwards",
    ));
}

fn settled(task: &Task, hints: &mut Vec<Value>, branch_gone: bool) {
    match task.state {
        TaskState::Completed => {
            if !branch_gone {
                hints.push(hint(
                    "resume",
                    "follows up in the same session; an instruction is required",
                ));
            }
            if let Some(path) = checkout(task) {
                if !branch_gone && let Some(branch) = task.effective_branch() {
                    hints.push(hint(
                        "shell",
                        format!(
                            "push {branch} from {path} and open the pull request — the branch is the deliverable"
                        ),
                    ));
                }
                hints.push(hint(
                    "archive",
                    format!(
                        "removes the checkout at {path}; {}",
                        if branch_gone {
                            "the branch is unavailable"
                        } else {
                            "the branch stays"
                        }
                    ),
                ));
            }
        }
        TaskState::Failed | TaskState::Cancelled | TaskState::Blocked => {
            if let Some(resets_at) = rate_limited(task) {
                hints.push(hint(
                    "handoff",
                    "another model or account picks it up right now",
                ));
                if !branch_gone {
                    hints.push(hint(
                        "resume",
                        match resets_at {
                            Some(instant) => format!(
                                "startAt: \"rate_limit\" waits until {instant} and runs it for free"
                            ),
                            None => {
                                "startAt: \"rate_limit\" waits until the account has usage again"
                                    .into()
                            }
                        },
                    ));
                }
            } else if !branch_gone {
                hints.push(hint(
                    "resume",
                    "picks up where it stopped; add an instruction to change course",
                ));
                hints.push(hint("handoff", "same task on another model or profile"));
            }
            if !branch_gone && suggested_scope(task) {
                hints.push(hint(
                    "resume",
                    "pass the suggestedScope it named to approve the paths it was denied",
                ));
            }
            if let Some(path) = checkout(task) {
                hints.push(hint(
                    "archive",
                    format!(
                        "removes the checkout at {path}; {}",
                        if branch_gone {
                            "the branch is unavailable"
                        } else {
                            "the branch stays"
                        }
                    ),
                ));
            }
        }
        TaskState::NeedsInput => {
            hints.push(hint(
                "reply",
                "answer the question; the session still holds the brief",
            ));
            hints.push(hint(
                "cancel",
                "stops it when the question is not worth answering",
            ));
        }
        TaskState::Pending | TaskState::Queued | TaskState::Running | TaskState::Answered => {
            started(task, hints);
        }
    }
}

fn watch(task: &Task, hints: &mut Vec<Value>) {
    hints.push(hint(
        "shell",
        format!(
            "background `oga watch {}` to be told when it settles",
            task.id
        ),
    ));
    hints.push(hint(
        "inspect",
        "read the record once watch settles, before reporting the task's state",
    ));
}

/// The checkout path, when the task ran in one and it is still on disk.
fn checkout(task: &Task) -> Option<&str> {
    task.worktree
        .as_ref()
        .map(|worktree| worktree.path.as_str())
        .filter(|path| Path::new(path).exists())
}

/// `Some(reset time)` when the account, not the work, is what stopped the run.
fn rate_limited(task: &Task) -> Option<Option<&str>> {
    task.completion
        .as_ref()
        .filter(|completion| completion.code == CompletionCode::RateLimit)
        .map(|completion| completion.resets_at.as_deref())
}

fn suggested_scope(task: &Task) -> bool {
    task.completion
        .as_ref()
        .is_some_and(|completion| completion.suggested_scope.is_some())
}

fn hint(tool: &str, when: impl Into<String>) -> Value {
    json!({ "tool": tool, "when": when.into() })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(state: TaskState) -> Task {
        Task {
            id: "task-1".into(),
            state,
            ..Task::default()
        }
    }

    #[test]
    fn a_cancelled_task_with_a_checkout_offers_archive_by_path() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().display().to_string();
        let mut cancelled = task(TaskState::Cancelled);
        cancelled.worktree = Some(oga_domain::TaskWorktree {
            origin_cwd: "/project".into(),
            path: path.clone(),
            branch: "oga/thing".into(),
            links: None,
        });
        let hints = next(&cancelled, Move::Settled { branch_gone: false });
        let tools = hints
            .iter()
            .map(|hint| hint["tool"].as_str().expect("tool"))
            .collect::<Vec<_>>();
        assert_eq!(tools, ["resume", "handoff", "archive"]);
        assert_eq!(
            hints[2]["when"],
            json!(format!("removes the checkout at {path}; the branch stays"))
        );
    }

    #[test]
    fn a_cancelled_task_without_a_checkout_never_mentions_one() {
        let hints = next(
            &task(TaskState::Cancelled),
            Move::Settled { branch_gone: false },
        );
        assert!(hints.iter().all(|hint| hint["tool"] != json!("archive")));
    }

    #[test]
    fn an_archived_task_with_a_gone_branch_never_offers_resume() {
        let hints = next(
            &task(TaskState::Completed),
            Move::Archived { branch_gone: true },
        );
        assert!(hints.iter().all(|hint| hint["tool"] != json!("resume")));
    }

    #[test]
    fn a_settled_task_with_a_gone_branch_never_offers_resume() {
        let hints = next(
            &task(TaskState::Completed),
            Move::Settled { branch_gone: true },
        );
        assert!(hints.iter().all(|hint| hint["tool"] != json!("resume")));
    }

    #[test]
    fn resume_on_a_running_task_is_refused_and_names_steer() {
        let (message, hints) = resume_refusal(&task(TaskState::Running)).expect("refusal");
        assert!(message.contains("state running"), "{message}");
        assert_eq!(hints.len(), 1);
        assert_eq!(hints[0]["tool"], json!("steer"));
    }

    #[test]
    fn resume_on_a_settled_task_is_not_refused() {
        for state in [
            TaskState::Failed,
            TaskState::Cancelled,
            TaskState::Blocked,
            TaskState::Completed,
            TaskState::Pending,
        ] {
            assert!(resume_refusal(&task(state)).is_none(), "{state:?}");
        }
    }

    #[test]
    fn a_rate_limited_ending_offers_the_account_moves() {
        let mut failed = task(TaskState::Failed);
        failed.completion = Some(oga_domain::TaskCompletion {
            exit_code: None,
            blocked: true,
            code: CompletionCode::RateLimit,
            reason: None,
            suggested_scope: None,
            resets_at: Some("2026-09-05T12:00:00.000Z".into()),
            asserted_completion: None,
            dependency_blocked: None,
        });
        let hints = next(&failed, Move::Settled { branch_gone: false });
        assert_eq!(hints[0]["tool"], json!("handoff"));
        assert!(
            hints[1]["when"]
                .as_str()
                .expect("when")
                .contains("2026-09-05T12:00:00.000Z")
        );
    }
}
