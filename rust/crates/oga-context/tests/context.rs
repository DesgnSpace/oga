use std::fs;

use oga_context::{
    BuildOptions, ContextIndex, ContextTarget, LearnRouteProposal, QuestionOptions, adapters,
    extract_symbols,
};
use oga_domain::{SymbolKind, Task, TaskScope};
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

    fn task(&self) -> Task {
        Task {
            id: "task-context".into(),
            profile_id: "profile-context".into(),
            model: "model-context".into(),
            cwd: self.project.path().display().to_string(),
            scope: everything(),
            ..Task::default()
        }
    }

    fn target(&self) -> ContextTarget {
        ContextTarget::new(self.project.path(), everything())
    }
}

fn everything() -> TaskScope {
    TaskScope {
        read: vec!["**".into()],
        write: vec!["**".into()],
    }
}

fn names(path: &str, source: &str) -> Vec<String> {
    extract_symbols(path, source)
        .expect("an adapter owns the fixture")
        .symbols
        .into_iter()
        .map(|symbol| symbol.name)
        .collect()
}

#[test]
fn every_adapter_query_compiles_for_every_extension_it_claims() {
    for adapter in adapters() {
        for extension in adapter.extensions() {
            let path = format!("fixture.{extension}");
            assert!(
                extract_symbols(&path, "").is_some(),
                "{} rejected .{extension}",
                adapter.name()
            );
        }
    }
}

#[test]
fn extracts_rust_declarations_of_every_kind() {
    let source = [
        "//! Module docs.",
        "/// The build budget.",
        "pub const BUILD_BUDGET: u64 = 2;",
        "static COUNTER: u32 = 0;",
        "pub struct Parsed {",
        "    pub count: usize,",
        "}",
        "pub enum Kind { Fn, Class }",
        "pub trait Adapter { fn name(&self) -> &'static str; }",
        "impl Adapter for Rust {",
        "    fn name(&self) -> &'static str { \"rust\" }",
        "}",
        "pub type Alias = Result<(), String>;",
        "pub mod inner { pub fn helper() {} }",
        "macro_rules! shout { () => {} }",
        "pub fn extract(source: &str) -> usize { let local = 1; local }",
    ]
    .join("\n");
    let extracted = extract_symbols("src/lib.rs", &source).expect("rust parses");
    let found = extracted
        .symbols
        .iter()
        .map(|symbol| (symbol.kind, symbol.qualified.as_str()))
        .collect::<Vec<_>>();
    assert_eq!(
        found,
        vec![
            (SymbolKind::Const, "BUILD_BUDGET"),
            (SymbolKind::Static, "COUNTER"),
            (SymbolKind::Struct, "Parsed"),
            (SymbolKind::Field, "Parsed::count"),
            (SymbolKind::Enum, "Kind"),
            (SymbolKind::Variant, "Kind::Fn"),
            (SymbolKind::Variant, "Kind::Class"),
            (SymbolKind::Trait, "Adapter"),
            (SymbolKind::Method, "Adapter::name"),
            (SymbolKind::Impl, "Rust"),
            (SymbolKind::Method, "Rust::name"),
            (SymbolKind::Type, "Alias"),
            (SymbolKind::Module, "inner"),
            (SymbolKind::Fn, "inner::helper"),
            (SymbolKind::Macro, "shout"),
            (SymbolKind::Fn, "extract"),
        ]
    );
    let budget = &extracted.symbols[0];
    assert_eq!(budget.doc.as_deref(), Some("The build budget."));
    assert_eq!(budget.signature, "pub const BUILD_BUDGET: u64 = 2;");
    assert!(budget.exported);
    assert!(!extracted.symbols[1].exported);
}

#[test]
fn skips_rust_test_modules_and_test_cases() {
    let source = [
        "pub fn ship() {}",
        "#[test]",
        "fn ship_sends_everything() {}",
        "#[cfg(test)]",
        "mod tests {",
        "    fn helper_that_should_not_answer_questions() {}",
        "}",
    ]
    .join("\n");
    assert_eq!(names("src/lib.rs", &source), vec!["ship"]);
}

