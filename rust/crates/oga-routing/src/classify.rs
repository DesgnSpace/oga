//! Task classification: what a prompt reads like, and what that costs by
//! default.

use std::sync::LazyLock;

use oga_domain::{Difficulty, TaskClass};
use regex::Regex;

/// What the prompt heuristic made of the work.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskDemand {
    pub task_class: TaskClass,
    pub difficulty: Difficulty,
    pub reason: String,
}

/// Cheapest first, which is also how a tie resolves: an ambiguous prompt buys
/// a cheap retry rather than an over-priced success.
pub const CLASS_ORDER: [TaskClass; 5] = [
    TaskClass::Mechanical,
    TaskClass::General,
    TaskClass::Build,
    TaskClass::Context,
    TaskClass::Reasoning,
];

fn class_difficulty(task_class: TaskClass) -> Difficulty {
    match task_class {
        TaskClass::Mechanical => Difficulty::Mechanical,
        TaskClass::General | TaskClass::Build => Difficulty::Standard,
        TaskClass::Context => Difficulty::Hard,
        // The class is a read of how open the answer's shape looks, so it
        // stands in for the judgment the caller did not offer.
        TaskClass::Reasoning => Difficulty::Critical,
    }
}

fn class_reason(task_class: TaskClass) -> &'static str {
    match task_class {
        TaskClass::Mechanical => "bounded mechanical work",
        TaskClass::General => "general bounded work",
        TaskClass::Build => "implementation against a stated shape",
        TaskClass::Context => "cross-file context comprehension",
        TaskClass::Reasoning => "deep judgment or failure analysis",
    }
}

/// What each class reads like, weighted. Decisive phrases count double; a word
/// that merely tends to appear counts once. `general` has no signals — it is
/// what a prompt matching nothing lands on.
static CLASS_SIGNALS: LazyLock<Vec<(TaskClass, i32, Regex)>> = LazyLock::new(|| {
    vec![
        (TaskClass::Mechanical, 1, regex(r"\brenam(?:e|ing)\b")),
        (
            TaskClass::Mechanical,
            1,
            regex(r"\b(?:find|search) and replace\b"),
        ),
        (
            TaskClass::Mechanical,
            1,
            regex(r"\b(?:lint|reformat|formatting)\b"),
        ),
        (TaskClass::Mechanical, 1, regex(r"\bcommit message\b")),
        (
            TaskClass::Mechanical,
            1,
            regex(r"\bapply (?:this|the) (?:diff|patch)\b"),
        ),
        (TaskClass::Mechanical, 1, regex(r"\bmechanical\b")),
        (TaskClass::Mechanical, 1, regex(r"\blist (?:the )?files\b")),
        (TaskClass::Mechanical, 1, regex(r"\bsummari[sz]e this\b")),
        // Probes and verification passes: the judgment went into writing the
        // checklist, and running it against the output is bounded work.
        (TaskClass::Mechanical, 2, regex(r"\bprobe\b")),
        (TaskClass::Mechanical, 1, regex(r"\bverif(?:y|ication)\b")),
        (
            TaskClass::Mechanical,
            1,
            regex(r"\b(?:check|confirm) (?:whether|that)\b"),
        ),
        (TaskClass::Mechanical, 1, regex(r"\bchecklist\b")),
        (
            TaskClass::Mechanical,
            1,
            regex(r"\brun (?:the |these |each )?(?:named )?commands?\b"),
        ),
        (
            TaskClass::Mechanical,
            1,
            regex(r"\breport what you (?:saw|see|observed)\b"),
        ),
        (TaskClass::Build, 1, regex(r"\bimplement\b")),
        (TaskClass::Build, 1, regex(r"\bbuild\b")),
        (TaskClass::Build, 1, regex(r"\bfix\b")),
        (TaskClass::Build, 1, regex(r"\bdebug\b")),
        (TaskClass::Build, 1, regex(r"\brefactor\b")),
        (
            TaskClass::Build,
            1,
            regex(r"\b(?:write|add) (?:a |the )?tests?\b"),
        ),
        (TaskClass::Build, 1, regex(r"\badd (?:a |the )?feature\b")),
        (TaskClass::Context, 1, regex(r"\bunderstand\b")),
        (TaskClass::Context, 1, regex(r"\btrace\b")),
        (TaskClass::Context, 1, regex(r"\binvestigate\b")),
        (TaskClass::Context, 1, regex(r"\breview\b")),
        (TaskClass::Context, 1, regex(r"\baudit\b")),
        (TaskClass::Context, 1, regex(r"\banaly[sz]e\b")),
        (TaskClass::Context, 1, regex(r"\bsurvey\b")),
        (
            TaskClass::Context,
            1,
            regex(r"\bread (?:the )?(?:codebase|source|call sites)\b"),
        ),
        (TaskClass::Context, 1, regex(r"\bhow [^.\n]{1,60} works\b")),
        (TaskClass::Context, 1, regex(r"\bwhy [^.\n]{1,60} fail")),
        (TaskClass::Reasoning, 2, regex(r"\barchitect(?:ure|ing)?\b")),
        (TaskClass::Reasoning, 2, regex(r"\bsystem design\b")),
        (TaskClass::Reasoning, 2, regex(r"\broot[- ]cause\b")),
        (TaskClass::Reasoning, 2, regex(r"\bthreat model\b")),
        (TaskClass::Reasoning, 2, regex(r"\brace conditions?\b")),
        (TaskClass::Reasoning, 2, regex(r"\bconcurren")),
        (TaskClass::Reasoning, 2, regex(r"\btrade-?offs?\b")),
        (TaskClass::Reasoning, 1, regex(r"\bsecurity\b")),
        (TaskClass::Reasoning, 1, regex(r"\bmigration\b")),
        (TaskClass::Reasoning, 1, regex(r"\bhard research\b")),
    ]
});

