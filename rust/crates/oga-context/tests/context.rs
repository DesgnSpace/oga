use std::fs;
use std::time::Duration;

use oga_context::{
    BuildOptions, ContextIndex, ContextTarget, LearnRouteProposal, QueryOptions, QuestionOptions,
    RenderTier, clean_comment, extract_symbols,
};
use oga_domain::{SourceLang, Task, TaskScope};
use oga_store::Store;
use tempfile::{TempDir, tempdir};

struct Fixture {
    project: TempDir,
    _database: TempDir,
    store: Store,
}

impl Fixture {
    fn new() -> Self {
        let project = tempdir().expect("project directory is creatable");
        fs::create_dir_all(project.path().join("src")).expect("source directory is creatable");
        let database = tempdir().expect("database directory is creatable");
        let store = Store::open_writable(database.path().join("oga.db"))
            .expect("context fixture store opens");
        Self {
            project,
            _database: database,
            store,
        }
    }

    fn write_auth(&self, body: &str) {
        fs::write(self.project.path().join("src/auth.ts"), body).expect("auth fixture writes");
    }

    fn write_other(&self) {
        fs::write(
            self.project.path().join("src/other.ts"),
            "export const other = true;\n",
        )
        .expect("other fixture writes");
    }
}

#[test]
fn builds_symbols_and_ranks_exact_questions() {
    let fixture = Fixture::new();
    fixture.write_auth(
        "/** Verify the caller token. */\nexport function checkAuth(token: string): boolean {\n  return token.length > 0;\n}\n",
    );
    let index = ContextIndex::new(&fixture.store);

    let result = index
        .build(fixture.project.path(), BuildOptions::default())
        .expect("context map builds");
    assert_eq!(result.file_count, 1);
    assert_eq!(result.symbol_count, 1);

    let files = index
        .files(fixture.project.path())
        .expect("mapped files read");
    assert_eq!(files[0].path, "src/auth.ts");
    assert_eq!(files[0].symbols[0].name, "checkAuth");

    let target = ContextTarget::new(
        fixture.project.path(),
        TaskScope {
            read: vec!["**".into()],
            write: Vec::new(),
        },
    );
    let result = index
        .question(&target, "where is checkAuth handled")
        .expect("question ranks context");
    assert_eq!(result.candidates[0].path, "src/auth.ts");
    assert_eq!(result.candidates[0].symbol.as_deref(), Some("checkAuth"));
    assert!(result.markdown.contains("src/auth.ts:2#checkAuth"));
}

#[test]
fn limits_question_results_and_reads_current_source_for_code() {
    let fixture = Fixture::new();
    fixture.write_auth(
        "export function checkAuth(token: string): boolean {\n  return token.length > 0;\n}\n",
    );
    fixture.write_other();
    let index = ContextIndex::new(&fixture.store);
    index
        .build(fixture.project.path(), BuildOptions::default())
        .expect("context map builds");
    let target = ContextTarget::new(
        fixture.project.path(),
        TaskScope {
            read: vec!["**".into()],
            write: Vec::new(),
        },
    );

    let result = index
        .question_with_options(
            &target,
            "where is checkAuth handled",
            QuestionOptions {
                limit: Some(1),
                code: true,
            },
        )
        .expect("question renders");
    assert_eq!(result.candidates.len(), 1);
    assert_eq!(
        result.candidates[0].code.as_deref(),
        Some(
            "```text\nexport function checkAuth(token: string): boolean {\n  return token.length > 0;\n}\n```"
        )
    );
    assert!(result.markdown.contains("return token.length > 0;"));
}

#[test]
fn filters_scope_and_reconciles_a_move() {
    let fixture = Fixture::new();
    fixture.write_auth("export function checkAuth(token: string): boolean { return !!token; }\n");
    fixture.write_other();
    let index = ContextIndex::new(&fixture.store);
    index
        .build(fixture.project.path(), BuildOptions::default())
        .expect("context map builds");

    let target = ContextTarget::new(
        fixture.project.path(),
        TaskScope {
            read: vec!["src/auth.ts".into()],
            write: Vec::new(),
        },
    );
    let result = index
        .list(
            &target,
            &QueryOptions {
                paths: vec!["src/".into()],
                tier: Some(RenderTier::Index),
                ..QueryOptions::default()
            },
        )
        .expect("scoped map reads");
    assert_eq!(result.files.len(), 1);
    assert_eq!(result.outside_scope, 1);

    fs::rename(
        fixture.project.path().join("src/auth.ts"),
        fixture.project.path().join("src/verify.ts"),
    )
    .expect("source file moves");
    let result = index
        .reconcile(fixture.project.path(), BuildOptions::default())
        .expect("context map reconciles");
    assert!(result.changed);
    let paths = index
        .files(fixture.project.path())
        .expect("reconciled files read")
        .into_iter()
        .map(|file| file.path)
        .collect::<Vec<_>>();
    assert!(paths.contains(&"src/verify.ts".into()));
    assert!(!paths.contains(&"src/auth.ts".into()));
}