#[test]
fn extracts_typescript_and_tsx_declarations() {
    let source = [
        "export const MAX_ROWS = 50;",
        "export type TaskState = \"queued\" | \"running\";",
        "export interface TaskRow { id: string }",
        "export enum Phase { Idle = \"idle\" }",
        "/** Shows the task list. */",
        "export function TaskList({ rows }: { rows: TaskRow[] }) {",
        "  return <ul>{rows.map((row) => <li key={row.id}>{row.id}</li>)}</ul>;",
        "}",
        "export default class Store {",
        "  private items: string[] = [];",
        "  add(item: string): void { this.items.push(item); }",
        "}",
        "const localOnly = 1;",
    ]
    .join("\n");
    let extracted = extract_symbols("web/src/TaskList.tsx", &source).expect("tsx parses");
    let found = extracted
        .symbols
        .iter()
        .map(|symbol| (symbol.kind, symbol.qualified.as_str(), symbol.exported))
        .collect::<Vec<_>>();
    assert_eq!(
        found,
        vec![
            (SymbolKind::Const, "MAX_ROWS", true),
            (SymbolKind::Type, "TaskState", true),
            (SymbolKind::Trait, "TaskRow", true),
            (SymbolKind::Field, "TaskRow.id", true),
            (SymbolKind::Enum, "Phase", true),
            (SymbolKind::Variant, "Phase.Idle", true),
            (SymbolKind::Fn, "TaskList", true),
            (SymbolKind::Class, "Store", true),
            (SymbolKind::Field, "Store.items", false),
            (SymbolKind::Method, "Store.add", true),
            (SymbolKind::Const, "localOnly", false),
        ]
    );
    assert_eq!(
        extracted.symbols[6].doc.as_deref(),
        Some("Shows the task list.")
    );
}

#[test]
fn extracts_swift_types_extensions_and_members() {
    let source = [
        "public struct ContentView: View {",
        "    /// The current count.",
        "    @State private var count = 0",
        "    func bump() { count += 1 }",
        "}",
        "extension ContentView {",
        "    static let title = \"Oga\"",
        "}",
        "protocol Refreshable { func refresh() async }",
        "enum Phase: String { case idle }",
        "typealias Handler = (Int) -> Void",
        "let sharedLimit = 42",
    ]
    .join("\n");
    let extracted = extract_symbols("App/ContentView.swift", &source).expect("swift parses");
    let found = extracted
        .symbols
        .iter()
        .map(|symbol| (symbol.kind, symbol.qualified.as_str()))
        .collect::<Vec<_>>();
    assert_eq!(
        found,
        vec![
            (SymbolKind::Struct, "ContentView"),
            (SymbolKind::Field, "ContentView::count"),
            (SymbolKind::Method, "ContentView::bump"),
            (SymbolKind::Impl, "ContentView"),
            (SymbolKind::Field, "ContentView::title"),
            (SymbolKind::Trait, "Refreshable"),
            (SymbolKind::Method, "Refreshable::refresh"),
            (SymbolKind::Enum, "Phase"),
            (SymbolKind::Variant, "Phase::idle"),
            (SymbolKind::Type, "Handler"),
            (SymbolKind::Const, "sharedLimit"),
        ]
    );
    assert!(
        !extracted.symbols[1].exported,
        "private var is not exported"
    );
    assert_eq!(
        extracted.symbols[1].doc.as_deref(),
        Some("The current count.")
    );
}