fn regex(pattern: &str) -> Regex {
    Regex::new(pattern).expect("static routing pattern")
}

/// Every matching signal adds weight and the heaviest class wins, because
/// first-match over unanchored words picks whichever class is tested first
/// once a brief is long enough to contain all of them.
pub fn classify_task(prompt: &str) -> TaskDemand {
    let text = prompt.to_lowercase();
    let mut scores = [0i32; 5];
    for (task_class, weight, pattern) in CLASS_SIGNALS.iter() {
        if pattern.is_match(&text) {
            scores[class_index(*task_class)] += weight;
        }
    }
    let task_class = CLASS_ORDER
        .into_iter()
        .fold(TaskClass::General, |best, candidate| {
            if scores[class_index(candidate)] > scores[class_index(best)] {
                candidate
            } else {
                best
            }
        });
    TaskDemand {
        task_class,
        difficulty: class_difficulty(task_class),
        reason: class_reason(task_class).to_owned(),
    }
}

fn class_index(task_class: TaskClass) -> usize {
    CLASS_ORDER
        .iter()
        .position(|c| *c == task_class)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn class(prompt: &str) -> TaskClass {
        classify_task(prompt).task_class
    }

    #[test]
    fn bounded_phrases_read_as_mechanical() {
        assert_eq!(
            class("Rename this symbol across the package."),
            TaskClass::Mechanical
        );
        assert_eq!(
            class("Run these commands and report what you saw."),
            TaskClass::Mechanical
        );
        assert_eq!(
            class("Write a commit message for this diff."),
            TaskClass::Mechanical
        );
        assert_eq!(class("Apply this diff to the tree."), TaskClass::Mechanical);
    }

    #[test]
    fn implementation_and_comprehension_read_differently() {
        assert_eq!(
            class("Implement the feature described in the plan."),
            TaskClass::Build
        );
        assert_eq!(
            class("Review this codebase and explain how auth works."),
            TaskClass::Context
        );
    }

    #[test]
    fn decisive_reasoning_phrases_outweigh_context_words() {
        assert_eq!(
            class("Architect a secure migration and analyze race conditions."),
            TaskClass::Reasoning
        );
        assert_eq!(
            class("Root-cause the race condition."),
            TaskClass::Reasoning
        );
    }

    #[test]
    fn a_prompt_matching_nothing_is_general_work() {
        assert_eq!(
            class("Draft the release notes from this changelog."),
            TaskClass::General
        );
    }

    #[test]
    fn ties_resolve_to_the_cheapest_matching_class() {
        assert_eq!(classify_task("fix").difficulty, Difficulty::Standard);
        assert_eq!(classify_task("rename").difficulty, Difficulty::Mechanical);
    }
}
