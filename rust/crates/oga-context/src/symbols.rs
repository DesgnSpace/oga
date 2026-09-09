//! The parsing engine: run one adapter's capture query over a parsed tree and
//! turn the matches into symbols. Everything here is language-agnostic; what a
//! language declares, and what counts as public in it, lives in `lang`.

use oga_domain::SymbolKind;
use streaming_iterator::StreamingIterator;
use tree_sitter::{Node, Parser, QueryCursor};

use crate::lang::{self, LanguageAdapter};
use crate::text::clean_comment;

/// The most symbols one file contributes. A generated file with thousands of
/// declarations would otherwise drown out the code someone is looking for.
const MAX_SYMBOLS_PER_FILE: usize = 400;

const MAX_SIGNATURE_CHARS: usize = 200;
const MAX_DOC_CHARS: usize = 400;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractedSymbol {
    pub kind: SymbolKind,
    pub name: String,
    pub qualified: String,
    pub line: u64,
    pub end_line: u64,
    pub signature: String,
    pub doc: Option<String>,
    pub exported: bool,
    pub parent: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractedFile {
    pub lang: &'static str,
    pub symbols: Vec<ExtractedSymbol>,
}

/// Parse `source` with the adapter that owns `path` and return its symbols in
/// source order. `None` means no adapter claims the file.
pub fn extract_symbols(path: &str, source: &str) -> Option<ExtractedFile> {
    let (adapter, extension) = lang::adapter_for(path)?;
    let mut parser = Parser::new();
    parser.set_language(&adapter.language(&extension)).ok()?;
    let tree = parser.parse(source, None)?;
    let query = lang::compiled_query(adapter, &extension);
    let mut cursor = QueryCursor::new();
    let mut captured = Vec::new();
    let mut matches = cursor.matches(query, tree.root_node(), source.as_bytes());
    while let Some(matched) = matches.next() {
        let mut definition = None;
        let mut name = None;
        for capture in matched.captures() {
            let capture_name = &query.capture_names()[capture.index as usize];
            if *capture_name == "name" {
                name = Some(capture.node);
            } else if let Some(kind) = lang::capture_kind(capture_name) {
                definition = Some((kind, capture.node));
            }
        }
        let (Some((kind, node)), Some(name)) = (definition, name) else {
            continue;
        };
        let Some(name) = slice(source, name).map(clean_name) else {
            continue;
        };
        if name.is_empty() {
            continue;
        }
        captured.push(Captured { kind, node, name });
    }
    captured.sort_by_key(|item| (item.node.start_byte(), usize::MAX - item.node.end_byte()));
    captured.dedup_by_key(|item| item.node.id());
    Some(ExtractedFile {
        lang: adapter.name(),
        symbols: nest(captured, adapter, source),
    })
}

struct Captured<'tree> {
    kind: SymbolKind,
    node: Node<'tree>,
    name: String,
}

/// Nest the captures by byte containment, then let that nesting settle the
/// three things it decides: the qualified name, whether a function is really a
/// method, and whether a declaration is local to a body and not worth
/// indexing.
fn nest(
    captured: Vec<Captured<'_>>,
    adapter: &dyn LanguageAdapter,
    source: &str,
) -> Vec<ExtractedSymbol> {
    let mut open = Vec::<Enclosing>::new();
    let mut symbols = Vec::new();
    for item in captured {
        while open
            .last()
            .is_some_and(|frame| frame.end <= item.node.start_byte())
        {
            open.pop();
        }
        if open.last().is_some_and(|frame| frame.opaque) {
            continue;
        }
        if adapter.is_noise_node(item.node, source) {
            open.push(Enclosing {
                end: item.node.end_byte(),
                kind: item.kind,
                qualified: item.name,
                opaque: true,
            });
            continue;
        }
        let parent = open.last();
        let kind = settle_kind(item.kind, parent.map(|frame| frame.kind));
        let qualified = match parent {
            Some(frame) => format!(
                "{}{}{}",
                frame.qualified,
                adapter.separator().as_str(),
                item.name
            ),
            None => item.name.clone(),
        };
        let mut symbol = ExtractedSymbol {
            kind,
            name: item.name,
            qualified,
            line: item.node.start_position().row as u64 + 1,
            end_line: end_line(item.node),
            signature: signature(item.node, source),
            doc: doc_comment(item.node, adapter, source),
            exported: adapter.exported(item.node, source),
            parent: parent.map(|frame| frame.qualified.clone()),
        };
        adapter.refine(&mut symbol, item.node, source);
        open.push(Enclosing {
            end: item.node.end_byte(),
            kind,
            qualified: symbol.qualified.clone(),
            opaque: is_opaque(kind),
        });
        symbols.push(symbol);
        if symbols.len() == MAX_SYMBOLS_PER_FILE {
            break;
        }
    }
    symbols
}