#[test]
fn learns_routes_and_verifies_changed_worktree_sources() {
    let fixture = Fixture::new();
    fixture.write_auth("export function checkAuth(token: string): boolean { return !!token; }\n");
    let index = ContextIndex::new(&fixture.store);
    index
        .build(fixture.project.path(), BuildOptions::default())
        .expect("context map builds");

    let task = Task {
        id: "task-context".into(),
        profile_id: "profile-context".into(),
        model: "model-context".into(),
        cwd: fixture.project.path().display().to_string(),
        scope: TaskScope {
            read: vec!["**".into()],
            write: Vec::new(),
        },
        ..Task::default()
    };
    let learned = index
        .learn_routes(
            &task,
            &[LearnRouteProposal {
                hints: vec!["auth".into(), "check".into()],
                path: "src/auth.ts".into(),
                symbol: Some("checkAuth".into()),
            }],
        )
        .expect("route learns");
    assert_eq!(learned.accepted, 1);
    let routes = index
        .learned_routes(fixture.project.path(), "auth check")
        .expect("learned routes read");
    assert_eq!(routes[0].path, "src/auth.ts");
    assert_eq!(routes[0].symbol.as_deref(), Some("checkAuth"));
    let target = ContextTarget::new(
        fixture.project.path(),
        TaskScope {
            read: vec!["**".into()],
            write: Vec::new(),
        },
    );
    let answer = index
        .question(&target, "auth check")
        .expect("learned route answers questions");
    assert_eq!(answer.candidates[0].symbol.as_deref(), Some("checkAuth"));

    let checkout = tempdir().expect("checkout directory is creatable");
    fs::create_dir_all(checkout.path().join("src"))
        .expect("checkout source directory is creatable");
    fs::copy(
        fixture.project.path().join("src/auth.ts"),
        checkout.path().join("src/auth.ts"),
    )
    .expect("source copies to checkout");
    fs::write(
        checkout.path().join("src/auth.ts"),
        "export function checkAuth(token: string): boolean { return token !== ''; }\n",
    )
    .expect("checkout source changes");
    let verification = index
        .verify_worktree(
            fixture.project.path(),
            checkout.path(),
            &["src/auth.ts".into()],
        )
        .expect("worktree verifies");
    assert!(verification[0].changed);
    assert!(verification[0].file.is_some());
}

#[test]
fn learned_routes_follow_symbol_renames() {
    let fixture = Fixture::new();
    fixture.write_auth("export function checkAuth(token: string): boolean { return !!token; }\n");
    let index = ContextIndex::new(&fixture.store);
    index
        .build(fixture.project.path(), BuildOptions::default())
        .expect("context map builds");
    let task = Task {
        id: "task-context".into(),
        profile_id: "profile-context".into(),
        model: "model-context".into(),
        cwd: fixture.project.path().display().to_string(),
        scope: TaskScope {
            read: vec!["**".into()],
            write: Vec::new(),
        },
        ..Task::default()
    };
    index
        .learn_routes(
            &task,
            &[LearnRouteProposal {
                hints: vec!["auth".into(), "check".into()],
                path: "src/auth.ts".into(),
                symbol: Some("checkAuth".into()),
            }],
        )
        .expect("route learns");
    let target = ContextTarget::new(
        fixture.project.path(),
        TaskScope {
            read: vec!["**".into()],
            write: Vec::new(),
        },
    );

    fixture.write_auth(
        "import { log } from './log';\n\nlog('auth');\n\nexport function checkAuth(token: string): boolean { return token !== ''; }\n",
    );
    index
        .reconcile(fixture.project.path(), BuildOptions::default())
        .expect("map reconciles the edit");
    let answer = index
        .question(&target, "auth check")
        .expect("edited file still routes");
    assert_eq!(answer.candidates[0].symbol.as_deref(), Some("checkAuth"));
    assert_eq!(answer.candidates[0].line, 5);
    assert!(answer.markdown.contains("src/auth.ts:5#checkAuth"));

    fixture.write_auth(
        "export function verifyToken(token: string): boolean { return token !== ''; }\n",
    );
    index
        .reconcile(fixture.project.path(), BuildOptions::default())
        .expect("map reconciles the rename");
    let answer = index
        .question(&target, "auth check")
        .expect("question still answers");
    assert_eq!(answer.candidates[0].symbol.as_deref(), Some("verifyToken"));
    let routes = index
        .learned_routes(fixture.project.path(), "auth check")
        .expect("learned routes read");
    assert_eq!(routes[0].symbol.as_deref(), Some("verifyToken"));
}

