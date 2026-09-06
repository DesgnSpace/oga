//! One adapter per language. An adapter names the extensions it owns, hands
//! over a grammar and a capture query, and answers the few questions the
//! engine cannot answer for itself: what counts as public, what a doc comment
//! looks like, and which files are not worth indexing.

mod markdown;
mod rust;
mod swift;
mod typescript;

use std::sync::{Mutex, OnceLock};

use oga_domain::SymbolKind;
use tree_sitter::{Language, Node, Query};

/// How a language joins a symbol to the symbol that encloses it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Separator {
    /// `Store::open`
    Colons,
    /// `TaskList.render`
    Dot,
    /// `Install > Homebrew`
    Arrow,
}

impl Separator {
    pub fn as_str(self) -> &'static str {
        match self {
            Separator::Colons => "::",
            Separator::Dot => ".",
            Separator::Arrow => " > ",
        }
    }
}

pub trait LanguageAdapter: Send + Sync {
    /// The name stored on every file row this adapter parses.
    fn name(&self) -> &'static str;

    /// Lowercase extensions, without the dot.
    fn extensions(&self) -> &'static [&'static str];

    /// The compiled grammar for a file with the given extension. One adapter
    /// can front several grammars — TSX needs its own.
    fn language(&self, extension: &str) -> Language;

    /// A capture query. Every pattern captures the declaration as
    /// `@def.<kind>` and its identifier as `@name`.
    fn query(&self, extension: &str) -> &'static str;

    fn separator(&self) -> Separator;

    /// Whether the language treats this declaration as visible outside its
    /// file.
    fn exported(&self, node: Node<'_>, source: &str) -> bool;

    /// Whether this node is a comment that documents whatever follows it.
    fn is_doc_comment(&self, node: Node<'_>) -> bool {
        matches!(node.kind(), "comment" | "line_comment" | "block_comment")
    }

    /// Files this adapter refuses: generated output, vendored copies, and
    /// anything else whose symbols would only crowd the answers.
    fn is_noise(&self, path: &str) -> bool {
        let _ = path;
        false
    }

    /// Declarations this adapter refuses, along with everything inside them:
    /// a test module, a test case, a generated block.
    fn is_noise_node(&self, node: Node<'_>, source: &str) -> bool {
        let _ = (node, source);
        false
    }
}

/// Every adapter, in registration order. Adding a language means adding one
/// module beside this one and one line here.
pub fn adapters() -> &'static [&'static dyn LanguageAdapter] {
    static ADAPTERS: OnceLock<Vec<&'static dyn LanguageAdapter>> = OnceLock::new();
    ADAPTERS
        .get_or_init(|| {
            vec![
                &rust::Rust as &'static dyn LanguageAdapter,
                &typescript::TypeScript,
                &swift::Swift,
                &markdown::Markdown,
            ]
        })
        .as_slice()
}

/// The adapter that owns `path`, and the extension it matched on.
pub fn adapter_for(path: &str) -> Option<(&'static dyn LanguageAdapter, String)> {
    let extension = path.rsplit_once('.')?.1.to_ascii_lowercase();
    let adapter = adapters()
        .iter()
        .find(|adapter| adapter.extensions().contains(&extension.as_str()))?;
    (!adapter.is_noise(path)).then_some((*adapter, extension))
}

struct CompiledQuery {
    adapter: &'static str,
    extension: String,
    query: &'static Query,
}

/// Compile a query once per (adapter, extension) and reuse it for every file.
/// Compiling is the expensive part of a lookup; parsing is not.
pub fn compiled_query(adapter: &dyn LanguageAdapter, extension: &str) -> &'static Query {
    static CACHE: OnceLock<Mutex<Vec<CompiledQuery>>> = OnceLock::new();
    let mut cached = CACHE
        .get_or_init(|| Mutex::new(Vec::new()))
        .lock()
        .expect("query cache lock");
    if let Some(entry) = cached
        .iter()
        .find(|entry| entry.adapter == adapter.name() && entry.extension == extension)
    {
        return entry.query;
    }
    let query = Query::new(&adapter.language(extension), adapter.query(extension))
        .expect("adapter query compiles");
    let query: &'static Query = Box::leak(Box::new(query));
    cached.push(CompiledQuery {
        adapter: adapter.name(),
        extension: extension.to_owned(),
        query,
    });
    query
}

/// The kind a `@def.<kind>` capture names.
pub fn capture_kind(capture: &str) -> Option<SymbolKind> {
    SymbolKind::parse(capture.strip_prefix("def.")?)
}

/// Path segments whose files exist to support the code rather than to be
/// found: fixtures, snapshots, build output, vendored trees.
pub fn is_support_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.split('/').any(|part| {
        [
            "node_modules",
            "vendor",
            "vendored",
            "generated",
            "__generated__",
            "__snapshots__",
            "dist",
            "build",
            "target",
            "coverage",
            ".next",
            ".svelte-kit",
        ]
        .contains(&part)
    }) || lower.ends_with(".d.ts")
        || lower.ends_with(".min.js")
        || lower.ends_with(".snap")
}
