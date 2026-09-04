//! Project context maps, source symbols, FTS ranking, scope filtering, and
//! learned source routes.

mod index;
mod symbols;
mod text;
mod walk;

pub use index::{
    BUILD_BUDGET, BuildOptions, BuildResult, ContextError, ContextIndex, ContextResult,
    ContextTarget, LearnRouteProposal, LearnRouteRejection, LearnRoutesResult, MAP_SCHEME,
    MAX_BUILD_FILES, MAX_FILE_BYTES, MAX_SYMBOLS_PER_CWD, MAX_SYMBOLS_PER_FILE, QueryOptions,
    QuestionCandidate, QuestionOptions, ReconcileResult, RenderTier, RouteMove,
    WorktreeVerification,
};
pub use symbols::{ExtractedFile, ExtractedSymbol, extract_refs, extract_symbols};
pub use text::{
    FTS_STOP_WORDS, MAP_STOP_WORDS, clean_comment, fts_query, hint_key, identifier_tokens,
    normalize_word, prompt_terms, raw_words, words,
};
pub use walk::{
    ContextWalkFile, GenericLanguage, LanguageEntry, WalkOptions, WalkResult, mapped_extension,
    mtime_ms, walk_context_files,
};

/// Build a project map through the supplied store.
pub fn build_context_map(
    store: &oga_store::Store,
    cwd: impl AsRef<std::path::Path>,
    options: BuildOptions,
) -> Result<BuildResult, ContextError> {
    ContextIndex::new(store).build(cwd, options)
}

/// Reconcile only files whose metadata changed, preserving entity identities.
pub fn reconcile_context_map(
    store: &oga_store::Store,
    cwd: impl AsRef<std::path::Path>,
    options: BuildOptions,
) -> Result<ReconcileResult, ContextError> {
    ContextIndex::new(store).reconcile(cwd, options)
}
