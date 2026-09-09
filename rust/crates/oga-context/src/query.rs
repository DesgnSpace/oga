//! Turning a plain-language question into one anchor.
//!
//! Signals are tried in the order a reader would trust them: a place the
//! question spelled out, then a hint someone taught the project, then a symbol
//! whose name is what was asked, then a symbol whose name shares the
//! question's words, then the search index over doc comments and signatures,
//! then the path.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use oga_domain::SymbolKind;

use crate::routes::LearnedRoute;
use crate::store::SymbolRow;
use crate::text::{identifier_words, name_key, words};

/// Beyond this many candidates the ranking has already decided; the rest are
/// noise carried by one shared word.
pub const CANDIDATE_POOL: usize = 200;
pub const DEFAULT_LIMIT: usize = 10;

/// A place the question named outright. Nothing the parser or a hint found
/// competes with a path someone typed.
const DIRECT: f64 = 4_000_000.0;
const ROUTE_EXACT: f64 = 1_000_000.0;
/// Every word of the question is a word this route was taught. Not the taught
/// phrase, but not a coincidence either.
const ROUTE_COVERED: f64 = 500_000.0;
/// One or two words in common. A suggestion, ranked among the parser's own,
/// so it can never displace a name the question spelled out.
const ROUTE_PARTIAL: f64 = 6_000.0;
const NAME_EXACT: f64 = 40_000.0;
const NAME_COVERED: f64 = 6_000.0;
const NAME_TERM: f64 = 3_000.0;
const PARENT_TERM: f64 = 800.0;
const DOC_TERM: f64 = 400.0;
const PATH_TERM: f64 = 250.0;
const EXPORTED: f64 = 300.0;
const SEARCH_WEIGHT: f64 = 20.0;
const SEARCH_CEILING: f64 = 30.0;

#[derive(Debug, Clone)]
pub struct Scored {
    pub symbol: SymbolRow,
    pub score: f64,
    pub matched: Vec<String>,
    /// A named place, a taught phrase, or an exact name settles the answer on
    /// its own.
    pub decisive: bool,
}

/// How much each of the question's words is worth. A word that reaches most
/// of the project barely narrows anything; a word that reaches three symbols
/// almost picks the answer by itself.
#[derive(Debug, Default)]
pub struct TermWeights {
    weights: HashMap<String, f64>,
}

impl TermWeights {
    pub fn new(hits: &HashMap<String, u64>, total: usize) -> Self {
        let total = total.max(1) as f64;
        Self {
            weights: hits
                .iter()
                .map(|(term, hits)| {
                    let rarity = (total / (1.0 + *hits as f64)).ln();
                    (term.clone(), rarity.clamp(0.5, 6.0))
                })
                .collect(),
        }
    }

    fn of(&self, term: &str) -> f64 {
        self.weights.get(term).copied().unwrap_or(1.0)
    }
}

#[derive(Debug, Default)]
pub struct Ranking {
    scored: HashMap<String, Scored>,
}

impl Ranking {
    /// A taught hint outranks anything the parser found, but only a route
    /// taught the phrase that was asked settles the answer by itself. One
    /// shared word among several routes — "adapter" taught for three
    /// unrelated files — must compete like everything else, not each claim
    /// sole confidence and get picked by an arbitrary tiebreak.
    pub fn add_route(
        &mut self,
        route: &LearnedRoute,
        symbol: SymbolRow,
        terms: &[String],
        weights: &TermWeights,
    ) {
        let matched = terms
            .iter()
            .filter(|term| {
                route
                    .aliases
                    .split_whitespace()
                    .any(|alias| alias == term.as_str())
            })
            .cloned()
            .collect::<Vec<_>>();
        let weighted = matched.iter().map(|term| weights.of(term)).sum::<f64>();
        let base = if route.exact {
            ROUTE_EXACT
        } else if matched.len() == terms.len() {
            ROUTE_COVERED
        } else {
            ROUTE_PARTIAL
        };
        self.insert(Scored {
            matched,
            symbol,
            score: base + weighted * NAME_TERM,
            decisive: route.exact,
        });
    }

    pub fn add_symbol(
        &mut self,
        symbol: SymbolRow,
        terms: &[String],
        question_key: &str,
        weights: &TermWeights,
        search_rank: Option<f64>,
    ) {
        let name_tokens = identifier_words(&symbol.name)
            .into_iter()
            .collect::<HashSet<_>>();
        let parent_tokens = symbol
            .parent
            .as_deref()
            .map(words)
            .unwrap_or_default()
            .into_iter()
            .collect::<HashSet<_>>();
        let path_tokens = words(&symbol.path);
        let prose_tokens = words(&format!(
            "{} {}",
            symbol.doc.as_deref().unwrap_or_default(),
            symbol.signature
        ));
        let mut matched = Vec::new();
        let mut score = 0.0;
        for term in terms {
            let mut best = 0.0_f64;
            if name_tokens.contains(term) {
                best = best.max(NAME_TERM);
            }
            if parent_tokens.contains(term) {
                best = best.max(PARENT_TERM);
            }
            if prose_tokens.contains(term) {
                best = best.max(DOC_TERM);
            }
            if path_tokens.contains(term) {
                best = best.max(PATH_TERM);
            }
            if best > 0.0 {
                matched.push(term.clone());
                score += best * weights.of(term);
            }
        }
        if matched.is_empty() {
            return;
        }
        let key = name_key(&symbol.name);
        let exact = !key.is_empty() && key == question_key;
        if exact {
            score += NAME_EXACT;
        }
        if key
            .split_whitespace()
            .all(|token| terms.iter().any(|term| term == token))
        {
            score += NAME_COVERED;
        }
        if symbol.exported {
            score += EXPORTED;
        }
        score += search_rank.unwrap_or_default().clamp(0.0, SEARCH_CEILING) * SEARCH_WEIGHT;
        self.insert(Scored {
            score: score * support_weight(&symbol.path) * kind_weight(symbol.kind),
            symbol,
            matched,
            decisive: exact,
        });
    }

