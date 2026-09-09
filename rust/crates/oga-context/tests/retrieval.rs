//! The retrieval corpus: the lookups an agent has to be able to trust,
//! grounded in this repository and in projects written for the cases this
//! repository cannot show.
//!
//! A wrong first answer is worse than no answer — it sends the reader to code
//! that has nothing to do with the question — so every row fixes where its
//! answer has to appear, not merely that it appears.

use std::fs;
use std::path::{Path, PathBuf};

use oga_context::{BuildOptions, ContextIndex, ContextTarget, LearnRouteProposal, QuestionOptions};
use oga_domain::{Task, TaskScope, TaskWorktree};
use oga_store::Store;
use tempfile::{TempDir, tempdir};

/// One corpus row: what someone types, the anchors that answer it, and how
/// far down the list every one of those anchors may be.
struct Case {
    question: &'static str,
    want: &'static [&'static str],
    within: usize,
}

/// Questions answered by this repository's own source.
const REPOSITORY: &[Case] = &[
    // A path someone copied out of a stack trace or a review.
    Case {
        question: "rust/crates/oga-context/src/query.rs",
        want: &["rust/crates/oga-context/src/query.rs"],
        within: 1,
    },
    Case {
        question: "rust/crates/oga-context/src/query.rs#is_confident",
        want: &["rust/crates/oga-context/src/query.rs#is_confident"],
        within: 1,
    },
    // Answers carry a line, so pasting one back has to resolve whatever the
    // line says — it is where to land, not part of the lookup.
    Case {
        question: "rust/crates/oga-context/src/routes.rs:200",
        want: &["rust/crates/oga-context/src/routes.rs"],
        within: 1,
    },
    Case {
        question: "rust/crates/oga-context/src/query.rs:120#direct_target",
        want: &["rust/crates/oga-context/src/query.rs#direct_target"],
        within: 1,
    },
    // A name that only reads as a name with what encloses it.
    Case {
        question: "rust/crates/oga-context/src/query.rs#TermWeights::of",
        want: &["rust/crates/oga-context/src/query.rs#of"],
        within: 1,
    },
    Case {
        question: "rust/Cargo.toml#workspace.package.version",
        want: &["rust/Cargo.toml#version"],
        within: 1,
    },
    // The end of a path, which is all anyone remembers of a long one.
    Case {
        question: "screens/sidebar/Sidebar.tsx",
        want: &["web/src/screens/sidebar/Sidebar.tsx"],
        within: 1,
    },
    // Two letters is a whole name in real projects.
    Case {
        question: "pi",
        want: &["rust/crates/oga-runner/src/bin/fake-provider.rs#pi"],
        within: 1,
    },
    // One name, two definitions: the answer is both of them.
    Case {
        question: "Io",
        want: &[
            "rust/crates/oga-config/src/lib.rs#Io",
            "rust/crates/oga-store/src/connection.rs#Io",
        ],
        within: 3,
    },
    // Nobody types the name; they describe what it does.
    Case {
        question: "cap on how many aliases a route keeps",
        want: &["rust/crates/oga-context/src/routes.rs#MAX_ROUTE_ALIASES"],
        within: 1,
    },
];

/// Questions the ranking still gets wrong. Recorded rather than asserted:
/// this is the measured state of paraphrase retrieval, and the next change to
/// ranking is meant to move it.
const OPEN: &[Case] = &[
    Case {
        question: "the most files one project may contribute",
        want: &["rust/crates/oga-context/src/index.rs#MAX_BUILD_FILES"],
        within: 1,
    },
    Case {
        question: "where are search results ranked",
        want: &["rust/crates/oga-context/src/index.rs#rank"],
        within: 3,
    },
];

