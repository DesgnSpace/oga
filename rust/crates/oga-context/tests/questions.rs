//! The question set. Every entry is a question someone would actually type,
//! paired with the one place in this repository that answers it.

use std::fs;
use std::path::{Path, PathBuf};

use oga_context::{BuildOptions, ContextIndex, ContextTarget, QuestionOptions};
use oga_domain::TaskScope;
use oga_store::Store;
use tempfile::{TempDir, tempdir};

/// `(question, path, symbol)`. The anchor is the answer's first line, so a
/// question that drifts to second place is a failure, not a warning.
const QUESTIONS: &[(&str, &str, &str)] = &[
    (
        "how relearn extracts symbols",
        "rust/crates/oga-context/src/symbols.rs",
        "extract_symbols",
    ),
    ("install with homebrew", "README.md", "Install"),
    (
        "sandbox exec binary path",
        "rust/crates/oga-runner/src/confinement/seatbelt.rs",
        "SANDBOX_EXEC",
    ),
    (
        "completion code for permission denied",
        "rust/crates/oga-domain/src/task.rs",
        "PermissionDenied",
    ),
    (
        "toast viewport component",
        "web/src/components/ToastViewport.tsx",
        "ToastViewport",
    ),
    (
        "sidebar screen",
        "web/src/screens/sidebar/Sidebar.tsx",
        "Sidebar",
    ),
    (
        "task scope read and write globs",
        "rust/crates/oga-domain/src/task.rs",
        "TaskScope",
    ),
    (
        "the enum of task states in rust",
        "rust/crates/oga-domain/src/task.rs",
        "TaskState",
    ),
    (
        "task shipped prompt field",
        "rust/crates/oga-domain/src/task.rs",
        "shipped_prompt",
    ),
    (
        "the busy timeout for sqlite",
        "rust/crates/oga-store/src/connection.rs",
        "BUSY_TIMEOUT_MS",
    ),
    (
        "how many events a worker may buffer",
        "rust/crates/oga-runner/src/lib.rs",
        "DEFAULT_EVENT_BUFFER",
    ),
    (
        "which struct holds a walked file",
        "rust/crates/oga-context/src/walk.rs",
        "WalkFile",
    ),
    (
        "how the walker decides a file is indexable",
        "rust/crates/oga-context/src/walk.rs",
        "is_indexable",
    ),
    (
        "compile the tree sitter query once",
        "rust/crates/oga-context/src/lang/mod.rs",
        "compiled_query",
    ),
    (
        "heal a saved route",
        "rust/crates/oga-context/src/routes.rs",
        "heal",
    ),
    (
        "route aliases cap",
        "rust/crates/oga-context/src/routes.rs",
        "MAX_ROUTE_ALIASES",
    ),
    (
        "swift adapter separator",
        "rust/crates/oga-context/src/lang/swift.rs",
        "separator",
    ),
    (
        "updater endpoints the desktop app polls",
        "rust/apps/oga-desktop/tauri.conf.json",
        "endpoints",
    ),
    (
        "traffic light position of the main window",
        "rust/apps/oga-desktop/tauri.conf.json",
        "trafficLightPosition",
    ),
    (
        "cargo workspace package table",
        "rust/Cargo.toml",
        "workspace.package",
    ),
    ("workspace members glob", "rust/Cargo.toml", "members"),
    (
        "does the packaging matrix fail fast",
        "rust/packaging/release.yml",
        "fail-fast",
    ),
    (
        "the package workflow build matrix",
        "rust/packaging/release.yml",
        "matrix",
    ),
];

#[test]
fn answers_plain_language_questions_about_this_repository() {
    let repository = repository_root();
    let database = tempdir().expect("database directory is creatable");
    let store =
        Store::open_writable(database.path().join("oga.db")).expect("question fixture store opens");
    let index = ContextIndex::new(&store);
    index
        .build(&repository, BuildOptions::default())
        .expect("repository index builds");
    let target = ContextTarget::new(&repository, readable());

    let misses = QUESTIONS
        .iter()
        .filter_map(|(question, path, symbol)| {
            let answer = index
                .question_with_options(&target, question, QuestionOptions::default())
                .expect("question answers");
            let top = answer.candidates.first();
            let hit = top.is_some_and(|candidate| {
                candidate.path == *path && candidate.symbol.as_deref() == Some(*symbol)
            });
            (!hit).then(|| {
                format!(
                    "  {question:?}\n    want {path}#{symbol}\n    got  {}",
                    answer.markdown.lines().next().unwrap_or_default()
                )
            })
        })
        .collect::<Vec<_>>();
    assert!(
        misses.is_empty(),
        "{} of {} questions missed:\n{}",
        misses.len(),
        QUESTIONS.len(),
        misses.join("\n")
    );
}

