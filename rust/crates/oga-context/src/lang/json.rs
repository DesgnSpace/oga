use tree_sitter::{Language, Node};

use super::{LanguageAdapter, Separator, config};
use crate::symbols::ExtractedSymbol;

pub struct Json;

impl LanguageAdapter for Json {
    fn name(&self) -> &'static str {
        "json"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["json", "jsonc"]
    }

    fn language(&self, _extension: &str) -> Language {
        tree_sitter_json::LANGUAGE.into()
    }

    fn query(&self, _extension: &str) -> &'static str {
        include_str!("queries/json.scm")
    }

    fn separator(&self) -> Separator {
        Separator::Dot
    }

    /// A settings file has no private half.
    fn exported(&self, _node: Node<'_>, _source: &str) -> bool {
        true
    }

    /// JSON has no comments, and the JSONC ones sit wherever the writer put
    /// them rather than above the key they explain.
    fn is_doc_comment(&self, _node: Node<'_>) -> bool {
        false
    }

    fn is_noise(&self, path: &str) -> bool {
        super::is_support_path(path) || config::is_lockfile(path)
    }

    fn refine(&self, symbol: &mut ExtractedSymbol, node: Node<'_>, source: &str) {
        config::refine(symbol, node, source);
    }
}