#[test]
fn answers_named_places_and_paraphrases_from_this_repository() {
    let repository = repository_root();
    let (_database, store) = fixture_store();
    let index = ContextIndex::new(&store);
    index
        .build(&repository, BuildOptions::default())
        .expect("repository index builds");
    let target = ContextTarget::new(&repository, readable());

    let mut report = Vec::new();
    let mut misses = 0;
    for case in REPOSITORY.iter().chain(OPEN) {
        let answer = index
            .question_with_options(&target, case.question, QuestionOptions::default())
            .expect("corpus question answers");
        let found = anchors(&answer);
        let hit = case
            .want
            .iter()
            .all(|want| found.iter().take(case.within).any(|got| got == want));
        misses += usize::from(!hit && REPOSITORY.iter().any(|row| row.question == case.question));
        report.push(format!(
            "  [{}] {:?}\n    want {} in the top {}\n    got  {}",
            if hit { "hit " } else { "miss" },
            case.question,
            case.want.join(" and "),
            case.within,
            top(&found),
        ));
    }
    let report = report.join("\n");
    println!("{report}");
    assert_eq!(
        misses,
        0,
        "{misses} of {} corpus lookups missed:\n{report}",
        REPOSITORY.len()
    );
}

/// The route taught this phrase wins over one that merely shares its words,
/// whatever order either was taught in or asked in.
#[test]
fn the_route_taught_this_phrase_wins_over_one_that_overlaps_it() {
    let project = Project::new();
    project.write(
        "src/auth.ts",
        "export function checkAuth() { return true; }\n",
    );
    project.write("src/other.ts", "export const other = true;\n");
    project.build();
    project.teach(&["front", "door"], "src/other.ts", Some("other"));
    project.teach(&["bell", "door", "front"], "src/auth.ts", Some("checkAuth"));

    for question in ["door front", "front door"] {
        assert_eq!(
            project.top(question),
            "src/other.ts#other",
            "{question:?} is what one of these routes was taught"
        );
    }
}

/// A hint from one of the interchangeable word families is still matched word
/// for word, not lost among the family it was widened to.
#[test]
fn a_hint_widened_to_its_synonyms_still_matches_word_for_word() {
    let project = Project::new();
    project.write(
        "src/auth.ts",
        "export function checkAuth() { return true; }\n",
    );
    project.write("src/other.ts", "export const other = true;\n");
    project.build();
    project.teach(&["auth"], "src/other.ts", Some("other"));
    project.teach(&["auth", "check"], "src/auth.ts", Some("checkAuth"));

    assert_eq!(project.top("auth"), "src/other.ts#other");
}

/// A route learns the words it was just taught even when its alias set is
/// already full. Refusing them would leave it unable to answer what someone
/// had only now told it.
#[test]
fn a_full_alias_set_still_learns_the_words_just_taught() {
    let project = Project::new();
    project.write(
        "src/auth.ts",
        "export function checkAuth() { return true; }\n",
    );
    project.build();
    project.teach(
        &[
            "alpha", "bravo", "charlie", "delta", "echo", "foxtrot", "golf", "hotel",
        ],
        "src/auth.ts",
        Some("checkAuth"),
    );
    project.teach(&["front", "door"], "src/auth.ts", Some("checkAuth"));

    assert_eq!(project.top("front door"), "src/auth.ts#checkAuth");
    assert_eq!(
        project.top("the front door of this app"),
        "src/auth.ts#checkAuth",
        "a question around the taught words still reaches it"
    );
}

/// Past the phrase cap the oldest phrase goes, not the one just taught.
#[test]
fn a_route_taught_past_the_phrase_cap_keeps_the_newest_phrase() {
    let project = Project::new();
    project.write(
        "src/auth.ts",
        "export function checkAuth() { return true; }\n",
    );
    project.write("src/other.ts", "export const other = true;\n");
    project.build();
    for filler in ["one", "two", "three", "four", "five", "six"] {
        project.teach(&[filler], "src/other.ts", Some("other"));
    }
    project.teach(&["front", "door"], "src/other.ts", Some("other"));
    project.teach(&["bell", "door", "front"], "src/auth.ts", Some("checkAuth"));

    assert_eq!(project.top("front door"), "src/other.ts#other");
}

/// A hint that happens to share one word with the question is a suggestion.
/// The name the question spelled out is the answer.
#[test]
fn an_incidental_hint_overlap_never_displaces_a_name_the_question_spelled_out() {
    let project = Project::new();
    project.write(
        "src/auth.ts",
        "export function checkAuth() { return true; }\n",
    );
    project.write("src/other.ts", "export const other = true;\n");
    project.build();
    project.teach(&["auth"], "src/other.ts", Some("other"));

    assert_eq!(project.top("checkAuth"), "src/auth.ts#checkAuth");
    assert_eq!(
        project.top("auth"),
        "src/other.ts#other",
        "the taught word on its own still routes"
    );
    assert_eq!(
        project.top("src/auth.ts#checkAuth"),
        "src/auth.ts#checkAuth",
        "nor does a hint displace a path"
    );
}

