//! Estimates provider spend from public model pricing (models.dev) when a
//! provider reports token counts but no dollar amount.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{
        OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use serde::Deserialize;
use tokio::sync::Mutex;

const CATALOGUE_URL: &str = "https://models.dev/api.json";
const CACHE_TTL: Duration = Duration::from_secs(24 * 60 * 60);
const CACHE_FILE: &str = "models-dev-pricing.json";

/// USD per one million tokens, as models.dev publishes it.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ModelRate {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
}

/// A model entry published by models.dev.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogModel {
    pub id: String,
    pub name: String,
}

/// Model pricing indexed for lookup, built once per fetch from `api.json`.
#[derive(Debug, Clone, Default)]
pub struct PricingCatalogue {
    /// Keyed `"<models.dev provider>/<model id>"`, for ids that already carry
    /// a provider prefix (`opencode/big-pickle`, `opencode-go/deepseek-v4-flash`).
    by_qualified: BTreeMap<String, ModelRate>,
    /// Keyed by bare model id, for matching across providers when no prefix
    /// is present or the prefix isn't a models.dev provider key. First
    /// provider found in the catalogue wins ties.
    by_bare: BTreeMap<String, ModelRate>,
    models: BTreeMap<String, Vec<CatalogModel>>,
}

#[derive(Deserialize)]
struct RawCatalogue(BTreeMap<String, RawProvider>);

#[derive(Deserialize)]
struct RawProvider {
    #[serde(default)]
    models: BTreeMap<String, RawModel>,
}

#[derive(Deserialize)]
struct RawModel {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    cost: Option<RawCost>,
}

#[derive(Deserialize)]
struct RawCost {
    #[serde(default)]
    input: f64,
    #[serde(default)]
    output: f64,
    #[serde(default)]
    cache_read: f64,
    #[serde(default)]
    cache_write: f64,
}

/// Parses one `api.json` payload into a lookup-ready catalogue. Models with
/// no published `cost` (free or unmetered) are skipped rather than priced at
/// zero, so an unpriced model still counts as unknown.
pub fn parse_catalogue(raw: &str) -> PricingCatalogue {
    let Ok(RawCatalogue(providers)) = serde_json::from_str(raw) else {
        return PricingCatalogue::default();
    };
    let mut catalogue = PricingCatalogue::default();
    for (provider_id, provider) in providers {
        for (model_id, model) in provider.models {
            catalogue
                .models
                .entry(provider_id.clone())
                .or_default()
                .push(CatalogModel {
                    id: model_id.clone(),
                    name: model.name.unwrap_or_else(|| model_id.clone()),
                });
            let Some(cost) = model.cost else { continue };
            let rate = ModelRate {
                input: cost.input,
                output: cost.output,
                cache_read: cost.cache_read,
                cache_write: cost.cache_write,
            };
            catalogue
                .by_qualified
                .insert(format!("{provider_id}/{model_id}"), rate);
            catalogue.by_bare.entry(model_id).or_insert(rate);
        }
    }
    catalogue
}

impl PricingCatalogue {
    /// Returns all models for a models.dev provider, including unpriced ones.
    pub fn models_for_provider(&self, provider: &str) -> &[CatalogModel] {
        self.models.get(provider).map(Vec::as_slice).unwrap_or(&[])
    }
}

fn normalize(id: &str) -> String {
    id.trim().to_lowercase()
}

const EFFORT_SUFFIXES: [&str; 4] = ["-low", "-medium", "-high", "-thinking"];

const MODEL_ALIASES: [(&str, &str, &str); 5] = [
    ("gemini-3.6-flash", "google", "gemini-3.6-flash"),
    ("gemini-3.1-pro", "google", "gemini-3.1-pro-preview"),
    ("claude-opus-4-6", "anthropic", "claude-opus-4-6"),
    ("claude-sonnet-4-6", "anthropic", "claude-sonnet-4-6"),
    ("gpt-oss-120b", "openai", "gpt-oss-120b"),
];

fn antigravity_catalog_key(model: &str) -> Option<String> {
    let mut bare = normalize(model);
    if let Some(suffix) = EFFORT_SUFFIXES
        .iter()
        .find(|suffix| bare.ends_with(**suffix))
    {
        bare.truncate(bare.len() - suffix.len());
    }
    MODEL_ALIASES
        .iter()
        .find(|(alias, _, _)| *alias == bare)
        .map(|(_, provider, catalog_model)| format!("{provider}/{catalog_model}"))
}

