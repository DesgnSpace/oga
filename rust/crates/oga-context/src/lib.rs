//! The project code index: tree-sitter parsing behind `oga query` and
//! `oga relearn`.
//!
//! `lang` holds one adapter per language, `symbols` runs an adapter's query
//! over a parsed tree, `walk` decides which files are candidates, `store`
//! reads and writes SQLite, `query` ranks an answer, `routes` keeps the hints
//! people taught the project, and `index` ties them together.

mod index;
mod lang;
mod query;
mod routes;
mod store;
mod symbols;
mod text;
mod walk;

pub use index::{
    BuildOptions, BuildResult, ContextError, ContextIndex, ContextResult, ContextTarget,
    LearnRouteProposal, LearnRouteRejection, LearnRoutesResult, QuestionCandidate, QuestionOptions,
    ReconcileResult,
};
pub use lang::{LanguageAdapter, adapters};
pub use routes::RouteMove;
pub use symbols::{ExtractedFile, ExtractedSymbol, extract_symbols};