#[test]
fn learned_routes_follow_file_moves_during_reconcile() {
    let fixture = Fixture::new();
    fixture.write_auth("export function checkAuth() { return true; }\n");
    let index = ContextIndex::new(&fixture.store);
    index
        .build(fixture.project.path(), BuildOptions::default())
        .expect("map builds");
    let task = Task {
        id: "task-context".into(),
        profile_id: "profile-context".into(),
        model: "model-context".into(),
        cwd: fixture.project.path().display().to_string(),
        scope: TaskScope {
            read: vec!["**".into()],
            write: Vec::new(),
        },
        ..Task::default()
    };
    index
        .learn_routes(
            &task,
            &[LearnRouteProposal {
                hints: vec!["auth".into(), "check".into()],
                path: "src/auth.ts".into(),
                symbol: Some("checkAuth".into()),
            }],
        )
        .expect("route learns");
    fs::rename(
        fixture.project.path().join("src/auth.ts"),
        fixture.project.path().join("src/security.ts"),
    )
    .expect("file moves");
    index
        .reconcile(fixture.project.path(), BuildOptions::default())
        .expect("map reconciles the move");
    let routes = index
        .learned_routes(fixture.project.path(), "auth check")
        .expect("routes read");
    assert_eq!(routes[0].path, "src/security.ts");
}

#[test]
fn learned_routes_follow_file_and_symbol_renames() {
    let fixture = Fixture::new();
    fixture.write_auth("export function checkAuth() { return true; }\n");
    let index = ContextIndex::new(&fixture.store);
    index
        .build(fixture.project.path(), BuildOptions::default())
        .expect("map builds");
    index
        .learn_user_route(
            fixture.project.path(),
            &LearnRouteProposal {
                hints: vec!["auth check".into()],
                path: "src/auth.ts".into(),
                symbol: Some("checkAuth".into()),
            },
        )
        .expect("route learns");
    fs::rename(
        fixture.project.path().join("src/auth.ts"),
        fixture.project.path().join("src/login.ts"),
    )
    .expect("file moves");
    fs::write(
        fixture.project.path().join("src/login.ts"),
        "export function verifyLogin() { return true; }\n",
    )
    .expect("renamed source writes");
    let result = index
        .reconcile(fixture.project.path(), BuildOptions::default())
        .expect("map reconciles the rename");
    assert_eq!(result.route_moves.len(), 1);
    let route = index
        .learned_routes(fixture.project.path(), "auth check")
        .expect("route resolves");
    assert_eq!(route[0].path, "src/login.ts");
    assert_eq!(route[0].symbol.as_deref(), Some("verifyLogin"));
}

#[test]
fn learned_routes_match_synonyms_and_dedupe_entity_aliases() {
    let fixture = Fixture::new();
    fixture.write_auth("export function checkAuth() { return true; }\n");
    let index = ContextIndex::new(&fixture.store);
    index
        .build(fixture.project.path(), BuildOptions::default())
        .expect("map builds");
    for hint in ["auth", "login|sign in"] {
        index
            .learn_user_route(
                fixture.project.path(),
                &LearnRouteProposal {
                    hints: vec![hint.into()],
                    path: "src/auth.ts".into(),
                    symbol: Some("checkAuth".into()),
                },
            )
            .expect("route learns");
    }
    for question in ["auth", "login", "sign in"] {
        assert_eq!(
            index
                .learned_routes(fixture.project.path(), question)
                .expect("synonym route resolves")[0]
                .symbol
                .as_deref(),
            Some("checkAuth")
        );
    }
    let count: i64 = fixture
        .store
        .with_connection(|connection| {
            Ok(
                connection.query_row("SELECT COUNT(*) FROM context_learned_routes", [], |row| {
                    row.get(0)
                })?,
            )
        })
        .expect("route count reads");
    assert_eq!(count, 1);
}