/// A path answers with the file it names, not with whatever else shares its
/// words.
#[test]
fn a_written_out_path_answers_with_that_file() {
    let project = Project::new();
    project.write(
        "src/auth.ts",
        "export function checkAuth() { return true; }\n",
    );
    project.write(
        "src/other.ts",
        "export function checkAuth() { return false; }\n",
    );
    project.build();

    assert_eq!(
        project.top("src/other.ts#checkAuth"),
        "src/other.ts#checkAuth"
    );
    assert_eq!(project.top("src/auth.ts"), "src/auth.ts");
}

/// Two letters is a name, not filler.
#[test]
fn answers_a_two_letter_name() {
    let project = Project::new();
    project.write("src/store.ts", "export function db() { return null; }\n");
    project.build();

    assert_eq!(project.top("db"), "src/store.ts#db");
}

/// One of two equal answers is not an answer. Asking for a single result
/// cannot turn a tie into a certainty.
#[test]
fn does_not_present_one_of_two_equal_answers_as_sure() {
    let project = Project::new();
    project.write(
        "src/auth.ts",
        "export function checkAuth() { return true; }\n",
    );
    project.write(
        "src/other.ts",
        "export function checkAuth() { return false; }\n",
    );
    project.build();

    let one = project
        .index()
        .question_with_options(
            &project.target(),
            "checkAuth",
            QuestionOptions {
                limit: Some(1),
                code: false,
            },
        )
        .expect("the question answers");
    assert_eq!(one.candidates.len(), 1);
    assert!(
        one.markdown.contains("(matched:"),
        "one of two definitions was reported as the answer: {}",
        one.markdown
    );
}

/// A question about code the task may not read is answered with the reason,
/// not with the next best thing.
#[test]
fn says_when_the_answer_is_outside_the_read_scope() {
    let project = Project::new();
    project.write(
        "src/auth.ts",
        "export function checkAuth() { return true; }\n",
    );
    project.write("src/other.ts", "export const other = true;\n");
    project.build();

    let answer = project
        .index()
        .question(
            &ContextTarget::new(
                project.dir.path(),
                TaskScope {
                    read: vec!["src/auth.ts".into()],
                    write: Vec::new(),
                },
            ),
            "other",
        )
        .expect("the scoped question answers");
    assert!(answer.candidates.is_empty());
    assert!(
        answer.markdown.contains("outside this task's read scope"),
        "{}",
        answer.markdown
    );
}

/// A route whose file is deleted stops answering, and stops being stored.
#[test]
fn a_deleted_target_takes_its_route_with_it() {
    let project = Project::new();
    project.write(
        "src/auth.ts",
        "export function checkAuth() { return true; }\n",
    );
    project.write("src/other.ts", "export const other = true;\n");
    project.build();
    project.teach(&["front door"], "src/auth.ts", Some("checkAuth"));

    fs::remove_file(project.dir.path().join("src/auth.ts")).expect("the target file goes");
    let reconciled = project
        .index()
        .reconcile(project.dir.path(), BuildOptions::default())
        .expect("the index reconciles the delete");
    assert_eq!(reconciled.routes_dropped, 1);
    assert_eq!(project.route_count(), 0);

    let answer = project
        .index()
        .question(&project.target(), "front door")
        .expect("the question answers");
    assert!(answer.candidates.is_empty(), "{}", answer.markdown);
}