/// Swift is a day-one adapter but this repository ships no Swift source, so
/// the extension case runs against a project written for it.
#[test]
fn answers_a_question_about_a_swift_extension() {
    let project = tempdir().expect("project directory is creatable");
    fs::write(
        project.path().join("Refresh.swift"),
        "import Foundation\n\n\
         public struct TaskList {}\n\n\
         /// Pull the newest tasks in.\n\
         extension TaskList {\n\
         \x20   public func refreshFromBroker() async {}\n\
         }\n",
    )
    .expect("swift fixture writes");
    let (_database, store) = fixture_store();
    let index = ContextIndex::new(&store);
    index
        .build(project.path(), BuildOptions::default())
        .expect("swift fixture index builds");
    let target = ContextTarget::new(project.path(), readable());

    let answer = index
        .question_with_options(&target, "refresh from broker", QuestionOptions::default())
        .expect("swift question answers");
    let top = answer.candidates.first().expect("swift question hits");
    assert_eq!(top.path, "Refresh.swift");
    assert_eq!(top.symbol.as_deref(), Some("refreshFromBroker"));

    let extension = index
        .files(project.path())
        .expect("swift fixture files read")
        .remove(0)
        .symbols
        .into_iter()
        .find(|symbol| symbol.kind == oga_domain::SymbolKind::Impl)
        .expect("the extension is indexed");
    assert_eq!(extension.name, "TaskList");
    assert_eq!(extension.doc.as_deref(), Some("Pull the newest tasks in."));
}

/// An unchanged project re-reads nothing. This is the ceiling `oga relearn`
/// has to stay under to be worth running before every lookup.
#[test]
fn an_unchanged_project_reconciles_without_reparsing() {
    let repository = repository_root();
    let (_database, store) = fixture_store();
    let index = ContextIndex::new(&store);
    let built = index
        .build(&repository, BuildOptions::default())
        .expect("repository index builds");
    assert!(!built.partial);
    assert!(built.symbol_count > 1_000, "{built:?}");

    let reconciled = index
        .reconcile(&repository, BuildOptions::default())
        .expect("repository index reconciles");
    assert!(!reconciled.changed, "{reconciled:?}");
    assert_eq!(reconciled.refreshed, 0);
    assert_eq!(reconciled.file_count, built.file_count);
    assert_eq!(reconciled.symbol_count, built.symbol_count);
}

/// Python, Go and PHP have no source in this repository, so their questions
/// run against a project written for them.
#[test]
fn answers_questions_about_python_go_and_php_sources() {
    let project = tempdir().expect("project directory is creatable");
    fs::write(
        project.path().join("router.py"),
        "class RouteHealer:\n\
         \x20   \"\"\"Repairs a saved route after a rename.\"\"\"\n\n\
         \x20   def rebuild_route_table(self, rows):\n\
         \x20       return len(rows)\n",
    )
    .expect("python fixture writes");
    fs::write(
        project.path().join("broker.go"),
        "package broker\n\n\
         const MaxInFlightWorkers = 8\n\n\
         type Broker struct{}\n\n\
         func (b *Broker) DrainPendingEvents() int { return 0 }\n",
    )
    .expect("go fixture writes");
    fs::write(
        project.path().join("Invoice.php"),
        "<?php\n\n\
         enum PaymentState: string\n\
         {\n\
         \x20   case Refunded = 'refunded';\n\
         }\n\n\
         class InvoiceRenderer\n\
         {\n\
         \x20   public function renderMonthlySummary(): string { return ''; }\n\
         }\n",
    )
    .expect("php fixture writes");

    let (_database, store) = fixture_store();
    let index = ContextIndex::new(&store);
    index
        .build(project.path(), BuildOptions::default())
        .expect("polyglot fixture index builds");
    let target = ContextTarget::new(project.path(), readable());

    for (question, path, symbol) in [
        (
            "rebuild the route table",
            "router.py",
            "rebuild_route_table",
        ),
        (
            "which class repairs a saved route",
            "router.py",
            "RouteHealer",
        ),
        ("drain pending events", "broker.go", "DrainPendingEvents"),
        (
            "how many workers may be in flight",
            "broker.go",
            "MaxInFlightWorkers",
        ),
        (
            "render the monthly summary",
            "Invoice.php",
            "renderMonthlySummary",
        ),
        ("which case marks a refund", "Invoice.php", "Refunded"),
    ] {
        let answer = index
            .question_with_options(&target, question, QuestionOptions::default())
            .expect("polyglot question answers");
        let top = answer.candidates.first().expect("polyglot question hits");
        assert_eq!(
            (top.path.as_str(), top.symbol.as_deref()),
            (path, Some(symbol)),
            "{question:?}"
        );
    }
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
    let store =
        Store::open_writable(database.path().join("oga.db")).expect("question fixture store opens");
    (database, store)
}

fn readable() -> TaskScope {
    TaskScope {
        read: vec!["**".into()],
        write: Vec::new(),
    }
}