#[test]
fn learned_route_aliases_are_capped() {
    let fixture = Fixture::new();
    fixture.write_auth("export function checkAuth() { return true; }\n");
    let index = ContextIndex::new(&fixture.store);
    index
        .build(fixture.project.path(), BuildOptions::default())
        .expect("map builds");
    index
        .learn_user_route(
            fixture.project.path(),
            &LearnRouteProposal {
                hints: vec!["login|alpha|bravo|charlie|delta|echo|foxtrot|golf|hotel".into()],
                path: "src/auth.ts".into(),
                symbol: Some("checkAuth".into()),
            },
        )
        .expect("route learns");
    let aliases: String = fixture
        .store
        .with_connection(|connection| {
            Ok(
                connection.query_row("SELECT aliases FROM context_learned_routes", [], |row| {
                    row.get(0)
                })?,
            )
        })
        .expect("route aliases read");
    assert!(aliases.split_whitespace().count() <= 8);
}

#[test]
fn validates_worktree_routes_against_checkout_before_origin_catches_up() {
    let fixture = Fixture::new();
    fixture.write_auth("export function oldAuth() { return true; }\n");
    let index = ContextIndex::new(&fixture.store);
    index
        .build(fixture.project.path(), BuildOptions::default())
        .expect("origin map builds");

    let checkout = tempdir().expect("checkout directory is creatable");
    fs::create_dir_all(checkout.path().join("src"))
        .expect("checkout source directory is creatable");
    fs::write(
        checkout.path().join("src/auth.ts"),
        "export function checkAuth() { return true; }\n",
    )
    .expect("checkout source writes");
    let task = Task {
        id: "task-worktree-context".into(),
        profile_id: "profile-context".into(),
        model: "model-context".into(),
        cwd: checkout.path().display().to_string(),
        worktree: Some(oga_domain::TaskWorktree {
            origin_cwd: fixture.project.path().display().to_string(),
            path: checkout.path().display().to_string(),
            branch: "task/context".into(),
            links: None,
        }),
        scope: TaskScope {
            read: vec!["**".into()],
            write: vec!["**".into()],
        },
        ..Task::default()
    };
    let result = index
        .learn_routes(
            &task,
            &[LearnRouteProposal {
                hints: vec!["auth".into(), "check".into()],
                path: "src/auth.ts".into(),
                symbol: Some("checkAuth".into()),
            }],
        )
        .expect("worktree route learns from checkout");
    assert_eq!(result.accepted, 1);
}

#[test]
fn folds_only_in_tree_corrections_and_keeps_them_out_of_missing_symbols() {
    let fixture = Fixture::new();
    fixture.write_auth("export function checkAuth() { return true; }\n");
    let index = ContextIndex::new(&fixture.store);
    let task = Task {
        id: "task-corrections".into(),
        cwd: fixture.project.path().display().to_string(),
        output: "## Map corrections\nsrc/auth.ts — Auth entry point.\nsrc/auth.ts:checkAuth — Checks the token.\nsrc/auth.ts:missing — Ignore this.\n".into(),
        ..Task::default()
    };
    index.fold_task(&task).expect("task corrections fold");
    let file = index
        .files(fixture.project.path())
        .expect("corrected file reads")[0]
        .clone();
    assert_eq!(file.purpose.as_deref(), Some("Auth entry point."));
    assert_eq!(
        file.symbols[0].purpose.as_deref(),
        Some("Checks the token.")
    );
    assert!(!file.symbols.iter().any(|symbol| symbol.name == "missing"));
}

#[test]
fn resolves_relative_imports_for_graph_importance() {
    let fixture = Fixture::new();
    fs::write(
        fixture.project.path().join("src/zzz.ts"),
        "export function shared() { return true; }\n",
    )
    .expect("hub fixture writes");
    fs::write(
        fixture.project.path().join("src/aaa.ts"),
        "export function lonely() { return true; }\n",
    )
    .expect("leaf fixture writes");
    for name in ["one", "two", "three"] {
        fs::write(
            fixture.project.path().join(format!("src/{name}.ts")),
            "import { shared } from './zzz';\nexport function useShared() { shared(); }\n",
        )
        .expect("caller fixture writes");
    }
    let index = ContextIndex::new(&fixture.store);
    index
        .build(fixture.project.path(), BuildOptions::default())
        .expect("importance map builds");
    let files = index
        .files(fixture.project.path())
        .expect("importance files read");
    let hub = files.iter().find(|file| file.path == "src/zzz.ts").unwrap();
    let leaf = files.iter().find(|file| file.path == "src/aaa.ts").unwrap();
    assert!(hub.importance > leaf.importance);
}