    fn insert(&mut self, candidate: Scored) {
        let anchor = format!("{}#{}", candidate.symbol.path, candidate.symbol.name);
        match self.scored.get(&anchor) {
            Some(existing) if existing.score >= candidate.score => {}
            _ => {
                self.scored.insert(anchor, candidate);
            }
        }
    }

    /// Best first. A score tie goes to whichever matched more of the
    /// question, then to path, so the same question keeps answering the
    /// same way instead of turning on an arbitrary tiebreak.
    pub fn ranked(self) -> Vec<Scored> {
        let mut ranked = self.scored.into_values().collect::<Vec<_>>();
        ranked.sort_by(|left, right| {
            right
                .score
                .partial_cmp(&left.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| right.matched.len().cmp(&left.matched.len()))
                .then_with(|| left.symbol.path.cmp(&right.symbol.path))
                .then_with(|| left.symbol.line.cmp(&right.symbol.line))
        });
        ranked.truncate(CANDIDATE_POOL);
        ranked
    }
}

/// Whether the top answer is worth returning on its own. Judge the candidates
/// the caller can actually read, after scope and disk have had their say: a
/// sure answer that was filtered out cannot lend its certainty to whatever is
/// left.
///
/// A named place, a taught phrase, or an exact name settles it, unless a
/// second candidate claims the same thing — two symbols of that name is two
/// answers. Otherwise the question has to have landed whole, and the
/// runner-up has to be well behind.
pub fn is_confident(ranked: &[Scored], terms: &[String]) -> bool {
    let Some(top) = ranked.first() else {
        return false;
    };
    let next = ranked.get(1);
    if top.decisive {
        return next.is_none_or(|next| !next.decisive);
    }
    let complete = terms
        .iter()
        .all(|term| top.matched.iter().any(|hit| hit == term));
    complete && next.is_none_or(|next| top.score >= next.score * 1.8)
}

/// The place a question named, as an answer. Sure of itself only when the
/// path resolved to one file and held what was asked for: two files ending
/// the same way are two answers, not one.
pub fn direct_hit(symbol: SymbolRow, sure: bool) -> Scored {
    Scored {
        symbol,
        score: DIRECT,
        matched: Vec::new(),
        decisive: sure,
    }
}

/// A question that names a place instead of describing one: `path`,
/// `path#symbol`, or `path:line`.
#[derive(Debug, Clone, Copy)]
pub struct DirectTarget<'a> {
    pub path: &'a str,
    pub symbol: Option<&'a str>,
}

/// Read a question as the place it names, if it names one. A single token that
/// carries a directory or a file extension is a path someone copied, not a
/// description of one.
pub fn direct_target(question: &str) -> Option<DirectTarget<'_>> {
    let question = question.trim();
    if question.is_empty() || question.split_whitespace().count() > 1 {
        return None;
    }
    let (head, symbol) = match question.split_once('#') {
        Some((head, named)) => (head, Some(named.trim()).filter(|named| !named.is_empty())),
        None => (question, None),
    };
    let head = head
        .rsplit_once(':')
        .filter(|(_, line)| !line.is_empty() && line.chars().all(|char| char.is_ascii_digit()))
        .map_or(head, |(path, _)| path);
    let path = head.trim_end_matches('/');
    let named = path.contains('/') || Path::new(path).extension().is_some();
    (named && !path.is_empty()).then_some(DirectTarget { path, symbol })
}

/// A field or a variant answers a question about the type that holds it, not
/// the other way round.
fn kind_weight(kind: SymbolKind) -> f64 {
    match kind {
        SymbolKind::Field | SymbolKind::Variant => 0.75,
        SymbolKind::Impl => 0.9,
        _ => 1.0,
    }
}

/// Fixtures, snapshots, and test doubles exist to hold code still, not to be
/// found. They stay reachable, just behind the real thing.
fn support_weight(path: &str) -> f64 {
    let lower = path.to_ascii_lowercase();
    let support = lower.split('/').any(|part| {
        [
            "test", "tests", "fixture", "fixtures", "mock", "mocks", "examples",
        ]
        .contains(&part)
    }) || lower.contains(".test.")
        || lower.contains(".spec.");
    if support { 0.4 } else { 1.0 }
}