#[test]
fn extracts_markdown_headings_with_their_level_and_section() {
    let source = [
        "# Install Oga",
        "",
        "Some text.",
        "",
        "## Install with Homebrew",
        "",
        "```sh",
        "# Not a heading",
        "```",
        "",
        "## Build from source",
    ]
    .join("\n");
    let extracted = extract_symbols("README.md", &source).expect("markdown parses");
    let found = extracted
        .symbols
        .iter()
        .map(|symbol| {
            (
                symbol.qualified.as_str(),
                symbol.signature.as_str(),
                symbol.line,
                symbol.end_line,
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        found,
        vec![
            ("Install Oga", "# Install Oga", 1, 11),
            (
                "Install Oga > Install with Homebrew",
                "## Install with Homebrew",
                5,
                10
            ),
            (
                "Install Oga > Build from source",
                "## Build from source",
                11,
                11
            ),
        ]
    );
}

#[test]
fn extracts_python_functions_classes_constants_and_fields() {
    let source = [
        "MAX_RETRIES = 3",
        "_private = 1",
        "attempts = 0",
        "",
        "@dataclass",
        "class Route:",
        "    \"\"\"A learned route.\"\"\"",
        "",
        "    hint: str",
        "    weight: int = 1",
        "",
        "    def heal(self, source):",
        "        \"\"\"Follow the target.\"\"\"",
        "        return True",
        "",
        "    def _internal(self):",
        "        return None",
        "",
        "def extract(path):",
        "    local = 1",
        "    return local",
    ]
    .join("\n");
    let extracted = extract_symbols("tools/route.py", &source).expect("python parses");
    let found = extracted
        .symbols
        .iter()
        .map(|symbol| (symbol.kind, symbol.qualified.as_str(), symbol.exported))
        .collect::<Vec<_>>();
    assert_eq!(
        found,
        vec![
            (SymbolKind::Const, "MAX_RETRIES", true),
            (SymbolKind::Class, "Route", true),
            (SymbolKind::Field, "Route.hint", true),
            (SymbolKind::Field, "Route.weight", true),
            (SymbolKind::Method, "Route.heal", true),
            (SymbolKind::Method, "Route._internal", false),
            (SymbolKind::Fn, "extract", true),
        ]
    );
    assert_eq!(
        extracted.symbols[1].doc.as_deref(),
        Some("A learned route.")
    );
    assert_eq!(
        extracted.symbols[4].doc.as_deref(),
        Some("Follow the target.")
    );
}

#[test]
fn extracts_go_declarations_and_keeps_the_receiver_in_the_method_name() {
    let source = [
        "package broker",
        "",
        "// MaxRetries caps the retry loop.",
        "const MaxRetries = 3",
        "",
        "var Timeout = 5",
        "",
        "type Kind = string",
        "",
        "type Handler func(int)",
        "",
        "type Route struct {",
        "\tHint   string",
        "\tweight int",
        "}",
        "",
        "type Adapter interface {",
        "\tName() string",
        "}",
        "",
        "func (r *Route) Heal(source string) bool { return true }",
        "",
        "func Extract(path string) int { return 1 }",
        "",
        "func private() {}",
    ]
    .join("\n");
    let extracted = extract_symbols("broker/route.go", &source).expect("go parses");
    let found = extracted
        .symbols
        .iter()
        .map(|symbol| (symbol.kind, symbol.qualified.as_str(), symbol.exported))
        .collect::<Vec<_>>();
    assert_eq!(
        found,
        vec![
            (SymbolKind::Const, "MaxRetries", true),
            (SymbolKind::Static, "Timeout", true),
            (SymbolKind::Type, "Kind", true),
            (SymbolKind::Type, "Handler", true),
            (SymbolKind::Struct, "Route", true),
            (SymbolKind::Field, "Route.Hint", true),
            (SymbolKind::Field, "Route.weight", false),
            (SymbolKind::Trait, "Adapter", true),
            (SymbolKind::Method, "Adapter.Name", true),
            (SymbolKind::Method, "Route.Heal", true),
            (SymbolKind::Fn, "Extract", true),
            (SymbolKind::Fn, "private", false),
        ]
    );
    assert_eq!(
        extracted.symbols[0].doc.as_deref(),
        Some("MaxRetries caps the retry loop.")
    );
    assert_eq!(extracted.symbols[9].parent.as_deref(), Some("Route"));
}

#[test]
fn extracts_php_namespaces_types_and_members() {
    let source = [
        "<?php",
        "",
        "namespace App\\Broker;",
        "",
        "interface Adapter",
        "{",
        "    public function name(): string;",
        "}",
        "",
        "trait Healing",
        "{",
        "    public function heal(): bool { return true; }",
        "}",
        "",
        "enum Kind: string",
        "{",
        "    case Queued = 'queued';",
        "}",
        "",
        "/** A learned route. */",
        "class Route implements Adapter",
        "{",
        "    public const MAX_HINTS = 12;",
        "",
        "    public string $hint = '';",
        "    private int $weight = 1;",
        "",
        "    public function name(): string { return $this->hint; }",
        "",
        "    private function secret(): void {}",
        "}",
        "",
        "function extract_symbols(string $path): int { return 1; }",
    ]
    .join("\n");
    let extracted = extract_symbols("app/Broker/Route.php", &source).expect("php parses");
    let found = extracted
        .symbols
        .iter()
        .map(|symbol| (symbol.kind, symbol.qualified.as_str(), symbol.exported))
        .collect::<Vec<_>>();
    assert_eq!(
        found,
        vec![
            (SymbolKind::Module, "App\\Broker", true),
            (SymbolKind::Trait, "Adapter", true),
            (SymbolKind::Method, "Adapter::name", true),
            (SymbolKind::Trait, "Healing", true),
            (SymbolKind::Method, "Healing::heal", true),
            (SymbolKind::Enum, "Kind", true),
            (SymbolKind::Variant, "Kind::Queued", true),
            (SymbolKind::Class, "Route", true),
            (SymbolKind::Const, "Route::MAX_HINTS", true),
            (SymbolKind::Field, "Route::hint", true),
            (SymbolKind::Field, "Route::weight", false),
            (SymbolKind::Method, "Route::name", true),
            (SymbolKind::Method, "Route::secret", false),
            (SymbolKind::Fn, "extract_symbols", true),
        ]
    );
    assert_eq!(
        extracted.symbols[7].doc.as_deref(),
        Some("A learned route.")
    );
}

#[test]
fn extracts_json_keys_with_their_dotted_path_and_short_values() {
    let source = [
        "{",
        "  \"productName\": \"Oga\",",
        "  \"updater\": {",
        "    \"endpoints\": [",
        "      { \"url\": \"https://oga.dev/appcast.xml\" }",
        "    ]",
        "  }",
        "}",
    ]
    .join("\n");
    let extracted = extract_symbols("tauri.conf.json", &source).expect("json parses");
    let found = extracted
        .symbols
        .iter()
        .map(|symbol| (symbol.kind, symbol.qualified.as_str(), symbol.line))
        .collect::<Vec<_>>();
    assert_eq!(
        found,
        vec![
            (SymbolKind::Const, "productName", 2),
            (SymbolKind::Module, "updater", 3),
            (SymbolKind::Module, "updater.endpoints", 4),
            (SymbolKind::Module, "updater.endpoints[0]", 5),
            (SymbolKind::Field, "updater.endpoints[0].url", 5),
        ]
    );
    assert_eq!(
        extracted.symbols[4].signature,
        "\"url\": \"https://oga.dev/appcast.xml\""
    );
}

#[test]
fn extracts_toml_tables_and_keys() {
    let source = [
        "title = \"oga\"",
        "",
        "# The version every crate shares.",
        "[workspace.package]",
        "version = \"0.0.6\"",
        "",
        "[[bin]]",
        "name = \"oga\"",
        "",
        "[[bin]]",
        "name = \"ogad\"",
    ]
    .join("\n");
    let extracted = extract_symbols("Cargo.toml", &source).expect("toml parses");
    let found = extracted
        .symbols
        .iter()
        .map(|symbol| (symbol.kind, symbol.qualified.as_str(), symbol.line))
        .collect::<Vec<_>>();
    assert_eq!(
        found,
        vec![
            (SymbolKind::Const, "title", 1),
            (SymbolKind::Module, "workspace.package", 4),
            (SymbolKind::Field, "workspace.package.version", 5),
            (SymbolKind::Module, "bin[0]", 7),
            (SymbolKind::Field, "bin[0].name", 8),
            (SymbolKind::Module, "bin[1]", 10),
            (SymbolKind::Field, "bin[1].name", 11),
        ]
    );
    assert_eq!(
        extracted.symbols[1].doc.as_deref(),
        Some("The version every crate shares.")
    );
}

#[test]
fn extracts_yaml_keys_through_sequences() {
    let source = [
        "name: build",
        "on:",
        "  push:",
        "    branches: [main]",
        "jobs:",
        "  test:",
        "    steps:",
        "      - uses: actions/checkout@v4",
        "        name: Check out",
    ]
    .join("\n");
    let extracted = extract_symbols(".github/workflows/ci.yml", &source).expect("yaml parses");
    let found = extracted
        .symbols
        .iter()
        .map(|symbol| (symbol.kind, symbol.qualified.as_str(), symbol.line))
        .collect::<Vec<_>>();
    assert_eq!(
        found,
        vec![
            (SymbolKind::Const, "name", 1),
            (SymbolKind::Module, "on", 2),
            (SymbolKind::Module, "on.push", 3),
            (SymbolKind::Module, "on.push.branches", 4),
            (SymbolKind::Module, "jobs", 5),
            (SymbolKind::Module, "jobs.test", 6),
            (SymbolKind::Module, "jobs.test.steps", 7),
            (SymbolKind::Module, "jobs.test.steps[0]", 8),
            (SymbolKind::Field, "jobs.test.steps[0].uses", 8),
            (SymbolKind::Field, "jobs.test.steps[0].name", 9),
        ]
    );
    assert_eq!(extracted.symbols[8].signature, "uses: actions/checkout@v4");
}

#[test]
fn refuses_lockfiles_whatever_their_format() {
    for path in [
        "package-lock.json",
        "bun.lock",
        "pnpm-lock.yaml",
        "Cargo.lock",
        "composer.lock",
        "Package.resolved",
        "web/uv.lock",
    ] {
        assert!(extract_symbols(path, "{}").is_none(), "{path} was indexed");
    }
}

#[test]
fn builds_symbols_and_answers_an_exact_name() {
    let fixture = Fixture::new();
    fixture.write_auth(
        "/** Verify the caller token. */\nexport function checkAuth(token: string): boolean {\n  return token.length > 0;\n}\n",
    );
    let index = ContextIndex::new(&fixture.store);

    let result = index
        .build(fixture.project.path(), BuildOptions::default())
        .expect("index builds");
    assert_eq!(result.file_count, 1);
    assert_eq!(result.symbol_count, 1);

    let result = index
        .question(&fixture.target(), "where is checkAuth handled")
        .expect("question ranks context");
    assert_eq!(result.candidates[0].path, "src/auth.ts");
    assert_eq!(result.candidates[0].symbol.as_deref(), Some("checkAuth"));
    assert!(result.markdown.contains("src/auth.ts:2#checkAuth"));
}

#[test]
fn reports_an_honest_miss() {
    let fixture = Fixture::new();
    fixture.write_auth("export function checkAuth() { return true; }\n");
    let index = ContextIndex::new(&fixture.store);
    index
        .build(fixture.project.path(), BuildOptions::default())
        .expect("index builds");

    let result = index
        .question(&fixture.target(), "kubernetes ingress controller")
        .expect("question answers");
    assert!(result.candidates.is_empty());
    assert!(
        result
            .markdown
            .starts_with("No confident match for \"kubernetes ingress controller\""),
        "{}",
        result.markdown
    );
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
        .expect("index builds");

    let result = index
        .question_with_options(
            &fixture.target(),
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
        .expect("index builds");

    let target = ContextTarget::new(
        fixture.project.path(),
        TaskScope {
            read: vec!["src/auth.ts".into()],
            write: Vec::new(),
        },
    );
    let result = index
        .question(&target, "where is other")
        .expect("scoped question answers");
    assert!(
        result.candidates.is_empty(),
        "src/other.ts is outside the task's read scope"
    );

    fs::rename(
        fixture.project.path().join("src/auth.ts"),
        fixture.project.path().join("src/verify.ts"),
    )
    .expect("source file moves");
    let result = index
        .reconcile(fixture.project.path(), BuildOptions::default())
        .expect("index reconciles");
    assert!(result.changed);
    assert_eq!(result.moved, 1);
    let moved = index
        .question(&fixture.target(), "where is checkAuth handled")
        .expect("question follows the move");
    assert_eq!(moved.candidates[0].path, "src/verify.ts");
}

#[test]
fn a_learned_route_outranks_everything_the_parser_found() {
    let fixture = Fixture::new();
    fixture.write_auth("export function checkAuth(token: string): boolean { return !!token; }\n");
    fixture.write_other();
    let index = ContextIndex::new(&fixture.store);
    index
        .build(fixture.project.path(), BuildOptions::default())
        .expect("index builds");

    let learned = index
        .learn_routes(
            &fixture.task(),
            &[LearnRouteProposal {
                hints: vec!["front door".into()],
                path: "src/auth.ts".into(),
                symbol: Some("checkAuth".into()),
            }],
        )
        .expect("route learns");
    assert_eq!(learned.accepted, 1);

    let answer = index
        .question(&fixture.target(), "front door")
        .expect("learned route answers questions");
    assert_eq!(answer.candidates[0].symbol.as_deref(), Some("checkAuth"));
    assert_eq!(answer.candidates.len(), 1, "a hint is decisive");
}

#[test]
fn a_route_rejects_a_symbol_that_is_not_there() {
    let fixture = Fixture::new();
    fixture.write_auth("export function checkAuth() { return true; }\n");
    let index = ContextIndex::new(&fixture.store);
    index
        .build(fixture.project.path(), BuildOptions::default())
        .expect("map builds");

    let result = index
        .learn_routes(
            &fixture.task(),
            &[LearnRouteProposal {
                hints: vec!["auth".into()],
                path: "src/auth.ts".into(),
                symbol: Some("missingSymbol".into()),
            }],
        )
        .expect("route proposal is judged");
    assert_eq!(result.accepted, 0);
    assert_eq!(
        result.rejected[0].reason,
        "symbol missingSymbol is not present in src/auth.ts"
    );
}

#[test]
fn learned_routes_follow_symbol_renames() {
    let fixture = Fixture::new();
    fixture.write_auth("export function checkAuth(token: string): boolean { return !!token; }\n");
    let index = ContextIndex::new(&fixture.store);
    index
        .build(fixture.project.path(), BuildOptions::default())
        .expect("index builds");
    index
        .learn_routes(
            &fixture.task(),
            &[LearnRouteProposal {
                hints: vec!["auth".into(), "check".into()],
                path: "src/auth.ts".into(),
                symbol: Some("checkAuth".into()),
            }],
        )
        .expect("route learns");

    fixture.write_auth(
        "import { log } from './log';\n\nlog('auth');\n\nexport function checkAuth(token: string): boolean { return !!token; }\n",
    );
    index
        .reconcile(fixture.project.path(), BuildOptions::default())
        .expect("map reconciles the edit");
    let answer = index
        .question(&fixture.target(), "auth check")
        .expect("edited file still routes");
    assert_eq!(answer.candidates[0].symbol.as_deref(), Some("checkAuth"));
    assert_eq!(answer.candidates[0].line, 5);
    assert!(answer.markdown.contains("src/auth.ts:5#checkAuth"));

    fixture.write_auth("export function verifyToken(token: string): boolean { return !!token; }\n");
    let reconciled = index
        .reconcile(fixture.project.path(), BuildOptions::default())
        .expect("map reconciles the rename");
    assert_eq!(reconciled.routes_confirmed, 1);
    let answer = index
        .question(&fixture.target(), "auth check")
        .expect("question still answers");
    assert_eq!(answer.candidates[0].symbol.as_deref(), Some("verifyToken"));
}

#[test]
fn learned_routes_follow_file_moves_during_reconcile() {
    let fixture = Fixture::new();
    fixture.write_auth("export function checkAuth() { return true; }\n");
    let index = ContextIndex::new(&fixture.store);
    index
        .build(fixture.project.path(), BuildOptions::default())
        .expect("map builds");
    index
        .learn_routes(
            &fixture.task(),
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
    let reconciled = index
        .reconcile(fixture.project.path(), BuildOptions::default())
        .expect("map reconciles the move");
    assert_eq!(
        reconciled.route_moves[0].to_path.as_str(),
        "src/security.ts"
    );
    let answer = index
        .question(&fixture.target(), "auth check")
        .expect("moved route answers");
    assert_eq!(answer.candidates[0].path, "src/security.ts");
}

#[test]
fn learned_routes_follow_a_file_and_symbol_rename_together() {
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
    fs::remove_file(fixture.project.path().join("src/auth.ts")).expect("old file goes");
    fs::write(
        fixture.project.path().join("src/login.ts"),
        "export function verifyLogin() { return true; }\n",
    )
    .expect("renamed source writes");

    let result = index
        .reconcile(fixture.project.path(), BuildOptions::default())
        .expect("map reconciles the rename");
    assert_eq!(result.route_moves.len(), 1);
    let answer = index
        .question(&fixture.target(), "auth check")
        .expect("route resolves");
    assert_eq!(answer.candidates[0].path, "src/login.ts");
    assert_eq!(answer.candidates[0].symbol.as_deref(), Some("verifyLogin"));
}

#[test]
fn learned_routes_match_synonyms_and_keep_one_row_per_target() {
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
        let answer = index
            .question(&fixture.target(), question)
            .expect("synonym route resolves");
        assert_eq!(
            answer.candidates[0].symbol.as_deref(),
            Some("checkAuth"),
            "{question}"
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
fn validates_worktree_routes_against_the_checkout_before_the_origin_catches_up() {
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
        cwd: checkout.path().display().to_string(),
        worktree: Some(oga_domain::TaskWorktree {
            origin_cwd: fixture.project.path().display().to_string(),
            path: checkout.path().display().to_string(),
            branch: "task/context".into(),
            links: None,
        }),
        ..fixture.task()
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
fn a_worktree_answer_reports_the_line_in_the_checkout() {
    let fixture = Fixture::new();
    fixture.write_auth("export function checkAuth() { return true; }\n");
    let index = ContextIndex::new(&fixture.store);
    index
        .build(fixture.project.path(), BuildOptions::default())
        .expect("origin map builds");

    let checkout = tempdir().expect("checkout directory is creatable");
    fs::create_dir_all(checkout.path().join("src"))
        .expect("checkout source directory is creatable");
    fs::write(
        checkout.path().join("src/auth.ts"),
        "// the checkout added a header\n\nexport function checkAuth() { return true; }\n",
    )
    .expect("checkout source writes");

    let answer = index
        .question(
            &ContextTarget::worktree(checkout.path(), fixture.project.path(), everything()),
            "check auth",
        )
        .expect("worktree question answers");
    assert_eq!(answer.candidates[0].path, "src/auth.ts");
    assert_eq!(answer.candidates[0].line, 3);
}

#[test]
fn marks_the_file_budget_as_partial() {
    let fixture = Fixture::new();
    fixture.write_auth("export const auth = true;\n");
    fixture.write_other();
    let index = ContextIndex::new(&fixture.store);
    let result = index
        .build(
            fixture.project.path(),
            BuildOptions {
                max_files: 1,
                max_symbols: 5_000,
            },
        )
        .expect("partial index builds");
    assert!(result.partial);
    assert_eq!(result.file_count, 1);
}

#[test]
fn reconcile_reparses_only_what_changed() {
    let fixture = Fixture::new();
    fixture.write_auth("export function checkAuth() { return true; }\n");
    fixture.write_other();
    let index = ContextIndex::new(&fixture.store);
    index
        .build(fixture.project.path(), BuildOptions::default())
        .expect("map builds");

    let idle = index
        .reconcile(fixture.project.path(), BuildOptions::default())
        .expect("idle reconcile");
    assert!(!idle.changed);
    assert_eq!(idle.refreshed, 0);

    fixture.write_auth("export function checkAuth() { return false; }\nexport const tries = 3;\n");
    let changed = index
        .reconcile(fixture.project.path(), BuildOptions::default())
        .expect("reconcile after an edit");
    assert!(changed.changed);
    assert_eq!(changed.refreshed, 1);
    assert_eq!(changed.file_count, 2);
    assert_eq!(changed.symbol_count, 3);
}