/// The last line the declaration actually covers. A node that ends at the
/// first column of a line — a markdown section handing over to the next
/// heading — stopped on the line before.
fn end_line(node: Node<'_>) -> u64 {
    let end = node.end_position();
    if end.column == 0 && end.row > node.start_position().row {
        end.row as u64
    } else {
        end.row as u64 + 1
    }
}

/// A symbol still open at this point in the file, and whether anything inside
/// it counts.
struct Enclosing {
    end: usize,
    kind: SymbolKind,
    qualified: String,
    opaque: bool,
}

/// A function declared inside a type is a method; a stored value declared
/// outside one is a constant, not a field.
fn settle_kind(kind: SymbolKind, parent: Option<SymbolKind>) -> SymbolKind {
    match (kind, parent) {
        (SymbolKind::Fn, Some(parent)) if holds_members(parent) => SymbolKind::Method,
        (SymbolKind::Field, None) => SymbolKind::Const,
        (kind, _) => kind,
    }
}

/// Nothing inside a body or a value is a symbol of its own: a local variable,
/// a parameter's inline type, a helper closure. They belong to the
/// declaration that holds them.
fn is_opaque(kind: SymbolKind) -> bool {
    matches!(
        kind,
        SymbolKind::Fn | SymbolKind::Method | SymbolKind::Const | SymbolKind::Static
    )
}

fn holds_members(kind: SymbolKind) -> bool {
    matches!(
        kind,
        SymbolKind::Impl
            | SymbolKind::Trait
            | SymbolKind::Class
            | SymbolKind::Struct
            | SymbolKind::Enum
    )
}

/// The declaration up to its body, on one line. A node with no body — a
/// constant, a heading — contributes its first line instead.
fn signature(node: Node<'_>, source: &str) -> String {
    let end = node
        .child_by_field_name("body")
        .map(|body| body.start_byte())
        .unwrap_or(node.end_byte())
        .min(node.end_byte());
    one_line(source.get(node.start_byte()..end).unwrap_or_default())
}

/// A declaration's first line, collapsed to single spaces and capped. What a
/// signature looks like once it is a searchable string.
pub fn one_line(text: &str) -> String {
    let text = text.split('\n').next().unwrap_or(text);
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(MAX_SIGNATURE_CHARS)
        .collect()
}

/// The comment block directly above the declaration, markers stripped.
/// Attributes and decorators sit between the two and are stepped over.
fn doc_comment(node: Node<'_>, adapter: &dyn LanguageAdapter, source: &str) -> Option<String> {
    let node = outermost_at_start(node);
    let mut blocks = Vec::new();
    let mut above = node.start_position().row;
    let mut previous = node.prev_named_sibling();
    while let Some(candidate) = previous {
        if is_annotation(candidate) {
            above = candidate.start_position().row;
            previous = candidate.prev_named_sibling();
            continue;
        }
        if !adapter.is_doc_comment(candidate) || candidate.end_position().row + 1 < above {
            break;
        }
        blocks.push(slice(source, candidate).unwrap_or_default());
        above = candidate.start_position().row;
        previous = candidate.prev_named_sibling();
    }
    blocks.reverse();
    clean_comment(&blocks.join("\n")).map(|doc| doc.chars().take(MAX_DOC_CHARS).collect())
}

/// A declaration's doc comment sits above whatever wraps it — `export`, a
/// `const` binding — not above the declaration node itself. Climb out of the
/// wrappers that open on the same line and lead with a keyword.
fn outermost_at_start(node: Node<'_>) -> Node<'_> {
    let mut outermost = node;
    while let Some(parent) = outermost.parent() {
        if parent.parent().is_none()
            || outermost.prev_named_sibling().is_some()
            || parent.start_position().row != outermost.start_position().row
        {
            break;
        }
        outermost = parent;
    }
    outermost
}

fn is_annotation(node: Node<'_>) -> bool {
    matches!(
        node.kind(),
        "attribute_item" | "attribute" | "decorator" | "modifiers"
    )
}

fn slice<'a>(source: &'a str, node: Node<'_>) -> Option<&'a str> {
    source.get(node.byte_range())
}

/// Names arrive with whatever the grammar handed back around them: a markdown
/// heading's closing `#`, a Swift backtick escape. A backtick pair only comes
/// off when it wraps the whole name, so a heading that quotes code keeps it.
fn clean_name(raw: &str) -> String {
    let name = raw.trim().trim_end_matches('#').trim();
    name.strip_prefix('`')
        .and_then(|rest| rest.strip_suffix('`'))
        .unwrap_or(name)
        .trim()
        .to_owned()
}