/// Looks up a rate for `model` as dispatched under `provider_id`. Tries the
/// model id as a `<provider-prefix>/<bare-id>` pair first — our own provider
/// name, or a prefix the model id already carries (`opencode/big-pickle`) —
/// then falls back to the bare id across every provider models.dev knows,
/// since our provider names rarely match models.dev's own (Codex runs OpenAI
/// models, Antigravity runs Google's, Claude runs Anthropic's).
pub fn rate_for<'a>(
    catalogue: &'a PricingCatalogue,
    provider_id: &str,
    model: &str,
) -> Option<&'a ModelRate> {
    let model = normalize(model);
    if normalize(provider_id) == "antigravity"
        && let Some(key) = antigravity_catalog_key(&model)
    {
        return catalogue.by_qualified.get(&key);
    }
    let (prefix, bare) = match model.split_once('/') {
        Some((prefix, bare)) => (prefix.to_owned(), bare.to_owned()),
        None => (normalize(provider_id), model.clone()),
    };
    catalogue
        .by_qualified
        .get(&format!("{prefix}/{bare}"))
        .or_else(|| catalogue.by_bare.get(&bare))
}

/// `tokens_in` excludes cached reads (matching `oga_providers::Usage`);
/// cached reads price at `cache_read`, everything else at `input`/`output`.
/// `cache_write` has no counterpart in `Usage` today, so it never applies.
pub fn estimate_usd(rate: &ModelRate, tokens_in: f64, tokens_out: f64, cached_tokens: f64) -> f64 {
    tokens_in.max(0.0) * rate.input / 1_000_000.0
        + tokens_out.max(0.0) * rate.output / 1_000_000.0
        + cached_tokens.max(0.0) * rate.cache_read / 1_000_000.0
}

struct CachedCatalogue {
    /// `None` for a copy loaded from disk, whose age is unknown and which is
    /// therefore always due for a refresh.
    fetched_at: Option<Instant>,
    catalogue: PricingCatalogue,
}

impl CachedCatalogue {
    fn is_fresh(&self) -> bool {
        self.fetched_at
            .is_some_and(|fetched_at| fetched_at.elapsed() < CACHE_TTL)
    }
}

static MEMORY_CACHE: OnceLock<Mutex<Option<CachedCatalogue>>> = OnceLock::new();

fn memory_cache() -> &'static Mutex<Option<CachedCatalogue>> {
    MEMORY_CACHE.get_or_init(|| Mutex::new(None))
}

fn disk_cache_path() -> PathBuf {
    oga_config::global_cwd()
        .join(".oga")
        .join("cache")
        .join(CACHE_FILE)
}

async fn read_disk_cache(path: &Path) -> Option<String> {
    tokio::fs::read_to_string(path).await.ok()
}

async fn write_disk_cache(path: &Path, raw: &str) {
    if let Some(parent) = path.parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }
    let _ = tokio::fs::write(path, raw).await;
}

async fn fetch_catalogue_raw() -> Option<String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .ok()?;
    let response = client.get(CATALOGUE_URL).send().await.ok()?;
    response.text().await.ok()
}

static REFRESH_IN_FLIGHT: AtomicBool = AtomicBool::new(false);

async fn store_in_memory(catalogue: PricingCatalogue, fetched_at: Option<Instant>) {
    let mut guard = memory_cache().lock().await;
    *guard = Some(CachedCatalogue {
        fetched_at,
        catalogue,
    });
}

/// Fetches models.dev once, writing the payload to disk and memory. At most
/// one refresh runs at a time; extra calls while one is in flight are no-ops.
async fn refresh_from_network() {
    if REFRESH_IN_FLIGHT.swap(true, Ordering::AcqRel) {
        return;
    }
    if let Some(raw) = fetch_catalogue_raw().await {
        write_disk_cache(&disk_cache_path(), &raw).await;
        store_in_memory(parse_catalogue(&raw), Some(Instant::now())).await;
    }
    REFRESH_IN_FLIGHT.store(false, Ordering::Release);
}

/// The current pricing catalogue, stale-while-revalidate: a fresh memory
/// copy is returned as is; a stale one is returned immediately while a
/// background task refetches. With nothing in memory, the disk copy —
/// however old — is served the same way. Only the very first run with no
/// disk cache waits on the network. Never called on a per-event hot path;
/// only when a run settles with token counts but no reported amount.
pub async fn catalogue() -> Option<PricingCatalogue> {
    {
        let guard = memory_cache().lock().await;
        if let Some(cached) = guard.as_ref() {
            if !cached.is_fresh() {
                tokio::spawn(refresh_from_network());
            }
            return Some(cached.catalogue.clone());
        }
    }
    if let Some(raw) = read_disk_cache(&disk_cache_path()).await {
        let parsed = parse_catalogue(&raw);
        store_in_memory(parsed.clone(), None).await;
        tokio::spawn(refresh_from_network());
        return Some(parsed);
    }
    refresh_from_network().await;
    let guard = memory_cache().lock().await;
    guard.as_ref().map(|cached| cached.catalogue.clone())
}

