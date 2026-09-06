use tree_sitter::{Language, Node};

use super::{LanguageAdapter, Separator, config};
use crate::symbols::ExtractedSymbol;

pub struct Toml;

impl LanguageAdapter for Toml {
    fn name(&self) -> &'static str {
        "toml"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["toml"]
    }

    fn language(&self, _extension: &str) -> Language {
        tree_sitter_toml_ng::LANGUAGE.into()
    }

    fn query(&self, _extension: &str) -> &'static str {
        include_str!("queries/toml.scm")
    }

    fn separator(&self) -> Separator {
        Separator::Dot
    }

    fn exported(&self, _node: Node<'_>, _source: &str) -> bool {
        true
    }

    fn is_noise(&self, path: &str) -> bool {
        super::is_support_path(path) || config::is_lockfile(path)
    }

    fn refine(&self, symbol: &mut ExtractedSymbol, node: Node<'_>, source: &str) {
        config::refine(symbol, node, source);
    }
}