/// A route a task taught for code only its branch has answers from that
/// branch, and survives being asked for.
#[test]
fn a_route_taught_in_a_checkout_answers_from_the_checkout() {
    let project = Project::new();
    project.write(
        "src/auth.ts",
        "export function oldAuth() { return true; }\n",
    );
    project.build();

    let checkout = tempdir().expect("checkout directory is creatable");
    fs::create_dir_all(checkout.path().join("src")).expect("checkout source directory");
    fs::write(
        checkout.path().join("src/auth.ts"),
        "// the branch added a header\n\nexport function checkAuth() { return true; }\n",
    )
    .expect("checkout source writes");
    let task = Task {
        cwd: checkout.path().display().to_string(),
        worktree: Some(TaskWorktree {
            origin_cwd: project.dir.path().display().to_string(),
            path: checkout.path().display().to_string(),
            branch: "oga/retrieval".into(),
            links: None,
        }),
        ..project.task()
    };
    let learned = project
        .index()
        .learn_routes(
            &task,
            &[LearnRouteProposal {
                hints: vec!["front door".into()],
                path: "src/auth.ts".into(),
                symbol: Some("checkAuth".into()),
            }],
        )
        .expect("the branch route learns");
    assert_eq!(learned.accepted, 1);

    let answer = project
        .index()
        .question(
            &ContextTarget::worktree(checkout.path(), project.dir.path(), readable()),
            "front door",
        )
        .expect("the branch route answers");
    assert_eq!(
        answer
            .candidates
            .first()
            .map(|hit| (hit.path.as_str(), hit.symbol.as_deref(), hit.line)),
        Some(("src/auth.ts", Some("checkAuth"), 3))
    );
    assert_eq!(
        project.route_count(),
        1,
        "the route survived being asked for"
    );
}

struct Project {
    dir: TempDir,
    _database: TempDir,
    store: Store,
}

impl Project {
    fn new() -> Self {
        let dir = tempdir().expect("project directory is creatable");
        fs::create_dir_all(dir.path().join("src")).expect("source directory is creatable");
        let (database, store) = fixture_store();
        Self {
            dir,
            _database: database,
            store,
        }
    }

    fn index(&self) -> ContextIndex<'_> {
        ContextIndex::new(&self.store)
    }

    fn write(&self, path: &str, body: &str) {
        fs::write(self.dir.path().join(path), body).expect("fixture source writes");
    }

    fn build(&self) {
        self.index()
            .build(self.dir.path(), BuildOptions::default())
            .expect("fixture index builds");
    }

    fn teach(&self, hints: &[&str], path: &str, symbol: Option<&str>) {
        self.index()
            .learn_user_route(
                self.dir.path(),
                &LearnRouteProposal {
                    hints: hints.iter().map(|hint| (*hint).to_owned()).collect(),
                    path: path.to_owned(),
                    symbol: symbol.map(str::to_owned),
                },
            )
            .expect("route learns");
    }

    fn target(&self) -> ContextTarget {
        ContextTarget::new(self.dir.path(), readable())
    }

    fn task(&self) -> Task {
        Task {
            id: "task-retrieval".into(),
            profile_id: "profile-retrieval".into(),
            model: "model-retrieval".into(),
            cwd: self.dir.path().display().to_string(),
            scope: TaskScope {
                read: vec!["**".into()],
                write: vec!["**".into()],
            },
            ..Task::default()
        }
    }

    fn top(&self, question: &str) -> String {
        let answer = self
            .index()
            .question(&self.target(), question)
            .expect("the question answers");
        anchors(&answer)
            .first()
            .cloned()
            .unwrap_or_else(|| format!("no answer: {}", answer.markdown))
    }

    fn route_count(&self) -> i64 {
        self.store
            .with_connection(|connection| {
                Ok(connection.query_row(
                    "SELECT COUNT(*) FROM context_learned_routes",
                    [],
                    |row| row.get(0),
                )?)
            })
            .expect("route count reads")
    }
}

fn anchors(answer: &oga_context::ContextResult) -> Vec<String> {
    answer
        .candidates
        .iter()
        .map(|candidate| match candidate.symbol.as_deref() {
            Some(symbol) => format!("{}#{symbol}", candidate.path),
            None => candidate.path.clone(),
        })
        .collect()
}

fn top(found: &[String]) -> String {
    if found.is_empty() {
        return "nothing".to_owned();
    }
    found
        .iter()
        .take(3)
        .enumerate()
        .map(|(place, anchor)| format!("{}. {anchor}", place + 1))
        .collect::<Vec<_>>()
        .join("  ")
}

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("the crate sits three levels below the repository root")
        .to_path_buf()
}

fn fixture_store() -> (TempDir, Store) {
    let database = tempdir().expect("database directory is creatable");
    let store = Store::open_writable(database.path().join("oga.db"))
        .expect("retrieval fixture store opens");
    (database, store)
}

fn readable() -> TaskScope {
    TaskScope {
        read: vec!["**".into()],
        write: Vec::new(),
    }
}