/// Prices `tokens_in`/`tokens_out`/`cached_tokens` for `model` under
/// `provider_id` using public pricing. `None` when the catalogue can't be
/// loaded or the model isn't in it — callers should log that once at debug
/// and move on, never fail the event over an unpriced model.
pub async fn estimate(
    provider_id: &str,
    model: &str,
    tokens_in: f64,
    tokens_out: f64,
    cached_tokens: f64,
) -> Option<f64> {
    let catalogue = catalogue().await?;
    let rate = rate_for(&catalogue, provider_id, model)?;
    Some(estimate_usd(rate, tokens_in, tokens_out, cached_tokens))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
        "anthropic": {
            "models": {
                "claude-opus-4-7": {
                    "name": "Claude Opus 4.7",
                    "cost": {"input": 5, "output": 25, "cache_read": 0.5, "cache_write": 6.25}
                },
                "claude-free": {}
            }
        },
        "opencode-go": {
            "models": {
                "deepseek-v4-flash": {
                    "cost": {"input": 0.3, "output": 1.2, "cache_read": 0.03, "cache_write": 0.3}
                }
            }
        }
    }"#;

    #[test]
    fn parses_cost_and_skips_free_models() {
        let catalogue = parse_catalogue(SAMPLE);
        assert!(rate_for(&catalogue, "anthropic", "claude-opus-4-7").is_some());
        assert!(rate_for(&catalogue, "anthropic", "claude-free").is_none());
        assert_eq!(
            catalogue
                .models_for_provider("anthropic")
                .iter()
                .find(|model| model.id == "claude-opus-4-7")
                .expect("model")
                .name,
            "Claude Opus 4.7"
        );
    }

    #[test]
    fn matches_qualified_id_first() {
        let catalogue = parse_catalogue(SAMPLE);
        let rate = rate_for(&catalogue, "pi", "opencode-go/deepseek-v4-flash").expect("rate");
        assert_eq!(rate.input, 0.3);
    }

    #[test]
    fn falls_back_to_bare_id_across_providers() {
        let catalogue = parse_catalogue(SAMPLE);
        // "claude" (our provider id) isn't a models.dev provider key, so the
        // qualified lookup misses and the bare id must still resolve.
        let rate = rate_for(&catalogue, "claude", "claude-opus-4-7").expect("rate");
        assert_eq!(rate.output, 25.0);
    }

    #[test]
    fn unknown_model_yields_none() {
        let catalogue = parse_catalogue(SAMPLE);
        assert!(rate_for(&catalogue, "codex", "gpt-5-nonexistent").is_none());
    }

    #[test]
    fn estimate_prices_cached_reads_separately_from_input() {
        let rate = ModelRate {
            input: 5.0,
            output: 25.0,
            cache_read: 0.5,
            cache_write: 6.25,
        };
        let usd = estimate_usd(&rate, 1_000_000.0, 1_000_000.0, 1_000_000.0);
        assert_eq!(usd, 5.0 + 25.0 + 0.5);
    }

    #[test]
    fn malformed_json_yields_empty_catalogue() {
        let catalogue = parse_catalogue("not json");
        assert!(rate_for(&catalogue, "anthropic", "claude-opus-4-7").is_none());
    }

    #[test]
    fn maps_antigravity_models_to_catalogue_ids() {
        assert_eq!(
            antigravity_catalog_key("gemini-3.1-pro-high"),
            Some("google/gemini-3.1-pro-preview".into())
        );
        assert_eq!(
            antigravity_catalog_key("claude-opus-4-6-thinking"),
            Some("anthropic/claude-opus-4-6".into())
        );
    }

    #[test]
    fn prices_antigravity_thinking_and_cache_tokens() {
        let catalogue = parse_catalogue(
            r#"{"google":{"models":{"gemini-3.6-flash":{"cost":{"input":1,"output":3,"cache_read":0.1}}}}}"#,
        );
        let rate = rate_for(&catalogue, "antigravity", "gemini-3.6-flash-medium").expect("rate");
        assert!((estimate_usd(rate, 10.0, 20.0, 30.0) - 0.000073).abs() < f64::EPSILON);
    }
}