#[test]
fn marks_file_budget_as_partial() {
    let fixture = Fixture::new();
    fixture.write_auth("export const auth = true;\n");
    fixture.write_other();
    let index = ContextIndex::new(&fixture.store);
    let result = index
        .build(
            fixture.project.path(),
            BuildOptions {
                max_files: 1,
                budget: Duration::from_secs(2),
                max_symbols: 5_000,
            },
        )
        .expect("partial context map builds");
    assert!(result.partial);
    assert_eq!(result.file_count, 1);
}

#[test]
fn extracts_supported_declarations_without_comment_or_string_noise() {
    let source = [
        "import { value } from './value';",
        "export function setMemory(cwd, key, value, expectedVersion?) { return 1 }",
        "function helper() {}",
        "export default function main() {}",
        "export class Foo extends Bar {}",
        "interface Ignored {}",
        "export enum IgnoredEnum { value }",
        "export type Memory = string;",
        "const arrow = (value) => value;",
        "const twoLines = (value) =>",
        "  value + 1;",
        "const long = (value: number): string => {",
        "  return String(value);",
        "};",
        "export async function many(a, b, c, d, e): Promise<void> {}",
    ]
    .join("\n");
    let extracted = extract_symbols(&source, SourceLang::Ts, None);
    let names = extracted
        .symbols
        .iter()
        .map(|symbol| symbol.name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        vec![
            "setMemory",
            "helper",
            "main",
            "Foo",
            "Memory",
            "long",
            "many"
        ]
    );
    assert_eq!(
        extracted.symbols[0].params.as_deref(),
        Some("cwd, key, value, expectedVersion?")
    );
    assert_eq!(extracted.symbols[5].returns.as_deref(), Some("string"));
    assert_eq!(extracted.symbols[6].params.as_deref(), Some("a, b, c, …"));
    assert_eq!(
        clean_comment("/** Signs the outgoing request. */").as_deref(),
        Some("Signs the outgoing request.")
    );
    assert_eq!(clean_comment("// Copyright 2026").as_deref(), None);

    let noisy_braces = [
        "const message = '} {';",
        "// const fake = 1",
        "const template = `x { y }`;",
        "export function okay() { return message; }",
    ]
    .join("\n");
    let extracted = extract_symbols(&noisy_braces, SourceLang::Ts, None);
    assert!(!extracted.unparsed);
    assert_eq!(
        extracted
            .symbols
            .iter()
            .map(|symbol| symbol.name.as_str())
            .collect::<Vec<_>>(),
        vec!["message", "template", "okay"]
    );

    let broken = extract_symbols("function broken( {", SourceLang::Ts, None);
    assert!(broken.unparsed);
    assert_eq!(
        broken.unparsed_reason.as_deref(),
        Some("TypeScript syntax scan failed")
    );

    let swift = extract_symbols(
        "public struct ContentView: View {}\nstruct Helper {}\nfunc add(a: Int, b: Int = 5) -> Int { a + b }\n",
        SourceLang::Swift,
        None,
    );
    assert_eq!(
        swift
            .symbols
            .iter()
            .map(|symbol| (symbol.name.as_str(), symbol.kind, symbol.exported))
            .collect::<Vec<_>>(),
        vec![
            ("ContentView", oga_domain::SymbolKind::View, true),
            ("Helper", oga_domain::SymbolKind::Struct, false),
            ("add", oga_domain::SymbolKind::Fn, false),
        ]
    );
    assert_eq!(swift.symbols[2].params.as_deref(), Some("a, b"));
    assert_eq!(swift.symbols[2].returns.as_deref(), Some("Int"));

    let generic = extract_symbols(
        "def check():\n    return True\n",
        SourceLang::Generic,
        Some(oga_context::GenericLanguage::Python),
    );
    assert_eq!(generic.symbols[0].name, "check");

    let object = extract_symbols(
        "const serveOptions = {\n  async fetch(request) {\n    return items.map((item) => item);\n  },\n};\nexport function after() { return 1 }\n",
        SourceLang::Ts,
        None,
    );
    assert_eq!(
        object
            .symbols
            .iter()
            .map(|symbol| symbol.name.as_str())
            .collect::<Vec<_>>(),
        vec!["serveOptions", "after"]
    );
    assert!(object.symbols[0].params.is_none());
}
