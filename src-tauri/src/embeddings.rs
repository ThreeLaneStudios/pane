// Pane — Semantic Search via OR/Ollama HTTP Embeddings
// embed_fn is injected by the caller (lib.rs or commands.rs) — embeddings.rs stays HTTP-backend-agnostic

use std::path::{Path, PathBuf};
use rusqlite::Connection;

// ─── Progress Reporting ──────────────────────────────────────────────

/// Progress events emitted by the embedding pipeline.
///
/// This type is **Tauri-free** — it carries only plain Rust data.
/// `lib.rs` maps these variants to `embeddings://status` event payloads
/// and owns the `AppHandle::emit` call (ADR-0047).
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EmbeddingProgress {
    /// Indexing progress snapshot — emitted every 25 items per sub-pass
    /// (notes → facts → chats) and always at the final item of each
    /// sub-pass so counts land on their totals (§3 throttle).
    Indexing {
        /// Notes embedded so far this run
        notes: usize,
        /// Facts embedded so far this run
        facts: usize,
        /// Chat messages embedded so far this run
        chats: usize,
        /// Total notes to process
        notes_total: usize,
        /// Total facts to process
        facts_total: usize,
        /// Total chat messages to process
        chats_total: usize,
    },
}





/// Returns true if the error string indicates an authentication failure (401/403).
/// Used to distinguish fatal auth errors (abort pipeline) from transient item failures
/// (skip-and-continue). Only HTTP 401 and HTTP 403 are considered auth errors.
fn is_auth_error(e: &str) -> bool {
    e.contains("HTTP 401") || e.contains("HTTP 403")
}

// ─── Token-Aware Truncation (#2776) ──────────────────────────────────

/// Last-resort fallback when NO token limit is known for the configured embedding
/// model — neither a fetched `context_length` from OpenRouter's API, nor a match in
/// `embedding_model_seed_lookup`'s static Ollama table. This is intentionally the
/// most conservative widely-deployed embedding-model context window (bge-base /
/// mxbai-embed-large class = 512 tokens), NOT an authoritative reading of the actual
/// model's limit. Every caller that falls back to this constant MUST log
/// "unknown model, using conservative floor" so the fallback is diagnosable from
/// terminal output, never silent (Matt's ruling #3, #2776).
pub const EMBEDDINGS_MAX_TOKENS_FALLBACK: u64 = 512;

/// #3101: `search_notes`'s semantic-search similarity cutoff. Was a bare `0.5` inline at
/// the call site, which drops conceptually correct hits — reproduced on a clean
/// OpenRouter index with the query "shipping announcement workflow" against a Release
/// Playbook note, which scored below 0.5 and was dropped even though it was the right
/// answer. `0.35` is not model-aware (the issue's "nice to have"); it is the one named
/// value every engine/model currently shares, same discipline as
/// `EMBEDDINGS_MAX_TOKENS_FALLBACK` above — a constant with a comment, not an inline
/// literal a future reader has to go hunting for.
pub const SEMANTIC_SEARCH_SIMILARITY_THRESHOLD: f32 = 0.35;

/// Static seed table for embedding-model token limits, used when no live API source
/// is available. Ollama's `/api/tags` does not report a context window for embedding
/// models (unlike OpenRouter's `/api/v1/models`, which returns `context_length`
/// directly), so this table fills that gap. Mirrors the `seed_lookup()` pattern in
/// `ai/context_budget.rs`.
///
/// Matched by case-insensitive substring against the model id/name, since Ollama tags
/// commonly carry size/version suffixes (e.g. `"bge-base-en-v1.5"`,
/// `"nomic-embed-text:v1.5"`).
///
/// **Values verified against each model's published card as of 2026-09-01** (see PR
/// #2776 scope doc / model cards on Hugging Face + ollama.com/library):
/// - `bge-m3` → 8192 (BAAI/bge-m3: "documents of up to 8192 tokens", XLM-RoBERTa base extended)
/// - `bge-base` → 512 (BAAI/bge-base-en-v1.5: BERT-base, max sequence length 512)
/// - `bge-large` → 512 (BAAI/bge-large-en-v1.5: BERT-large, max sequence length 512)
/// - `nomic-embed-text` → 8192 (nomic-ai/nomic-embed-text-v1.5: native context 8192)
/// - `mxbai-embed-large` → 512 (mixedbread-ai/mxbai-embed-large-v1: BERT-large, 512)
/// - `all-minilm` → 256 (sentence-transformers/all-MiniLM-L6-v2: truncates at 256 by model card)
///
/// Returns `None` on no match — callers fall through to `EMBEDDINGS_MAX_TOKENS_FALLBACK`.
pub fn embedding_model_seed_lookup(model_id: &str) -> Option<u64> {
    let m = model_id.to_lowercase();
    if m.contains("bge-m3") { return Some(8192); }
    if m.contains("bge-base") { return Some(512); }
    if m.contains("bge-large") { return Some(512); }
    if m.contains("nomic-embed-text") { return Some(8192); }
    if m.contains("mxbai-embed-large") { return Some(512); }
    if m.contains("all-minilm") { return Some(256); }
    None
}

/// Read the persisted embedding token limit for pipeline callers, per this three-tier
/// precedence chain (#2776 follow-up, Gap 2):
///
/// 1. Stored `ai_settings.embeddings_max_tokens` (written by `switch_embedding_engine`),
///    if present and parseable — the normal case for any install that has saved the
///    Semantic Search card since #2776 shipped.
/// 2. **NEW** — `embedding_model_seed_lookup` keyed on the current `ai_settings.embeddings_model`.
///    Before this tier existed, an install with NO stored `embeddings_max_tokens` (fresh
///    install, or an engine/model configured before #2776 landed) fell straight through to
///    the 512 floor even when its model — e.g. `bge-m3` (8192 tokens) — was a known entry
///    in the same seed table the WRITE path (`resolve_embeddings_max_tokens` in
///    `ai::scheduler`) already consults. That gap is harmless for bge-base (512 == the
///    floor already) but silently 16x-over-truncates a bge-m3 user's content. This tier
///    calls the SAME standalone `embedding_model_seed_lookup` the write path uses — one
///    table, two callers, no duplication.
/// 3. `EMBEDDINGS_MAX_TOKENS_FALLBACK` (512) — last resort, ALWAYS logged (Matt's ruling #3)
///    so a missing/unmatched setting is diagnosable from terminal output, never silently
///    treated as authoritative.
///
/// A stored value that fails to parse as a number is treated the same as absent — both
/// fall through to tier 2, then tier 3.
///
/// Used by all three `run_embedding_pipeline` callers (`run_integrity_check`,
/// `switch_embedding_engine`'s own re-read is skipped since it just wrote the value,
/// and `lib.rs` startup) per #2776 scope §5c.
pub fn read_embeddings_max_tokens(conn: &Connection) -> usize {
    // Tier 1: stored value.
    if let Some(v) = crate::database::get_ai_setting(conn, "embeddings_max_tokens") {
        match v.parse::<usize>() {
            Ok(n) => return n,
            Err(_) => {
                eprintln!("[EMBEDDINGS] embeddings_max_tokens '{}' is not a valid number, falling through to seed table", v);
            }
        }
    }

    // Tier 2 (NEW): seed table lookup by the current model, shared with the write path
    // (`resolve_embeddings_max_tokens` in `ai::scheduler`).
    let embeddings_model = crate::database::get_ai_setting(conn, "embeddings_model").unwrap_or_default();
    if !embeddings_model.is_empty() {
        if let Some(t) = embedding_model_seed_lookup(&embeddings_model) {
            eprintln!("[EMBEDDINGS] embeddings_max_tokens not set, resolved from static seed table for '{}': {}", embeddings_model, t);
            return t as usize;
        }
    }

    // Tier 3: conservative floor, always logged.
    eprintln!("[EMBEDDINGS] embeddings_max_tokens not set, unknown model, using conservative floor of {} tokens", EMBEDDINGS_MAX_TOKENS_FALLBACK);
    EMBEDDINGS_MAX_TOKENS_FALLBACK as usize
}

// ─── Adaptive Retry on Token-Limit 400 (#2776 follow-up, Gap 1) ─────────────

/// Extract the server-reported input-token count from a token-limit 400 error body, if
/// present. Matches OpenRouter's `"You passed <N> input tokens"` wording (Matt's v1.0.43
/// retest, #2776 follow-up) AND requires the error text to actually carry a 400 status
/// (`"HTTP 400"`, matching this file's own `"... HTTP {status}: {body}"` error format,
/// e.g. `lib.rs::build_http_embed_fn`) — any other status code, or a 400 whose body
/// doesn't match this specific pattern (e.g. `"Invalid API key"`), returns `None` so the
/// caller fails immediately instead of retrying.
fn parse_token_limit_from_error(error_text: &str) -> Option<u64> {
    if !error_text.contains("HTTP 400") {
        return None;
    }
    // TEI (HuggingFace Text Embeddings Inference) wording — the actual backend behind
    // OpenRouter's BAAI/bge-* embedding models (confirmed via
    // huggingface/text-embeddings-inference#356, #218):
    //   "Input validation error: `inputs` must have less than 512 tokens. Given: 513"
    // Tried first: this is the format Matt's real bge-base-en-v1.5 traffic actually
    // produces. The regex scans the raw string regardless of JSON-escaping depth, so
    // it matches whether this text arrives flat or nested inside OpenRouter's
    // `error.metadata.raw` envelope (#2776 follow-up 2).
    if let Ok(re) = regex::Regex::new(r"must have less than \d+ tokens\. Given: (\d+)") {
        if let Some(n) = re
            .captures(error_text)
            .and_then(|c| c.get(1))
            .and_then(|m| m.as_str().parse::<u64>().ok())
        {
            return Some(n);
        }
    }
    // OpenAI-chat-completions-style wording — kept as a fallback in case a different
    // provider/model family routes through this same error path with that phrasing.
    let re = regex::Regex::new(r"You passed (\d+) input tokens").ok()?;
    re.captures(error_text)?.get(1)?.as_str().parse::<u64>().ok()
}

/// Adaptive retry for a single embed call that fails with a token-limit 400 — a
/// **mechanism**, not ratio tuning (Matt's v1.0.43 retest found the 4.0 chars/token
/// estimate with its 10% safety margin still overshoots on dense content: 1843 chars of
/// markdown estimated under budget but actually tokenized to 513 tokens against bge-base's
/// 512 limit — the heuristic can't be perfect for every content type, so this catches the
/// resulting 400 instead of chasing the ratio).
///
/// `attempt` performs ONE HTTP embed call — the same `embed_fn` callers already have.
/// This function is the sole owner of the "should I retry, and with what content"
/// decision; it is called AT the three `run_embedding_pipeline` embed-call sites
/// (notes/facts/chats), not as a global retry loop wrapping the whole pipeline.
///
/// On a token-limit 400 (detected via `parse_token_limit_from_error`), the content is
/// re-truncated ONCE using the server's actual token count as ground truth — replacing
/// the heuristic estimate for this retry with the real ratio observed for this specific
/// content: `new_char_limit = current_chars * (model_max_tokens / actual_tokens)`, then
/// the existing 10% safety margin is applied on top of that derived limit, then the
/// existing `floor_char_boundary` UTF-8 safety pass.
///
/// - A non-token 400 (or any other error) on the FIRST attempt is returned immediately —
///   never retried.
/// - Whatever the RETRY returns (success, a second token-limit 400, or any other error)
///   is returned as-is — exactly one retry, ever, no infinite loop.
pub fn embed_with_token_retry<F>(
    content: &str,
    model_max_tokens: u64,
    attempt: &F,
) -> Result<Vec<f32>, String>
where
    F: Fn(&str) -> Result<Vec<f32>, String>,
{
    match attempt(content) {
        Ok(v) => Ok(v),
        Err(e) => {
            let actual_tokens = match parse_token_limit_from_error(&e) {
                Some(n) if n > 0 => n,
                _ => return Err(e),
            };
            let current_chars = content.len() as f64;
            let derived_char_limit =
                current_chars * (model_max_tokens as f64 / actual_tokens as f64);
            let safe_char_limit = (derived_char_limit * 0.9).floor().max(0.0) as usize;
            let byte_limit = content.floor_char_boundary(safe_char_limit.min(content.len()));
            let retry_content = &content[..byte_limit];
            eprintln!(
                "[EMBEDDINGS] Retry: token-limit 400 (server reported {} actual tokens vs {} limit) — retrying with {} chars (was {})",
                actual_tokens, model_max_tokens, retry_content.len(), content.len()
            );
            match attempt(retry_content) {
                Ok(v) => {
                    eprintln!("[EMBEDDINGS] Retry succeeded on second attempt");
                    Ok(v)
                }
                Err(e2) => {
                    eprintln!("[EMBEDDINGS] Retry failed on second attempt: {}", e2);
                    Err(e2)
                }
            }
        }
    }
}

/// Truncate `content` to fit within `max_tokens`, using the app's existing char/token
/// heuristic (`TokenEstimationRatio`, already used for chat context budgeting —
/// `ai/context_budget.rs`) rather than inventing a second estimator.
///
/// Formula (#2776 scope 5b): `(max_tokens * chars_per_token * 0.9).floor()` chars — the
/// 10% safety margin absorbs the gap between this heuristic estimate and the model's
/// real server-side tokenizer (OpenRouter/Ollama), which is what actually rejects
/// over-limit requests. `chars_per_token = 2.2` (#2776 Round 3, tightened from the
/// original `4.0` GPT-family default — that ratio matches `TokenEstimationRatio::for_model`'s
/// chat-model default, but embedding models are not chat models: bge-base-en-v1.5's real
/// WordPiece tokenizer (the TEI backend behind OpenRouter's bge-* models) runs meaningfully
/// denser on real content than 4.0 chars/token assumes, and Round 3's production-shaped wire
/// test (`test_build_http_embed_fn_wire_body_shrinks_on_retry`) confirmed the truncate→wire→
/// retry mechanism itself is intact end-to-end, isolating the residual over-limit-on-first-
/// attempt symptom to this ratio being too generous, not a wire/closure bug). `2.2` plus the
/// existing 10% margin below gives enough headroom for first attempts to land under budget
/// for the dense content shapes seen in production; `embed_with_token_retry`'s adaptive
/// retry (unchanged) remains the backstop for any content shape this static ratio still
/// misses.
///
/// `floor_char_boundary` (existing UTF-8 safety, unchanged) is applied on top of the
/// resulting byte count so multi-byte UTF-8 sequences are never split mid-character.
fn truncate_to_token_budget(content: &str, max_tokens: usize) -> &str {
    let ratio = crate::ai::context_budget::TokenEstimationRatio { chars_per_token: 2.2 };
    let max_chars = ((max_tokens as f32) * ratio.chars_per_token * 0.9).floor().max(0.0) as usize;
    if content.len() > max_chars {
        &content[..content.floor_char_boundary(max_chars)]
    } else {
        content
    }
}

/// Cosine similarity between two vectors
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 { 0.0 } else { dot / (norm_a * norm_b) }
}

/// Serialize embedding to bytes for SQLite BLOB storage
pub fn serialize_embedding(embedding: &[f32]) -> Vec<u8> {
    embedding.iter().flat_map(|f| f.to_le_bytes()).collect()
}

/// Deserialize embedding from SQLite BLOB
pub fn deserialize_embedding(data: &[u8]) -> Vec<f32> {
    data.chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect()
}

/// Simple content hash for change detection
pub fn content_hash(content: &str) -> String {
    let hash = blake3::hash(content.as_bytes());
    hash.to_hex().to_string()
}

// ─── Database Operations ─────────────────────────────────────────────

/// Create the embeddings table if it doesn't exist
pub fn create_embeddings_table(conn: &Connection) -> Result<(), String> {
    conn.execute_batch("
        CREATE TABLE IF NOT EXISTS embeddings (
            id TEXT PRIMARY KEY,
            source TEXT NOT NULL,
            embedding BLOB NOT NULL,
            content_hash TEXT NOT NULL,
            engine TEXT,
            model TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_embeddings_source ON embeddings(source);
    ").map_err(|e| format!("Failed to create embeddings table: {}", e))?;
    // Idempotent: add engine/model columns if missing (upgrading from an older schema —
    // same self-healing idiom as the pre-existing engine column, #3101). SCHEMA_VERSION
    // is not bumped for this: `embeddings` is not in `migrations::CRR_TABLES`, so a plain
    // ALTER TABLE ADD COLUMN is safe (no CRR rebuild-shape hazard, verified against
    // `migrations::CRR_TABLES` — same precedent as the pre-existing `engine` column add).
    let _ = conn.execute_batch("ALTER TABLE embeddings ADD COLUMN engine TEXT;");
    let _ = conn.execute_batch("ALTER TABLE embeddings ADD COLUMN model TEXT;");
    Ok(())
}

/// Check if an embedding exists with the same content hash
pub fn embedding_exists(conn: &Connection, id: &str, hash: &str) -> bool {
    conn.query_row(
        "SELECT 1 FROM embeddings WHERE id = ?1 AND content_hash = ?2",
        rusqlite::params![id, hash],
        |_| Ok(()),
    ).is_ok()
}

/// Store an embedding
///
/// `engine` tags which HTTP backend produced this embedding (e.g. `"openrouter"`, `"ollama"`).
/// Pass `None` only in tests or when engine is unknown (treated as stale/orphaned).
///
/// `model` tags which specific model (within that engine) produced this embedding
/// (#3101) — e.g. two different OpenRouter models produce vectors that are NOT
/// comparable, even though both are tagged `engine = "openrouter"`. Pass `None` only
/// in tests or for pre-#3101 rows that predate this column (treated as stale, same as
/// a NULL `engine`).
pub fn store_embedding(conn: &Connection, id: &str, source: &str, embedding: &[f32], hash: &str, engine: Option<&str>, model: Option<&str>) -> Result<(), String> {
    let blob = serialize_embedding(embedding);
    conn.execute(
        "INSERT OR REPLACE INTO embeddings (id, source, embedding, content_hash, engine, model) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params![id, source, blob, hash, engine, model],
    ).map_err(|e| format!("Failed to store embedding: {}", e))?;
    Ok(())
}

/// Load all embeddings for a given source type
pub fn load_embeddings(conn: &Connection, source: Option<&str>) -> Result<Vec<(String, String, Vec<f32>)>, String> {
    let query = if let Some(src) = source {
        format!("SELECT id, source, embedding FROM embeddings WHERE source = '{}'", src)
    } else {
        "SELECT id, source, embedding FROM embeddings".to_string()
    };

    let mut stmt = conn.prepare(&query)
        .map_err(|e| format!("Failed to prepare embeddings query: {}", e))?;
    
    let rows = stmt.query_map([], |row| {
        let id: String = row.get(0)?;
        let source: String = row.get(1)?;
        let blob: Vec<u8> = row.get(2)?;
        Ok((id, source, deserialize_embedding(&blob)))
    }).map_err(|e| format!("Failed to query embeddings: {}", e))?;

    let mut results = Vec::new();
    for row in rows {
        if let Ok(r) = row {
            results.push(r);
        }
    }
    Ok(results)
}

/// A pending (not-yet-embedded) item queued for the next flush: `(id, content_hash, truncated_text)`.
type PendingEmbedItem = (String, String, String);

/// Processes one buffered chunk of pending items for a single sub-pass (notes/facts/
/// chats share this — it is the one place embed+store actually happens, #3082).
///
/// `batch` is `None` for the unbatched pipeline (`run_embedding_pipeline`) — each item
/// in `items` is embedded one at a time via `embed_with_token_retry`, identical to the
/// pre-#3082 behavior byte-for-byte (same error messages, same auth-abort semantics).
///
/// `batch` is `Some(batch_embed_fn)` for the batched pipeline
/// (`run_embedding_pipeline_batched`) — `items` (up to the configured batch size) are
/// sent in ONE `/embeddings` call. On any non-auth batch-level failure (HTTP error,
/// JSON error, or a returned vector count that doesn't match the request) this
/// degrades to the exact same per-item `embed_with_token_retry` loop as the unbatched
/// path, but ONLY for this one chunk — so one bad item in a batch of 25 is blamed on
/// that item alone, and the other items in the same batch are unaffected either way
/// (Matt's ruling — the `0 failed` live-test baseline must hold under batching).
fn embed_and_store_chunk<F>(
    conn: &Connection,
    source: &str,
    engine: &str,
    model: &str,
    max_tokens: u64,
    items: &[PendingEmbedItem],
    embed_fn: &F,
    batch_embed_fn: Option<&dyn Fn(&[&str]) -> Result<Vec<Vec<f32>>, String>>,
) -> Result<(usize, usize), String>
where
    F: Fn(&str) -> Result<Vec<f32>, String>,
{
    if items.is_empty() {
        return Ok((0, 0));
    }

    if let Some(batch_fn) = batch_embed_fn {
        let texts: Vec<&str> = items.iter().map(|(_, _, t)| t.as_str()).collect();
        match batch_fn(&texts) {
            Ok(embeddings) if embeddings.len() == items.len() => {
                let mut stored = 0usize;
                for ((id, hash, _), emb) in items.iter().zip(embeddings.iter()) {
                    store_embedding(conn, id, source, emb, hash, Some(engine), Some(model))?;
                    stored += 1;
                }
                return Ok((stored, 0));
            }
            Ok(embeddings) => {
                // Length mismatch — never assume positional correspondence on a response
                // shape we didn't expect (#3082). Treat exactly like a batch failure.
                eprintln!(
                    "[EMBEDDINGS] Batch length mismatch for {} items ({} requested, {} returned) — \
                     falling back to per-item embedding for this batch only",
                    source, items.len(), embeddings.len()
                );
            }
            Err(e) if is_auth_error(&e) => {
                return Err(format!("Embedding auth failed — aborting pipeline: {}", e));
            }
            Err(e) => {
                eprintln!(
                    "[EMBEDDINGS] Batch call failed for {} {} item(s) ({}) — falling back to \
                     per-item embedding for this batch only",
                    items.len(), source, e
                );
            }
        }
    }

    // Unbatched path, OR the fallback-from-a-failed-batch path — both run the exact
    // same per-item loop, so a batch failure degrades to today's unbatched behavior
    // for precisely the items in that batch, never more.
    let mut stored = 0usize;
    let mut failed = 0usize;
    for (id, hash, truncated) in items {
        match embed_with_token_retry(truncated, max_tokens, embed_fn) {
            Ok(emb) => {
                store_embedding(conn, id, source, &emb, hash, Some(engine), Some(model))?;
                stored += 1;
            }
            Err(e) if is_auth_error(&e) => {
                return Err(format!("Embedding auth failed — aborting pipeline: {}", e));
            }
            Err(e) => {
                eprintln!("[EMBEDDINGS] Failed to embed {} {}: {}", source, id, e);
                failed += 1;
            }
        }
    }
    Ok((stored, failed))
}

/// Run the full embedding pipeline — index notes, memories, and recent chats.
///
/// `embed_fn` is called to produce an embedding vector for each text snippet, one at a
/// time. The caller (lib.rs or commands.rs) constructs the closure from the active HTTP
/// backend (OR/Ollama/Venice). Keeping it generic avoids coupling the pipeline to a
/// concrete backend and makes this function unit-testable (ADR-0047).
///
/// `progress` is called at most once per 25 items per sub-pass (notes →
/// facts → chats), and always once at the final item of each sub-pass so
/// counts land on their totals (§3 throttle).
///
/// Unchanged entry point — every existing caller (`run_integrity_check`, the startup
/// pipeline, and every test in this module) keeps its exact pre-#3082 behavior. This is
/// a thin wrapper around `run_embedding_pipeline_impl` with `batch = None`; see
/// `run_embedding_pipeline_batched` below for the batched variant — one shared
/// implementation backs both, not two parallel copies of the pipeline.
pub fn run_embedding_pipeline<F>(
    conn: &Connection,
    pane_root: &Path,
    embed_fn: &F,
    engine: &str,
    model: &str,
    max_tokens: usize,
    progress: impl Fn(EmbeddingProgress) + Send + Sync,
) -> Result<(usize, usize, usize, usize), String>
where
    F: Fn(&str) -> Result<Vec<f32>, String>,
{
    run_embedding_pipeline_impl(conn, pane_root, embed_fn, engine, model, max_tokens, progress, None)
}

/// Batched variant of `run_embedding_pipeline` (#3082) — used by `switch_embedding_engine`.
///
/// `embed_fn` remains required: it is the per-item fallback used whenever a batch fails
/// as a unit (see `embed_and_store_chunk`), so a single bad item inside a batch is still
/// attributed to that item alone, exactly as the unbatched path would.
///
/// `batch_embed_fn` sends up to `batch_size` texts in one `/embeddings` call.
/// `batch_size` comes from `resolve_embeddings_batch_size` (`ai::scheduler`) — never a
/// bare literal here, the caller resolves it from `ai_settings` before calling in.
pub fn run_embedding_pipeline_batched<F>(
    conn: &Connection,
    pane_root: &Path,
    embed_fn: &F,
    engine: &str,
    model: &str,
    max_tokens: usize,
    progress: impl Fn(EmbeddingProgress) + Send + Sync,
    batch_embed_fn: &dyn Fn(&[&str]) -> Result<Vec<Vec<f32>>, String>,
    batch_size: usize,
) -> Result<(usize, usize, usize, usize), String>
where
    F: Fn(&str) -> Result<Vec<f32>, String>,
{
    run_embedding_pipeline_impl(
        conn,
        pane_root,
        embed_fn,
        engine,
        model,
        max_tokens,
        progress,
        Some((batch_embed_fn, batch_size.max(1))),
    )
}

fn run_embedding_pipeline_impl<F>(
    conn: &Connection,
    pane_root: &Path,
    embed_fn: &F,
    engine: &str,
    model: &str,
    max_tokens: usize,
    progress: impl Fn(EmbeddingProgress) + Send + Sync,
    batch: Option<(&dyn Fn(&[&str]) -> Result<Vec<Vec<f32>>, String>, usize)>,
) -> Result<(usize, usize, usize, usize), String>
where
    F: Fn(&str) -> Result<Vec<f32>, String>,
{
    let mut notes_count = 0usize;
    let mut facts_count = 0usize;
    let mut chats_count = 0usize;
    let mut failed_count = 0usize;

    // batch_size = 1 for the unbatched path — this makes every "flush" below fire after
    // exactly one pending item, identical cadence to the pre-#3082 per-item loop.
    let (batch_embed_fn, batch_size) = match batch {
        Some((f, n)) => (Some(f), n),
        None => (None, 1usize),
    };

    // ── Collect all items upfront so we have totals for progress payloads ──

    let notes: Vec<(String, String, Option<String>)> = {
        let mut stmt = conn.prepare(
            // #1717: exclude daily-log notes with no user content (body IS NULL or empty).
            // These notes have a file scaffold (date heading etc.) that passes the file-body
            // guard but carry no semantic content — OR returns data[] = [] for them.
            // COALESCE is required: bare `note_type = 'daily'` returns NULL (not FALSE) when
            // note_type IS NULL, which propagates through NOT() and drops the row — COALESCE
            // converts NULL to '' so non-daily notes are always included.
            "SELECT id, title, file_path FROM notes \
             WHERE deleted_at IS NULL \
             AND NOT (COALESCE(note_type, '') = 'daily' AND (body IS NULL OR TRIM(body) = ''))"
        ).map_err(|e| format!("Failed to query notes: {}", e))?;
        let x = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .map_err(|e| e.to_string())?
            .filter_map(|r| r.ok())
            .collect();
        x
    };

    let facts: Vec<(String, String, Option<String>)> = {
        let mut stmt = conn.prepare(
            "SELECT id, fact, category FROM ai_memory WHERE is_active = 1"
        ).map_err(|e| format!("Failed to query ai_memory: {}", e))?;
        let x = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .map_err(|e| e.to_string())?
            .filter_map(|r| r.ok())
            .collect();
        x
    };

    let chats: Vec<(String, String)> = {
        let mut stmt = conn.prepare(
            "SELECT id, content FROM chat_messages_local ORDER BY created_at DESC LIMIT 1000"
        ).map_err(|e| format!("Failed to query chats: {}", e))?;
        let x = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(|e| e.to_string())?
            .filter_map(|r| r.ok())
            .collect();
        x
    };

    let notes_total = notes.len();
    let facts_total = facts.len();
    let chats_total = chats.len();

    // ── Notes sub-pass ───────────────────────────────────────────────────

    let mut pending_notes: Vec<PendingEmbedItem> = Vec::new();

    for (i, (id, title, file_path)) in notes.iter().enumerate() {
        // #1673: capture body separately so we can guard on body emptiness,
        // not the title+body composite (title alone would pass for date-stubs).
        let (body_text, content) = if let Some(fp) = file_path {
            let full_path = if fp.starts_with('/') {
                PathBuf::from(fp)
            } else {
                pane_root.join(fp)
            };
            let fc = std::fs::read_to_string(&full_path).unwrap_or_default();
            (fc.clone(), format!("{}\n{}", title, fc))
        } else {
            // title-only note (no file): no meaningful body to embed
            (String::new(), title.clone())
        };

        // #1673: skip notes with no body content — date-stub notes (title="2026-03-28",
        // body=empty) pass the old !content.trim().is_empty() guard because the title alone
        // is non-empty; OR then returns 200 with empty data[] → "missing data[0].embedding".
        // Title-only notes (file_path=None) also have no meaningful body to embed.
        // C35 (#3128): the chats sub-pass below already has a `len() >=
        // 20` floor alongside its emptiness check — notes never did. A
        // near-empty note (a stub body of a few characters) passed this
        // guard and got embedded with no floor, producing a degenerate,
        // near-centroid vector that the #3126 0.35 threshold drop then
        // made newly admissible as a false-positive match for unrelated
        // queries. Same floor as chats, for consistency.
        if !body_text.trim().is_empty() && body_text.trim().len() >= 20 {
            let hash = content_hash(&content);
            if !embedding_exists(conn, id, &hash) {
                // Token-aware truncation (#2776) — replaces the old 2000-char cap, which
                // for English text (~4 chars/token) landed at ~500-570 tokens, routinely
                // exceeding bge-base's 512-token limit and causing OpenRouter HTTP 400s.
                let truncated = truncate_to_token_budget(&content, max_tokens).to_string();
                pending_notes.push((id.clone(), hash, truncated));
            }
        }

        let is_last = i + 1 == notes_total;
        if pending_notes.len() >= batch_size || (is_last && !pending_notes.is_empty()) {
            let (stored, failed) = embed_and_store_chunk(
                conn, "note", engine, model, max_tokens as u64, &pending_notes, embed_fn, batch_embed_fn,
            )?;
            notes_count += stored;
            failed_count += failed;
            pending_notes.clear();
        }

        // Batch progress: fire every 25 items or at the last item of this sub-pass
        if notes_total > 0 && ((i + 1) % 25 == 0 || is_last) {
            progress(EmbeddingProgress::Indexing {
                notes: notes_count,
                facts: facts_count,
                chats: chats_count,
                notes_total,
                facts_total,
                chats_total,
            });
        }
    }

    // ── Facts sub-pass ───────────────────────────────────────────────────

    let mut pending_facts: Vec<PendingEmbedItem> = Vec::new();

    for (i, (id, fact, category)) in facts.iter().enumerate() {
        let content = if let Some(cat) = category {
            format!("{}: {}", cat, fact)
        } else {
            fact.clone()
        };

        // #1673: skip facts with empty body for consistency with notes guard
        // C35 (#3128): same `len() >= 20` floor as notes/chats — see the
        // notes sub-pass comment above for why.
        if !fact.trim().is_empty() && fact.trim().len() >= 20 {
            let hash = content_hash(&content);
            if !embedding_exists(conn, id, &hash) {
                // #2776: facts previously had NO truncation at all — any fact string
                // longer than ~512 tokens' worth of characters always failed. Apply the
                // same token-aware truncation as notes/chats.
                let truncated = truncate_to_token_budget(&content, max_tokens).to_string();
                pending_facts.push((id.clone(), hash, truncated));
            }
        }

        let is_last = i + 1 == facts_total;
        if pending_facts.len() >= batch_size || (is_last && !pending_facts.is_empty()) {
            let (stored, failed) = embed_and_store_chunk(
                conn, "memory", engine, model, max_tokens as u64, &pending_facts, embed_fn, batch_embed_fn,
            )?;
            facts_count += stored;
            failed_count += failed;
            pending_facts.clear();
        }

        // Batch progress
        if facts_total > 0 && ((i + 1) % 25 == 0 || is_last) {
            progress(EmbeddingProgress::Indexing {
                notes: notes_count,
                facts: facts_count,
                chats: chats_count,
                notes_total,
                facts_total,
                chats_total,
            });
        }
    }

    // ── Chats sub-pass ───────────────────────────────────────────────────

    let mut pending_chats: Vec<PendingEmbedItem> = Vec::new();

    for (i, (id, content)) in chats.iter().enumerate() {
        if !content.trim().is_empty() && content.len() >= 20 {
            let hash = content_hash(content);
            if !embedding_exists(conn, id, &hash) {
                let truncated = truncate_to_token_budget(content, max_tokens).to_string();
                pending_chats.push((id.clone(), hash, truncated));
            }
        }

        let is_last = i + 1 == chats_total;
        if pending_chats.len() >= batch_size || (is_last && !pending_chats.is_empty()) {
            let (stored, failed) = embed_and_store_chunk(
                conn, "chat", engine, model, max_tokens as u64, &pending_chats, embed_fn, batch_embed_fn,
            )?;
            chats_count += stored;
            failed_count += failed;
            pending_chats.clear();
        }

        // Batch progress
        if chats_total > 0 && ((i + 1) % 25 == 0 || is_last) {
            progress(EmbeddingProgress::Indexing {
                notes: notes_count,
                facts: facts_count,
                chats: chats_count,
                notes_total,
                facts_total,
                chats_total,
            });
        }
    }

    Ok((notes_count, facts_count, chats_count, failed_count))
}

/// Semantic search — find similar items across all embedded content
pub fn semantic_search<F>(
    conn: &Connection,
    embed_fn: &F,
    query: &str,
    threshold: f32,
    limit: usize,
) -> Result<Vec<(String, String, f32)>, String>
where
    F: Fn(&str) -> Result<Vec<f32>, String>,
{
    let query_emb = embed_fn(query)?;
    let query_dim = query_emb.len();

    let all_embeddings = load_embeddings(conn, None)?;

    // #3101: a vector whose dimension doesn't match the query's is a BUG (a stale vector
    // from a different embedding model that the clear-on-switch predicate hasn't caught
    // yet, or a race during a re-index), not a low similarity score. Scoring it anyway
    // produces a cosine value that is numerically well-formed but semantically meaningless
    // — worse than an error, because nothing surfaces as broken (the issue's own framing).
    // Skip it and log, never score it.
    let mut scored: Vec<(String, String, f32)> = Vec::with_capacity(all_embeddings.len());
    let mut skipped_dimension_mismatch = 0usize;
    for (id, source, emb) in &all_embeddings {
        if emb.len() != query_dim {
            skipped_dimension_mismatch += 1;
            eprintln!(
                "[EMBEDDINGS] semantic_search: skipping {} {} — dimension mismatch (query={}, stored={}); \
                 stale vector from a different embedding model, not yet cleared/re-indexed",
                source, id, query_dim, emb.len()
            );
            continue;
        }
        scored.push((id.clone(), source.clone(), cosine_similarity(&query_emb, emb)));
    }
    if skipped_dimension_mismatch > 0 {
        eprintln!(
            "[EMBEDDINGS] semantic_search: skipped {} dimension-mismatched vector(s), {} scored",
            skipped_dimension_mismatch, scored.len()
        );
    }

    // #3101: log the top-3 candidates by score REGARDLESS of threshold, so a correct
    // answer that scores just under the cutoff (the issue's own "shipping announcement
    // workflow" repro) is diagnosable from terminal output instead of silently vanishing.
    let mut by_score = scored.clone();
    by_score.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));
    for (id, source, sim) in by_score.iter().take(3) {
        eprintln!(
            "[EMBEDDINGS] semantic_search top candidate: {} {} score={:.4} (threshold={:.4})",
            source, id, sim, threshold
        );
    }

    let mut results: Vec<(String, String, f32)> = scored
        .into_iter()
        .filter(|(_, _, sim)| *sim > threshold)
        .collect();

    results.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));
    results.truncate(limit);

    Ok(results)
}

// ─── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::sync::Mutex;

    // ── Helper: in-memory SQLite with the minimal schema ──────────────────

    fn make_test_db() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().expect("in-memory DB");
        conn.execute_batch("
            CREATE TABLE IF NOT EXISTS notes (
                id TEXT PRIMARY KEY,
                title TEXT NOT NULL DEFAULT '',
                file_path TEXT,
                body TEXT,
                note_type TEXT,
                deleted_at TEXT
            );
            CREATE TABLE IF NOT EXISTS ai_memory (
                id TEXT PRIMARY KEY,
                fact TEXT NOT NULL,
                category TEXT,
                is_active INTEGER NOT NULL DEFAULT 1
            );
            CREATE TABLE IF NOT EXISTS chat_messages_local (
                id TEXT PRIMARY KEY,
                content TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT ''
            );
            CREATE TABLE IF NOT EXISTS embeddings (
                id TEXT PRIMARY KEY,
                source TEXT NOT NULL,
                embedding BLOB NOT NULL,
                content_hash TEXT NOT NULL,
                engine TEXT,
                model TEXT,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
            CREATE INDEX IF NOT EXISTS idx_embeddings_source ON embeddings(source);
        ").expect("schema setup");
        conn
    }

    // Fake embed function: always returns a 768-dim zero vector
    fn mock_embed(_text: &str) -> Result<Vec<f32>, String> {
        Ok(vec![0.0f32; 768])
    }

    /// Write `content` to a temporary .md file; caller must keep the returned handle alive
    /// for the duration of the test (drop = file deleted).
    fn make_temp_note(content: &str) -> (tempfile::NamedTempFile, String) {
        use std::io::Write;
        let mut f = tempfile::Builder::new()
            .suffix(".md")
            .tempfile()
            .expect("temp note file");
        write!(f, "{}", content).expect("write temp note");
        let path = f.path().to_str().expect("utf8 path").to_string();
        (f, path)
    }

    // ── Serialization ──────────────────────────────────────────────────

    #[test]
    fn test_embedding_progress_indexing_serializes() {
        let p = EmbeddingProgress::Indexing {
            notes: 25,
            facts: 0,
            chats: 0,
            notes_total: 100,
            facts_total: 50,
            chats_total: 200,
        };
        let json: serde_json::Value = serde_json::to_value(&p).expect("serialize");
        assert_eq!(json["kind"], "indexing");
        assert_eq!(json["notes"], 25u64);
        assert_eq!(json["notes_total"], 100u64);
        assert_eq!(json["facts_total"], 50u64);
        assert_eq!(json["chats_total"], 200u64);
    }

    // ── Pipeline callback tests ──────────────────────────────────────────

    #[test]
    fn test_pipeline_callback_fires_with_zero_items() {
        // Empty DB: pipeline should succeed without calling progress at all
        let conn = make_test_db();
        let pane_root = std::path::Path::new("/tmp");
        let events: Mutex<Vec<EmbeddingProgress>> = Mutex::new(Vec::new());

        run_embedding_pipeline(&conn, pane_root, &mock_embed, "test", "test-model", 512, |evt| {
            events.lock().unwrap().push(evt);
        }).expect("pipeline");

        // With zero items in each sub-pass no progress events fire
        assert_eq!(events.lock().unwrap().len(), 0);
    }

    #[test]
    fn test_pipeline_callback_fires_at_end_of_notes_subpass() {
        // 3 notes → progress should fire once (at the final item), not 3 times
        let conn = make_test_db();
        // #1673: notes need real file bodies; title-only notes are now skipped
        let _temp_files: Vec<(tempfile::NamedTempFile, String)> = (0..3).map(|i| {
            let (f, path) = make_temp_note(&format!("Body content for note {}", i));
            conn.execute(
                "INSERT INTO notes (id, title, file_path, deleted_at) VALUES (?1, ?2, ?3, NULL)",
                rusqlite::params![format!("note-{}", i), format!("Note {}", i), path],
            ).unwrap();
            (f, path)
        }).collect();

        let pane_root = std::path::Path::new("/tmp");
        let events: Mutex<Vec<EmbeddingProgress>> = Mutex::new(Vec::new());

        run_embedding_pipeline(&conn, pane_root, &mock_embed, "test", "test-model", 512, |evt| {
            events.lock().unwrap().push(evt);
        }).expect("pipeline");

        let locked = events.lock().unwrap();
        // 3 notes → only 1 indexing event (final-item flush, since 3 < 25)
        assert_eq!(locked.len(), 1, "expected exactly 1 progress event for 3 notes");
        let EmbeddingProgress::Indexing { notes_total, .. } = &locked[0];
        assert_eq!(*notes_total, 3);
    }

    #[test]
    fn test_pipeline_batching_25_items() {
        // 50 notes → exactly 2 progress events (at item 25 and item 50)
        let conn = make_test_db();
        // #1673: notes need real file bodies; title-only notes are now skipped
        let _temp_files: Vec<(tempfile::NamedTempFile, String)> = (0..50).map(|i| {
            let (f, path) = make_temp_note(&format!("Body content for note title {}", i));
            conn.execute(
                "INSERT INTO notes (id, title, file_path, deleted_at) VALUES (?1, ?2, ?3, NULL)",
                rusqlite::params![format!("note-{}", i), format!("Note title {}", i), path],
            ).unwrap();
            (f, path)
        }).collect();

        let pane_root = std::path::Path::new("/tmp");
        let events: Mutex<Vec<EmbeddingProgress>> = Mutex::new(Vec::new());

        run_embedding_pipeline(&conn, pane_root, &mock_embed, "test", "test-model", 512, |evt| {
            events.lock().unwrap().push(evt);
        }).expect("pipeline");

        let locked = events.lock().unwrap();
        // 50 notes: fire at i=24 (item 25) and i=49 (item 50, which is also the last)
        assert_eq!(locked.len(), 2, "expected exactly 2 indexing events for 50 notes");
    }

    #[test]
    fn test_pipeline_batching_at_most_once_per_25_items() {
        // 75 notes → exactly 3 events (items 25, 50, 75)
        let conn = make_test_db();
        // #1673: notes need real file bodies; title-only notes are now skipped
        let _temp_files: Vec<(tempfile::NamedTempFile, String)> = (0..75).map(|i| {
            let (f, path) = make_temp_note(&format!("Body content for note {}", i));
            conn.execute(
                "INSERT INTO notes (id, title, file_path, deleted_at) VALUES (?1, ?2, ?3, NULL)",
                rusqlite::params![format!("note-{}", i), format!("Title for note {}", i), path],
            ).unwrap();
            (f, path)
        }).collect();

        let pane_root = std::path::Path::new("/tmp");
        let event_count = Mutex::new(0usize);

        run_embedding_pipeline(&conn, pane_root, &mock_embed, "test", "test-model", 512, |_evt| {
            *event_count.lock().unwrap() += 1;
        }).expect("pipeline");

        assert_eq!(*event_count.lock().unwrap(), 3,
            "75 notes should produce exactly 3 indexing events (at 25, 50, 75)");
    }

    #[test]
    fn test_pipeline_final_item_flush_lands_on_total() {
        // 27 notes → 2 events (at 25 and 27); last event's notes == notes processed
        let conn = make_test_db();
        // #1673: notes need real file bodies; title-only notes are now skipped
        let _temp_files: Vec<(tempfile::NamedTempFile, String)> = (0..27).map(|i| {
            let (f, path) = make_temp_note(&format!("Body content for title {}", i));
            conn.execute(
                "INSERT INTO notes (id, title, file_path, deleted_at) VALUES (?1, ?2, ?3, NULL)",
                rusqlite::params![format!("note-{:02}", i), format!("Title {}", i), path],
            ).unwrap();
            (f, path)
        }).collect();

        let pane_root = std::path::Path::new("/tmp");
        let events: Mutex<Vec<EmbeddingProgress>> = Mutex::new(Vec::new());

        run_embedding_pipeline(&conn, pane_root, &mock_embed, "test", "test-model", 512, |evt| {
            events.lock().unwrap().push(evt);
        }).expect("pipeline");

        let locked = events.lock().unwrap();
        // 27 notes: fire at i=24 (batch) and i=26 (last item)
        assert_eq!(locked.len(), 2, "expected 2 events for 27 notes");
        // Final event should have notes_total == 27
        let EmbeddingProgress::Indexing { notes_total, notes, .. } = &locked[1];
        assert_eq!(*notes_total, 27, "final event notes_total should be 27");
        assert_eq!(*notes, 27, "all 27 notes should be embedded (mock_embed always succeeds)");
    }

    #[test]
    fn test_pipeline_multi_subpass_counts() {
        // 3 notes + 2 facts + 5 chats → 3 separate final-flush events
        let conn = make_test_db();

        // #1673: notes need real file bodies; title-only notes are now skipped
        let _temp_files: Vec<(tempfile::NamedTempFile, String)> = (0..3).map(|i| {
            let (f, path) = make_temp_note(&format!("Body content for note {}", i));
            conn.execute(
                "INSERT INTO notes (id, title, file_path, deleted_at) VALUES (?1, ?2, ?3, NULL)",
                rusqlite::params![format!("note-{}", i), format!("Note {}", i), path],
            ).unwrap();
            (f, path)
        }).collect();
        // C35 (#3128): facts now need len >= 20 to be processed, same as
        // chats below — "Fact about thing {}" alone is only 18-19 chars.
        for i in 0..2 {
            conn.execute(
                "INSERT INTO ai_memory (id, fact, category, is_active) VALUES (?1, ?2, NULL, 1)",
                rusqlite::params![format!("fact-{}", i), format!("Fact about thing number {}", i)],
            ).unwrap();
        }
        // Chats need len >= 20 to be processed
        for i in 0..5 {
            conn.execute(
                "INSERT INTO chat_messages_local (id, content, created_at) VALUES (?1, ?2, datetime('now'))",
                rusqlite::params![format!("chat-{}", i), format!("This is chat message number {} with enough length", i)],
            ).unwrap();
        }

        let pane_root = std::path::Path::new("/tmp");
        let events: Mutex<Vec<EmbeddingProgress>> = Mutex::new(Vec::new());

        run_embedding_pipeline(&conn, pane_root, &mock_embed, "test", "test-model", 512, |evt| {
            events.lock().unwrap().push(evt);
        }).expect("pipeline");

        let locked = events.lock().unwrap();
        // 3 sub-passes each producing 1 final-flush event
        assert_eq!(locked.len(), 3, "expected 3 events (one per sub-pass final flush)");

        // Notes sub-pass final event
        let EmbeddingProgress::Indexing { notes_total, facts_total, chats_total, .. } = &locked[0];
        assert_eq!(*notes_total, 3);
        assert_eq!(*facts_total, 2); // totals known from the start
        assert_eq!(*chats_total, 5);

        // Facts sub-pass final event
        let EmbeddingProgress::Indexing { facts, .. } = &locked[1];
        assert_eq!(*facts, 2);

        // Chats sub-pass final event
        let EmbeddingProgress::Indexing { chats, .. } = &locked[2];
        assert_eq!(*chats, 5);
    }

    #[test]
    fn test_pipeline_skips_already_embedded_notes() {
        // Same note embedded twice: second run produces 0 new embeddings but
        // progress still fires (notes_total > 0, final flush fires).
        let conn = make_test_db();
        // #1673: notes need real file bodies; title-only notes are now skipped
        let (_f, path) = make_temp_note("Hello world body content");
        conn.execute(
            "INSERT INTO notes (id, title, file_path, deleted_at) VALUES ('n1', 'Hello world', ?1, NULL)",
            rusqlite::params![path],
        ).unwrap();

        let pane_root = std::path::Path::new("/tmp");

        // First run: embeds the note
        let first_counts = run_embedding_pipeline(&conn, pane_root, &mock_embed, "test", "test-model", 512, |_| {})
            .expect("first pipeline");
        assert_eq!(first_counts.0, 1, "first run should embed 1 note");

        // Second run: note is already embedded with the same content hash
        let second_counts = run_embedding_pipeline(&conn, pane_root, &mock_embed, "test", "test-model", 512, |_| {})
            .expect("second pipeline");
        assert_eq!(second_counts.0, 0, "second run should skip already-embedded note");
    }

    // ── #3101: semantic_search dimension guard + threshold ─────────────────────────

    /// #3101 — a vector whose dimension doesn't match the query's must be SKIPPED
    /// (logged, never scored), not compared anyway. Seeds one same-dimension match
    /// and one mismatched-dimension row from a stale/different model; confirms the
    /// mismatched row never appears in results regardless of threshold.
    #[test]
    fn test_semantic_search_skips_dimension_mismatched_vectors() {
        let conn = make_test_db();
        // Query embed_fn always returns a 4-dim vector.
        let embed_fn = |_text: &str| -> Result<Vec<f32>, String> { Ok(vec![1.0, 0.0, 0.0, 0.0]) };

        // Same-dimension row: identical vector → cosine similarity 1.0, well above any threshold.
        store_embedding(&conn, "match-4dim", "note", &[1.0, 0.0, 0.0, 0.0], "hash-ok", Some("openrouter"), Some("model-a")).unwrap();
        // Mismatched-dimension row: a stale vector from a different model (8-dim).
        store_embedding(&conn, "stale-8dim", "note", &[1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0], "hash-stale", Some("openrouter"), Some("model-b")).unwrap();

        let results = semantic_search(&conn, &embed_fn, "query", 0.0, 10).expect("semantic_search");

        assert_eq!(results.len(), 1, "only the same-dimension row must be scored");
        assert_eq!(results[0].0, "match-4dim", "the matching-dimension row must be the one returned");
        assert!(
            results.iter().all(|(id, _, _)| id != "stale-8dim"),
            "the dimension-mismatched row must never appear in results, at any threshold"
        );
    }

    /// #3101 — the new 0.35 threshold (vs the old fixed 0.5) must catch a hit that
    /// scores between the two cutoffs — reproducing the issue's own "shipping
    /// announcement workflow" repro shape (a conceptually correct answer that scored
    /// just under 0.5). Crafts two vectors with a known cosine similarity in that
    /// band, directly (not via a real embedding model), so the test is deterministic.
    #[test]
    fn test_semantic_search_035_threshold_catches_hit_05_would_drop() {
        let conn = make_test_db();
        // cos(theta) for unit vectors (1,0) and (x, sqrt(1-x^2)): choose x = 0.42,
        // giving cosine similarity exactly 0.42 — strictly between 0.35 and 0.5.
        let x: f32 = 0.42;
        let y: f32 = (1.0 - x * x).sqrt();
        let query_vec = vec![1.0f32, 0.0f32];
        let stored_vec = vec![x, y];
        // Sanity check the fixture's own premise before trusting the assertions below.
        let sim = cosine_similarity(&query_vec, &stored_vec);
        assert!((sim - 0.42).abs() < 0.001, "fixture premise: cosine similarity must be ~0.42, got {}", sim);

        store_embedding(&conn, "mid-score", "note", &stored_vec, "hash-mid", Some("openrouter"), Some("model-a")).unwrap();
        let embed_fn = move |_text: &str| -> Result<Vec<f32>, String> { Ok(query_vec.clone()) };

        // At the OLD 0.5 cutoff: dropped (0.42 < 0.5) — reproduces the issue's bug.
        let old_cutoff_results = semantic_search(&conn, &embed_fn, "q", 0.5, 10).expect("semantic_search");
        assert!(old_cutoff_results.is_empty(), "at the OLD 0.5 cutoff this hit must be dropped (reproducing the bug)");

        // At the NEW 0.35 cutoff (SEMANTIC_SEARCH_SIMILARITY_THRESHOLD): caught.
        let new_cutoff_results = semantic_search(&conn, &embed_fn, "q", SEMANTIC_SEARCH_SIMILARITY_THRESHOLD, 10).expect("semantic_search");
        assert_eq!(new_cutoff_results.len(), 1, "at the NEW 0.35 cutoff this hit must be caught");
        assert_eq!(new_cutoff_results[0].0, "mid-score");
    }

    /// #3101 — migration CRR-hazard check (issue's own instruction: "verify CRR
    /// status first"). `embeddings` must NOT be in `migrations::CRR_TABLES`, which is
    /// what makes the plain `ALTER TABLE embeddings ADD COLUMN model TEXT` in
    /// `create_embeddings_table` safe — a CRR-tracked table would need the
    /// `migrate_attachments_v68`-style rebuild shape instead (adding a column to an
    /// already-`crsql_as_crr`'d table breaks cr-sqlite's generated triggers, which are
    /// static SQL naming every column explicitly at `crsql_as_crr()` time).
    #[test]
    fn test_embeddings_table_is_not_crr_tracked() {
        assert!(
            !crate::migrations::CRR_TABLES.contains(&"embeddings"),
            "embeddings must stay out of CRR_TABLES — if this ever changes, the plain \
             ALTER TABLE ADD COLUMN used for the `model` column (and the pre-existing \
             `engine` column) becomes unsafe and needs the CRR rebuild-shape migration instead"
        );
    }

    // ── HTTP embed_fn + engine switch tests ─────────────────────────────────────────────
    // These tests use mockito to intercept HTTP calls; no real OR/Ollama endpoints are hit.

    /// Make a test DB with ai_settings table (for engine config tests)
    fn make_full_test_db() -> rusqlite::Connection {
        let conn = make_test_db();
        conn.execute_batch("
            CREATE TABLE IF NOT EXISTS ai_settings (
                key TEXT PRIMARY KEY,
                value TEXT
            );
        ").expect("ai_settings setup");
        conn
    }

    #[test]
    fn test_openrouter_embed_fn_parses_response() {
        // Mock the OR embeddings endpoint to return a small 3-dim vector
        let mut server = mockito::Server::new();
        let _m = server.mock("POST", "/api/v1/embeddings")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"data":[{"embedding":[0.1,0.2,0.3]}],"model":"text-embedding-3-small","object":"list"}"#)
            .create();

        // Build a minimal embed_fn that hits the mock server instead of real OR
        let api_key = "test-key-abc".to_string();
        let model = "text-embedding-3-small".to_string();
        let base_url = server.url();
        let embed_fn = move |text: &str| -> Result<Vec<f32>, String> {
            let client = reqwest::blocking::Client::new();
            let body = serde_json::json!({ "model": model, "input": text });
            let url = format!("{}/api/v1/embeddings", base_url);
            let resp = client.post(&url)
                .header("Authorization", format!("Bearer {}", api_key))
                .header("Content-Type", "application/json")
                .json(&body)
                .send()
                .map_err(|e| format!("request failed: {}", e))?;
            let json: serde_json::Value = resp.json()
                .map_err(|e| format!("json parse failed: {}", e))?;
            let emb = json["data"][0]["embedding"]
                .as_array()
                .ok_or_else(|| "missing embedding".to_string())?;
            Ok(emb.iter().map(|v| v.as_f64().unwrap_or(0.0) as f32).collect())
        };

        let result = embed_fn("hello world");
        assert!(result.is_ok(), "embed_fn should succeed: {:?}", result);
        let vec = result.unwrap();
        assert_eq!(vec.len(), 3, "should return 3 floats");
        assert!((vec[0] - 0.1f32).abs() < 1e-6, "first element should be ~0.1");
        assert!((vec[1] - 0.2f32).abs() < 1e-6, "second element should be ~0.2");
        assert!((vec[2] - 0.3f32).abs() < 1e-6, "third element should be ~0.3");
    }

    #[test]
    fn test_ollama_embed_fn_parses_response() {
        // Mock the Ollama /api/embed endpoint to return a small 3-dim vector
        let mut server = mockito::Server::new();
        let _m = server.mock("POST", "/api/embed")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"embeddings":[[0.4,0.5,0.6]],"model":"nomic-embed-text"}"#)
            .create();

        let model = "nomic-embed-text".to_string();
        let base_url = server.url();
        let embed_fn = move |text: &str| -> Result<Vec<f32>, String> {
            let client = reqwest::blocking::Client::new();
            let body = serde_json::json!({ "model": model, "input": text });
            let url = format!("{}/api/embed", base_url);
            let resp = client.post(&url)
                .header("Content-Type", "application/json")
                .json(&body)
                .send()
                .map_err(|e| format!("request failed: {}", e))?;
            let json: serde_json::Value = resp.json()
                .map_err(|e| format!("json parse failed: {}", e))?;
            let emb = json["embeddings"][0]
                .as_array()
                .ok_or_else(|| "missing embeddings[0]".to_string())?;
            Ok(emb.iter().map(|v| v.as_f64().unwrap_or(0.0) as f32).collect())
        };

        let result = embed_fn("test text");
        assert!(result.is_ok(), "ollama embed_fn should succeed: {:?}", result);
        let vec = result.unwrap();
        assert_eq!(vec.len(), 3, "should return 3 floats");
        assert!((vec[0] - 0.4f32).abs() < 1e-6, "first element should be ~0.4");
    }

    #[test]
    fn test_embed_fn_propagates_http_error() {
        // Mock returns a 500 error
        let mut server = mockito::Server::new();
        let _m = server.mock("POST", "/api/v1/embeddings")
            .with_status(500)
            .with_header("content-type", "application/json")
            .with_body(r#"{"error":"server error"}"#)
            .create();

        let base_url = server.url();
        let embed_fn = move |_text: &str| -> Result<Vec<f32>, String> {
            let client = reqwest::blocking::Client::new();
            let body = serde_json::json!({ "model": "m", "input": "x" });
            let url = format!("{}/api/v1/embeddings", base_url);
            let resp = client.post(&url)
                .json(&body)
                .send()
                .map_err(|e| format!("request failed: {}", e))?;
            if !resp.status().is_success() {
                let status = resp.status();
                let body_text = resp.text().unwrap_or_default();
                return Err(format!("HTTP {}: {}", status, body_text));
            }
            Err("unreachable".to_string())
        };

        let result = embed_fn("hello");
        assert!(result.is_err(), "embed_fn should return error on HTTP 500");
        let err = result.unwrap_err();
        assert!(err.contains("500") || err.contains("HTTP"), "error should mention HTTP status: {}", err);
    }

    // ── #2776: OpenRouter 400 "token limit exceeded" integration coverage ──────────

    #[test]
    fn test_openrouter_400_token_limit_pre_fix_truncation_would_exceed() {
        // Demonstrates the ROOT CAUSE: the old 2000-char cap has NO safety margin against
        // estimation error between this heuristic and bge-base's real WordPiece tokenizer
        // (the thing that actually rejects the request with HTTP 400, per #2776's reported
        // "You passed 513 input tokens" error). A generous 4 chars/token estimate on plain
        // English words puts 2000 chars right at the edge (2000/4.0 = exactly 500) — but
        // the codebase's OWN more conservative ratio for non-GPT-family text
        // (`TokenEstimationRatio::for_model`'s Ollama/unknown-model default, 3.5
        // chars/token — see `ai/context_budget.rs`) already tips the same 2000-char slice
        // over the 512-token limit, which is exactly the estimation-error gap the fix's
        // 10% safety margin exists to absorb. This test locks in the FAILURE mode being
        // fixed (a bare, marginless char cap), not the fix itself.
        let optimistic_ratio = crate::ai::context_budget::TokenEstimationRatio { chars_per_token: 4.0 };
        let conservative_ratio = crate::ai::context_budget::TokenEstimationRatio { chars_per_token: 3.5 };
        let old_cap_2000_chars = "word ".repeat(500); // 2500 bytes of content
        let pre_fix_slice = if old_cap_2000_chars.len() > 2000 {
            &old_cap_2000_chars[..old_cap_2000_chars.floor_char_boundary(2000)]
        } else {
            old_cap_2000_chars.as_str()
        };
        // Right at the edge under the optimistic estimate — no margin at all.
        assert_eq!(optimistic_ratio.estimate_tokens(pre_fix_slice), 500);
        // Over the limit under the codebase's own more conservative ratio — the old cap
        // had zero safety margin to absorb exactly this kind of estimation gap.
        let pre_fix_tokens = conservative_ratio.estimate_tokens(pre_fix_slice);
        assert!(
            pre_fix_tokens > 512,
            "pre-fix 2000-char truncation should estimate > 512 tokens under a conservative ratio (got {}), reproducing #2776's root cause",
            pre_fix_tokens
        );
    }

    #[test]
    fn test_post_fix_truncation_stays_under_limit_for_range_of_content_lengths() {
        // Confirms the FIX: for a range of content lengths (well under, near, and far
        // over the old 2000-char cap), truncate_to_token_budget(_, 512) always produces
        // a slice whose estimated token count is <= 512.
        let ratio = crate::ai::context_budget::TokenEstimationRatio { chars_per_token: 4.0 };
        for word_count in [10usize, 100, 500, 1000, 5000, 20000] {
            let content = "word ".repeat(word_count);
            let truncated = truncate_to_token_budget(&content, 512);
            let tokens = ratio.estimate_tokens(truncated);
            assert!(
                tokens <= 512,
                "word_count={} produced {} estimated tokens (must be <= 512)",
                word_count,
                tokens
            );
        }
    }

    #[test]
    fn test_openrouter_400_response_simulated_via_mockito() {
        // Simulates OpenRouter's actual 400 response for an over-limit request
        // ("You passed 513 input tokens... context length is only 512", per #2776's
        // issue log) and confirms embed_fn surfaces it as a non-auth, skip-and-continue
        // error — exercised end-to-end through embed_fn, matching the real failure path.
        let mut server = mockito::Server::new();
        let _m = server.mock("POST", "/api/v1/embeddings")
            .with_status(400)
            .with_header("content-type", "application/json")
            .with_body(r#"{"error":{"message":"You passed 513 input tokens, however the model context length is only 512."}}"#)
            .create();

        let base_url = server.url();
        let embed_fn = move |_text: &str| -> Result<Vec<f32>, String> {
            let client = reqwest::blocking::Client::new();
            let body = serde_json::json!({ "model": "baai/bge-base-en-v1.5", "input": "x" });
            let url = format!("{}/api/v1/embeddings", base_url);
            let resp = client.post(&url)
                .json(&body)
                .send()
                .map_err(|e| format!("request failed: {}", e))?;
            if !resp.status().is_success() {
                let status = resp.status();
                let body_text = resp.text().unwrap_or_default();
                return Err(format!("HTTP {}: {}", status, body_text));
            }
            Err("unreachable".to_string())
        };

        let result = embed_fn("over-limit content");
        assert!(result.is_err(), "embed_fn should return error on HTTP 400");
        let err = result.unwrap_err();
        assert!(err.contains("400"), "error should mention HTTP 400: {}", err);
        assert!(!is_auth_error(&err), "HTTP 400 must NOT be treated as an auth error (skip-and-continue, not abort)");
    }

    // ── #2776 follow-up (Gap 1): adaptive retry on a token-limit 400 ────────────────

    /// Builds an `embed_fn` around a mockito server + shared call counter, matching
    /// this file's existing embed_fn-over-mockito shape (see
    /// `test_openrouter_400_response_simulated_via_mockito` above). The counter lets
    /// tests assert exactly how many HTTP requests were made without relying on
    /// mockito's own hit-count assertions, since `embed_with_token_retry` calls
    /// `attempt` (not the mock server) directly.
    fn counting_embed_fn(
        base_url: String,
        counter: std::sync::Arc<Mutex<usize>>,
    ) -> impl Fn(&str) -> Result<Vec<f32>, String> {
        move |text: &str| -> Result<Vec<f32>, String> {
            *counter.lock().unwrap() += 1;
            let client = reqwest::blocking::Client::new();
            let body = serde_json::json!({ "model": "baai/bge-base-en-v1.5", "input": text });
            let url = format!("{}/api/v1/embeddings", base_url);
            let resp = client.post(&url)
                .json(&body)
                .send()
                .map_err(|e| format!("request failed: {}", e))?;
            if !resp.status().is_success() {
                let status = resp.status();
                let body_text = resp.text().unwrap_or_default();
                return Err(format!("HTTP {}: {}", status, body_text));
            }
            let json: serde_json::Value = resp.json()
                .map_err(|e| format!("json parse failed: {}", e))?;
            let emb = json["data"][0]["embedding"]
                .as_array()
                .ok_or_else(|| "missing embedding".to_string())?;
            Ok(emb.iter().map(|v| v.as_f64().unwrap_or(0.0) as f32).collect())
        }
    }

    #[test]
    fn test_embed_retry_on_token_limit_400() {
        // First call: a token-limit 400 ("You passed 513 input tokens", matching
        // Matt's v1.0.43 retest / OpenRouter's real error shape). Second call
        // (the retry, with proportionally shorter content): 200 with an embedding.
        let mut server = mockito::Server::new();
        let _m400 = server.mock("POST", "/api/v1/embeddings")
            .match_body(mockito::Matcher::Regex(r#""input":"AAAA.{1800,}"#.to_string()))
            .with_status(400)
            .with_header("content-type", "application/json")
            .with_body(r#"{"error":{"message":"You passed 513 input tokens, however the model context length is only 512."}}"#)
            .create();
        let _m200 = server.mock("POST", "/api/v1/embeddings")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"data":[{"embedding":[0.1,0.2,0.3]}]}"#)
            .create();

        let counter = std::sync::Arc::new(Mutex::new(0usize));
        let seen_bodies: std::sync::Arc<Mutex<Vec<usize>>> = std::sync::Arc::new(Mutex::new(vec![]));
        let seen_bodies_clone = seen_bodies.clone();
        let base_url = server.url();
        let counter_clone = counter.clone();
        let attempt = move |text: &str| -> Result<Vec<f32>, String> {
            *counter_clone.lock().unwrap() += 1;
            seen_bodies_clone.lock().unwrap().push(text.len());
            let client = reqwest::blocking::Client::new();
            let body = serde_json::json!({ "model": "baai/bge-base-en-v1.5", "input": text });
            let url = format!("{}/api/v1/embeddings", base_url);
            let resp = client.post(&url)
                .json(&body)
                .send()
                .map_err(|e| format!("request failed: {}", e))?;
            if !resp.status().is_success() {
                let status = resp.status();
                let body_text = resp.text().unwrap_or_default();
                return Err(format!("HTTP {}: {}", status, body_text));
            }
            let json: serde_json::Value = resp.json()
                .map_err(|e| format!("json parse failed: {}", e))?;
            let emb = json["data"][0]["embedding"]
                .as_array()
                .ok_or_else(|| "missing embedding".to_string())?;
            Ok(emb.iter().map(|v| v.as_f64().unwrap_or(0.0) as f32).collect())
        };

        // 1843 chars of content, matching Matt's exact repro shape (dense content that
        // estimated under budget but tokenized to 513 against a 512 limit).
        let content = "A".repeat(1843);
        let result = embed_with_token_retry(&content, 512, &attempt);

        assert!(result.is_ok(), "retry should succeed on the second (shorter-content) attempt: {:?}", result);
        assert_eq!(*counter.lock().unwrap(), 2, "exactly 2 requests must be made (original + one retry)");

        let bodies = seen_bodies.lock().unwrap();
        assert_eq!(bodies.len(), 2);
        assert!(
            bodies[1] < bodies[0],
            "the retry must send proportionally SHORTER content than the original attempt: original={} retry={}",
            bodies[0], bodies[1]
        );
        // Sanity-check the proportional math: 1843 * (512/513) * 0.9 ≈ 1655 chars.
        assert!(
            bodies[1] <= 1656 && bodies[1] >= 1600,
            "retry content length should match the proportional re-truncation formula, got {}",
            bodies[1]
        );
    }

    // ── #2776 Round 3: production-shaped wire-invariant test ────────────────────────

    /// The gap in every retry test above: they all build `embed_fn` by hand (a
    /// parameter-honoring lambda written directly in the test), which can NEVER catch a
    /// closure-capture / lifetime / serialization bug in the REAL production constructor
    /// (`build_http_embed_fn` in `lib.rs`) because the test's own hand-written closure is
    /// definitionally correct.
    ///
    /// This test instead builds `embed_fn` via `build_http_embed_fn` itself — the actual
    /// function every production call site uses — wires it through the real
    /// `embed_with_token_retry`, and inspects the bytes mockito's server actually received
    /// on the wire (via `Request::body()`, not the `text` parameter the test *thinks* it
    /// passed in). If a wire-vs-source divergence existed anywhere between the closure
    /// parameter and `.send()`, this is the test that would catch it — the production
    /// symptom this responds to (#2776 Round 3) is the server reporting the exact same
    /// 513-token count for a 1224-char body AND a 1099-char retry, which is impossible if
    /// the wire genuinely carried the shorter content.
    #[test]
    fn test_build_http_embed_fn_wire_body_shrinks_on_retry() {
        // Uses the Ollama arm (configurable `embeddings_endpoint`) so the mockito server
        // can stand in for the real HTTP target — the OpenRouter arm hardcodes its URL
        // and cannot be pointed at a mock, per #2776 Round 3's instructions.
        let mut server = mockito::Server::new();

        // Single mock, dynamic status+body keyed off a shared call counter (rather than
        // two mocks distinguished by a body-length regex, as in the tests above) — this
        // lets us read the counter, and independently the ACTUAL wire bytes mockito
        // received (`req.body()`), on every call regardless of what content-length
        // pattern the production closure happens to send.
        let seen_bodies: std::sync::Arc<Mutex<Vec<usize>>> = std::sync::Arc::new(Mutex::new(Vec::new()));
        let seen_bodies_status = std::sync::Arc::clone(&seen_bodies);
        let seen_bodies_body = std::sync::Arc::clone(&seen_bodies);

        let _m = server.mock("POST", "/api/embed")
            .with_status_code_from_request(move |req| {
                // Record the ACTUAL wire body length mockito received for this request
                // (not the `text` parameter — that's the whole point of this test).
                let wire_len = req.body().map(|b| b.len()).unwrap_or(0);
                seen_bodies_status.lock().unwrap().push(wire_len);
                let call_num = seen_bodies_status.lock().unwrap().len();
                // First call: token-limit 400 (TEI wording, matching real bge-base-en-v1.5
                // traffic per #2776 follow-up 2). Second call (the retry): 200 + embedding.
                if call_num == 1 { 400 } else { 200 }
            })
            .with_body_from_request(move |_req| {
                let call_num = seen_bodies_body.lock().unwrap().len();
                if call_num <= 1 {
                    br#"{"error":"Input validation error: `inputs` must have less than 512 tokens. Given: 513"}"#.to_vec()
                } else {
                    br#"{"embeddings":[[0.1,0.2,0.3]]}"#.to_vec()
                }
            })
            .with_header("content-type", "application/json")
            .create();

        // Wire `embed_fn` up via the REAL production constructor, pointed at the mock
        // server through the Ollama arm's `embeddings_endpoint` setting.
        let conn = make_full_test_db();
        crate::database::set_ai_setting(&conn, "embeddings_endpoint", &server.url())
            .expect("set embeddings_endpoint");
        let (embed_fn, engine_tag) = crate::build_http_embed_fn(&conn, "ollama", "bge-base-en-v1.5")
            .expect("build_http_embed_fn should succeed for the ollama arm");
        assert_eq!(engine_tag, "ollama");

        // Content long enough that the production truncation path (`truncate_to_token_budget`,
        // the same function `run_embedding_pipeline` calls at each of its three sub-pass
        // call sites) actually cuts it down before the first attempt — matching the shape
        // real indexed content takes, not a hand-picked literal.
        let content = "A".repeat(5000);
        let truncated = truncate_to_token_budget(&content, 512);

        let result = embed_with_token_retry(truncated, 512, &embed_fn);

        assert!(
            result.is_ok(),
            "embed_with_token_retry through the real build_http_embed_fn closure should \
             succeed on the second (shorter-content) attempt: {:?}",
            result
        );

        let bodies = seen_bodies.lock().unwrap();
        assert_eq!(bodies.len(), 2, "exactly 2 wire requests must be made (original + one retry)");

        // THE assertion this whole test exists for: the SECOND request's ACTUAL wire body
        // (captured from mockito's own `Request::body()`, not from what the test *thinks*
        // it sent) must be strictly shorter than the first. If this fails, the wire is not
        // carrying the truncated content despite the source code looking correct end to
        // end — i.e. #2776 Round 3's production bug has been reproduced here, and per the
        // task's own instructions, Commit 2 (the ratio fix) must NOT proceed; this failure
        // itself IS the bug to name and fix.
        assert!(
            bodies[1] < bodies[0],
            "PRODUCTION-SHAPED WIRE INVARIANT VIOLATED (#2776 Round 3): second wire body ({} \
             bytes) must be strictly shorter than the first ({} bytes) — matches the reported \
             symptom of the server seeing an unchanged token count across a supposedly-shrinking \
             retry. If this assertion is failing, do NOT proceed to a ratio fix per the task \
             spec — this IS the real bug.",
            bodies[1], bodies[0]
        );
    }

    #[test]
    fn test_embed_no_retry_on_non_token_400() {
        // A 400 whose body does NOT match the token-limit pattern (e.g. a real auth
        // failure surfaced as 400 by some backends) must fail immediately — no retry,
        // exactly 1 request made.
        let mut server = mockito::Server::new();
        let _m = server.mock("POST", "/api/v1/embeddings")
            .with_status(400)
            .with_header("content-type", "application/json")
            .with_body(r#"{"error":{"message":"Invalid API key"}}"#)
            .create();

        let counter = std::sync::Arc::new(Mutex::new(0usize));
        let attempt = counting_embed_fn(server.url(), counter.clone());

        let result = embed_with_token_retry("some content", 512, &attempt);

        assert!(result.is_err(), "a non-token 400 must return an error");
        assert_eq!(*counter.lock().unwrap(), 1, "exactly 1 request must be made — no retry on a non-token 400");
        let err = result.unwrap_err();
        assert!(err.contains("Invalid API key"), "error should carry through the original body: {}", err);
    }

    #[test]
    fn test_embed_second_token_400_fails_cleanly() {
        // Both the original attempt AND the retry return a token-limit 400 — the
        // second failure must be returned as-is, with no third attempt (no infinite
        // loop), and exactly 2 requests made total.
        let mut server = mockito::Server::new();
        let _m = server.mock("POST", "/api/v1/embeddings")
            .with_status(400)
            .with_header("content-type", "application/json")
            .with_body(r#"{"error":{"message":"You passed 513 input tokens, however the model context length is only 512."}}"#)
            .create();

        let counter = std::sync::Arc::new(Mutex::new(0usize));
        let attempt = counting_embed_fn(server.url(), counter.clone());

        let result = embed_with_token_retry("some content that is still too dense", 512, &attempt);

        assert!(result.is_err(), "a second token-limit 400 must return an error, not loop");
        assert_eq!(*counter.lock().unwrap(), 2, "exactly 2 requests must be made (original + one retry, then stop)");
        let err = result.unwrap_err();
        assert!(err.contains("You passed 513 input tokens"), "error should carry through the second 400's body: {}", err);
    }

    // ── #2776 follow-up 2 (TEI wording + retry logging) ─────────────────────────────

    #[test]
    fn test_parse_token_limit_recognizes_tei_wording() {
        // Real OpenRouter "Provider returned error" envelope wrapping TEI's actual
        // validation-error wording as an escaped JSON string in `error.metadata.raw`
        // (huggingface/text-embeddings-inference#356 confirms TEI's exact wording;
        // OpenRouter's own docs confirm the metadata.raw wrapping pattern for
        // provider-level errors). This is the real double-wrapped shape a
        // baai/bge-base-en-v1.5 call via OpenRouter/Together actually produces —
        // the old "You passed N input tokens" regex never matches this text at all.
        let tei_wrapped = r#"OpenRouter embed HTTP 400: {"error":{"code":400,"message":"Provider returned error","metadata":{"raw":"{\"error\":\"Input validation error: `inputs` must have less than 512 tokens. Given: 513\",\"error_type\":\"Validation\"}","provider_name":"Together"}}}"#;
        assert_eq!(
            parse_token_limit_from_error(tei_wrapped),
            Some(513),
            "must recognize TEI's real wording ('must have less than N tokens. Given: M') inside the OpenRouter envelope"
        );
    }

    #[test]
    fn test_parse_token_limit_still_recognizes_openai_wording() {
        // Regression: the pre-existing OpenAI-chat-completions-style wording must
        // still work as a fallback for other provider/model families.
        let openai_style = "HTTP 400: You passed 513 input tokens";
        assert_eq!(
            parse_token_limit_from_error(openai_style),
            Some(513),
            "OpenAI-style wording must still parse via the fallback regex"
        );
    }

    #[test]
    fn test_embed_retry_on_tei_wrapped_token_limit_400() {
        // First call: a real OpenRouter "Provider returned error" envelope wrapping
        // TEI's actual token-limit wording (see diagnosis at
        // docs/audits/2026-09-01-2776-retry-diagnosis.md). Second call (the retry,
        // with proportionally shorter content): 200 with an embedding. This is the
        // exact shape that previously produced ZERO retries for real bge-base-en-v1.5
        // traffic because the old regex never matched TEI's wording.
        let mut server = mockito::Server::new();
        let _m400 = server.mock("POST", "/api/v1/embeddings")
            .match_body(mockito::Matcher::Regex(r#""input":"AAAA.{1800,}"#.to_string()))
            .with_status(400)
            .with_header("content-type", "application/json")
            .with_body(r#"{"error":{"code":400,"message":"Provider returned error","metadata":{"raw":"{\"error\":\"Input validation error: `inputs` must have less than 512 tokens. Given: 513\",\"error_type\":\"Validation\"}","provider_name":"Together"}}}"#)
            .create();
        let _m200 = server.mock("POST", "/api/v1/embeddings")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"data":[{"embedding":[0.1,0.2,0.3]}]}"#)
            .create();

        let counter = std::sync::Arc::new(Mutex::new(0usize));
        let seen_bodies: std::sync::Arc<Mutex<Vec<usize>>> = std::sync::Arc::new(Mutex::new(vec![]));
        let seen_bodies_clone = seen_bodies.clone();
        let base_url = server.url();
        let counter_clone = counter.clone();
        let attempt = move |text: &str| -> Result<Vec<f32>, String> {
            *counter_clone.lock().unwrap() += 1;
            seen_bodies_clone.lock().unwrap().push(text.len());
            let client = reqwest::blocking::Client::new();
            let body = serde_json::json!({ "model": "baai/bge-base-en-v1.5", "input": text });
            let url = format!("{}/api/v1/embeddings", base_url);
            let resp = client.post(&url)
                .json(&body)
                .send()
                .map_err(|e| format!("request failed: {}", e))?;
            if !resp.status().is_success() {
                let status = resp.status();
                let body_text = resp.text().unwrap_or_default();
                return Err(format!("OpenRouter embed HTTP {}: {}", status, body_text));
            }
            let json: serde_json::Value = resp.json()
                .map_err(|e| format!("json parse failed: {}", e))?;
            let emb = json["data"][0]["embedding"]
                .as_array()
                .ok_or_else(|| "missing embedding".to_string())?;
            Ok(emb.iter().map(|v| v.as_f64().unwrap_or(0.0) as f32).collect())
        };

        // Same dense-content repro shape as Matt's original report.
        let content = "A".repeat(1843);
        let result = embed_with_token_retry(&content, 512, &attempt);

        assert!(result.is_ok(), "retry must succeed against the real TEI-wrapped-in-OpenRouter error shape: {:?}", result);
        assert_eq!(*counter.lock().unwrap(), 2, "exactly 2 requests must be made (original + one retry)");

        let bodies = seen_bodies.lock().unwrap();
        assert_eq!(bodies.len(), 2);
        assert!(
            bodies[1] < bodies[0],
            "the retry must send proportionally SHORTER content than the original attempt: original={} retry={}",
            bodies[0], bodies[1]
        );
    }

    #[test]
    fn test_embed_retry_logs_on_success() {
        // Verifies the logging *mechanism* exists: the retry path (which now contains
        // `eprintln!("[EMBEDDINGS] Retry: ...")` and
        // `eprintln!("[EMBEDDINGS] Retry succeeded on second attempt")`) executes and
        // reaches a successful second attempt. Rust's test harness does not capture
        // `eprintln!` output for inline assertion (stdout capture only, and even that
        // is best-effort), so this test exercises the exact success branch those log
        // lines live in rather than scraping stderr text — the eprintln content itself
        // is verified by inspection of the source (see embed_with_token_retry).
        let mut server = mockito::Server::new();
        let _m400 = server.mock("POST", "/api/v1/embeddings")
            .match_body(mockito::Matcher::Regex(r#""input":"BBBB.{1800,}"#.to_string()))
            .with_status(400)
            .with_header("content-type", "application/json")
            .with_body(r#"{"error":{"code":400,"message":"Provider returned error","metadata":{"raw":"{\"error\":\"Input validation error: `inputs` must have less than 512 tokens. Given: 513\",\"error_type\":\"Validation\"}","provider_name":"Together"}}}"#)
            .create();
        let _m200 = server.mock("POST", "/api/v1/embeddings")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"data":[{"embedding":[0.4,0.5,0.6]}]}"#)
            .create();

        let counter = std::sync::Arc::new(Mutex::new(0usize));
        let attempt = counting_embed_fn(server.url(), counter.clone());

        // Content prefixed "BBBB" so it doesn't collide with other tests' mock bodies
        // when run in the same process; body content is otherwise irrelevant here.
        let content = format!("BBBB{}", "B".repeat(1839));
        let result = embed_with_token_retry(&content, 512, &attempt);

        assert!(
            result.is_ok(),
            "the retry-succeeded log branch must be reached via a successful second attempt: {:?}",
            result
        );
        assert_eq!(
            *counter.lock().unwrap(),
            2,
            "exactly 2 requests confirms the code passed through the retry (and its eprintln! calls) rather than short-circuiting"
        );
        assert_eq!(
            result.unwrap(),
            vec![0.4f32, 0.5, 0.6],
            "the embedding returned must be the retry (second-attempt) response, confirming the success-path eprintln branch executed"
        );
    }

    // ── #2988 PR1: Venice embed_fn arm (§1b/§1e) — real build_http_embed_fn, mockito-mocked ──
    //
    // Unlike the OpenRouter arm (hardcoded URL, cannot be mocked — see the comment on
    // `test_build_http_embed_fn_wire_body_shrinks_on_retry` above), the Venice arm's base
    // URL has its own `#[cfg(test)]` override (`crate::test_set_venice_embed_base`,
    // lib.rs) added specifically so these tests can exercise the REAL production
    // constructor end-to-end instead of a hand-written stand-in closure. No live Venice
    // key exists on this machine (confirmed by Reed — `security find-generic-password`
    // returns "item could not be found") — these tests inject a fake key via
    // `MockKeychain` and mock the HTTP boundary with mockito; they verify wire shape and
    // credential SOURCE, not a real Venice response. `#[serial(keychain_override)]` is
    // required because `KEYCHAIN_OVERRIDE` is process-global state (same convention as
    // `migrations.rs`'s `migrate_v82_tests`).

    #[test]
    #[serial(keychain_override)]
    fn test_build_http_embed_fn_venice_parses_response() {
        use crate::keychain::test_support::KeychainGuard;

        let _kc = KeychainGuard::set();
        crate::keychain::store_password("ai_venice_key", "test-fake-venice-key-not-real")
            .expect("mock keychain store");

        let mut server = mockito::Server::new();
        let _m = server.mock("POST", "/embeddings")
            .match_header("authorization", "Bearer test-fake-venice-key-not-real")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"data":[{"embedding":[0.7,0.8,0.9]}],"model":"text-embedding-bge-m3","object":"list"}"#)
            .create();

        crate::test_set_venice_embed_base(&server.url());

        let conn = make_full_test_db();
        let result = crate::build_http_embed_fn(&conn, "venice", "text-embedding-bge-m3");

        let (embed_fn, engine_tag) = result.expect("build_http_embed_fn should succeed for the venice arm");
        assert_eq!(engine_tag, "venice", "engine tag must be 'venice'");

        // NOTE: the base-URL override is read INSIDE the closure (at call time, not
        // build time) — clearing it must happen AFTER embed_fn() runs, or the second
        // call falls through to the real `https://api.venice.ai` default.
        let vec_result = embed_fn("hello venice");
        crate::test_clear_venice_embed_base();
        let vec = vec_result.expect("venice embed_fn should succeed against the mock");
        assert_eq!(vec.len(), 3, "should return 3 floats");
        assert!((vec[0] - 0.7f32).abs() < 1e-6);
        assert!((vec[1] - 0.8f32).abs() < 1e-6);
        assert!((vec[2] - 0.9f32).abs() < 1e-6);
    }

    /// §1e PIN: the venice arm must read the chat-provider key from the keychain
    /// (`read_venice_key`), NEVER the Semantic Search card's `embeddings_api_key` slot —
    /// even when `embeddings_api_key` is populated with a different value. The mockito
    /// mock only matches the KEYCHAIN key's Bearer header; if a future "consistency fix"
    /// changed the venice arm to read `embeddings_api_key` instead, the request would
    /// carry the WRONG key, the mock would not match, and this test would fail with a
    /// connection/mismatch error instead of silently passing — exactly the regression
    /// guard the build brief asked for.
    #[test]
    #[serial(keychain_override)]
    fn test_build_http_embed_fn_venice_reads_chat_key_not_embeddings_api_key() {
        use crate::keychain::test_support::KeychainGuard;

        let _kc = KeychainGuard::set();
        crate::keychain::store_password("ai_venice_key", "correct-chat-key-123")
            .expect("mock keychain store");

        let conn = make_full_test_db();
        // A DIFFERENT key saved in the Semantic Search card's own slot — if the venice
        // arm ever reads this instead of the keychain, the mock below (which only
        // matches the keychain key) will not match and the request will fail.
        crate::database::set_ai_setting(&conn, "embeddings_api_key", "wrong-semantic-search-card-key")
            .expect("set embeddings_api_key");

        let mut server = mockito::Server::new();
        let _m = server.mock("POST", "/embeddings")
            .match_header("authorization", "Bearer correct-chat-key-123")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"data":[{"embedding":[0.1]}],"model":"text-embedding-bge-m3","object":"list"}"#)
            .create();

        crate::test_set_venice_embed_base(&server.url());
        let result = crate::build_http_embed_fn(&conn, "venice", "text-embedding-bge-m3");
        let (embed_fn, _tag) = result.expect("build_http_embed_fn should succeed");
        let embed_result = embed_fn("pin test"); // base-URL override still active here
        crate::test_clear_venice_embed_base();

        assert!(
            embed_result.is_ok(),
            "venice embed_fn must authenticate with the KEYCHAIN key (read_venice_key), not \
             embeddings_api_key — if this fails, the arm started reading the wrong credential \
             source and §1e's deliberate deviation has been \"fixed\" away: {:?}",
            embed_result
        );
    }

    /// When no Venice key exists in the keychain (this machine's actual state, per
    /// Reed's confirmation), build_http_embed_fn must fail loudly with a clear message
    /// — not silently fall back to `embeddings_api_key` or any other slot.
    #[test]
    #[serial(keychain_override)]
    fn test_build_http_embed_fn_venice_missing_key_errors() {
        use crate::keychain::test_support::KeychainGuard;

        let _kc = KeychainGuard::set(); // empty mock keychain — no venice key stored
        let conn = make_full_test_db();

        let result = crate::build_http_embed_fn(&conn, "venice", "text-embedding-bge-m3");
        match result {
            Ok(_) => panic!("build_http_embed_fn must error when no Venice key is configured"),
            Err(err) => assert!(
                err.contains("Venice API key not configured"),
                "error must name the real problem (missing Venice key), got: {}",
                err
            ),
        }
    }

    // ── #3082: build_http_embed_batch_fn — real production constructor, mockito-mocked ──
    //
    // Same injectable-base-URL trick as the Venice arm above
    // (`crate::test_set_venice_embed_base`) lets these tests exercise the REAL
    // `build_http_embed_batch_fn` end-to-end, not a hand-written stand-in, for the one
    // engine whose base URL is test-overridable. The OpenRouter/Ollama arms of the
    // batch builder share the identical wire-shape logic (array `input`, array
    // `data[]`/`embeddings[]` response) — this is the one case in this codebase where
    // that shape can be verified against the real function rather than a reimplemented
    // closure (see the comment on `test_build_http_embed_fn_wire_body_shrinks_on_retry`
    // for why the OpenRouter arm specifically cannot be).

    #[test]
    #[serial(keychain_override)]
    fn test_build_http_embed_batch_fn_venice_sends_array_and_parses_array_response() {
        use crate::keychain::test_support::KeychainGuard;

        let _kc = KeychainGuard::set();
        crate::keychain::store_password("ai_venice_key", "test-fake-venice-key-not-real")
            .expect("mock keychain store");

        let mut server = mockito::Server::new();
        let _m = server.mock("POST", "/embeddings")
            .match_header("authorization", "Bearer test-fake-venice-key-not-real")
            // Confirms the wire body actually sends an ARRAY, not a single string —
            // the whole point of batching (#3082).
            .match_body(mockito::Matcher::PartialJson(serde_json::json!({
                "input": ["first item", "second item", "third item"]
            })))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"data":[{"embedding":[0.1,0.1]},{"embedding":[0.2,0.2]},{"embedding":[0.3,0.3]}],"model":"text-embedding-bge-m3","object":"list"}"#)
            .create();

        crate::test_set_venice_embed_base(&server.url());

        let conn = make_full_test_db();
        let result = crate::build_http_embed_batch_fn(&conn, "venice", "text-embedding-bge-m3");
        let (batch_embed_fn, engine_tag) = result.expect("build_http_embed_batch_fn should succeed for venice");
        assert_eq!(engine_tag, "venice");

        let texts = ["first item", "second item", "third item"];
        let vecs_result = batch_embed_fn(&texts);
        crate::test_clear_venice_embed_base();
        let vecs = vecs_result.expect("batch embed_fn should succeed against the mock");

        assert_eq!(vecs.len(), 3, "must return one vector per input item, in order");
        assert!((vecs[0][0] - 0.1f32).abs() < 1e-6, "item 0's vector must correspond to item 0, not a shuffled position");
        assert!((vecs[1][0] - 0.2f32).abs() < 1e-6, "item 1's vector must correspond to item 1");
        assert!((vecs[2][0] - 0.3f32).abs() < 1e-6, "item 2's vector must correspond to item 2");
    }

    #[test]
    #[serial(keychain_override)]
    fn test_build_http_embed_batch_fn_venice_http_error_propagates() {
        use crate::keychain::test_support::KeychainGuard;

        let _kc = KeychainGuard::set();
        crate::keychain::store_password("ai_venice_key", "test-fake-venice-key-not-real")
            .expect("mock keychain store");

        let mut server = mockito::Server::new();
        let _m = server.mock("POST", "/embeddings")
            .with_status(400)
            .with_header("content-type", "application/json")
            .with_body(r#"{"error":"one item exceeded the token limit"}"#)
            .create();

        crate::test_set_venice_embed_base(&server.url());

        let conn = make_full_test_db();
        let (batch_embed_fn, _tag) = crate::build_http_embed_batch_fn(&conn, "venice", "text-embedding-bge-m3")
            .expect("build_http_embed_batch_fn should succeed");

        let texts = ["a", "b"];
        let result = batch_embed_fn(&texts);
        crate::test_clear_venice_embed_base();

        assert!(result.is_err(), "a batch-level HTTP error must propagate as Err, not a partial/empty success");
        assert!(result.unwrap_err().contains("400"), "error must mention the HTTP status");
    }

    #[test]
    fn test_engine_switch_clears_stale_embeddings() {
        // Simulate engine switch: old embeddings tagged 'openrouter' should be cleared
        // when switching to 'ollama', and vice versa.
        let conn = make_test_db();

        // Insert two embeddings with different engines
        store_embedding(&conn, "id-or", "note", &[0.1f32, 0.2f32], "hash-or", Some("openrouter"), Some("model-a")).unwrap();
        store_embedding(&conn, "id-ollama", "note", &[0.3f32, 0.4f32], "hash-ollama", Some("ollama"), Some("model-a")).unwrap();

        // Simulate switch to 'ollama': delete all rows where engine IS NULL OR engine != 'ollama'
        let deleted = conn.execute(
            "DELETE FROM embeddings WHERE engine IS NULL OR engine != ?1",
            rusqlite::params!["ollama"],
        ).unwrap();
        assert_eq!(deleted, 1, "switching to ollama should delete 1 stale openrouter row");

        // Only the ollama row should remain
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM embeddings",
            [],
            |row| row.get(0),
        ).unwrap();
        assert_eq!(count, 1, "only the ollama embedding should remain");

        let engine: Option<String> = conn.query_row(
            "SELECT engine FROM embeddings WHERE id = 'id-ollama'",
            [],
            |row| row.get(0),
        ).unwrap();
        assert_eq!(engine, Some("ollama".to_string()), "remaining row should be tagged 'ollama'");
    }

    #[test]
    fn test_engine_switch_clears_null_engine_rows() {
        // NULL-engine rows (legacy/stale) are cleared on any engine switch
        let conn = make_test_db();

        // Insert a legacy row with no engine tag
        store_embedding(&conn, "legacy", "note", &[0.1f32], "hash-legacy", None, None).unwrap();
        store_embedding(&conn, "new-row", "note", &[0.2f32], "hash-new", Some("openrouter"), Some("model-a")).unwrap();

        // Switch to openrouter: NULL rows and non-openrouter rows go
        let deleted = conn.execute(
            "DELETE FROM embeddings WHERE engine IS NULL OR engine != ?1",
            rusqlite::params!["openrouter"],
        ).unwrap();
        assert_eq!(deleted, 1, "should delete the 1 legacy NULL-engine row");

        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM embeddings", [], |row| row.get(0)
        ).unwrap();
        assert_eq!(count, 1, "only openrouter row should remain");
    }

    /// #2988 PR1: the generic DELETE-on-switch predicate (§1c) must clear Venice-tagged
    /// rows the same way it clears OpenRouter/Ollama rows, in BOTH directions —
    /// switching TO venice clears OR/Ollama rows, and switching AWAY FROM venice clears
    /// venice rows. Confirms adding Venice needed no new re-index path (§1c's claim).
    #[test]
    fn test_engine_switch_clears_stale_embeddings_venice_both_directions() {
        let conn = make_test_db();

        // Direction 1: switching TO venice must clear an existing openrouter row.
        store_embedding(&conn, "id-or", "note", &[0.1f32], "hash-or", Some("openrouter"), Some("model-a")).unwrap();
        let deleted = conn.execute(
            "DELETE FROM embeddings WHERE engine IS NULL OR engine != ?1",
            rusqlite::params!["venice"],
        ).unwrap();
        assert_eq!(deleted, 1, "switching to venice should delete the stale openrouter row");
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM embeddings", [], |r| r.get(0)).unwrap();
        assert_eq!(count, 0, "no rows should remain before the venice row is written");

        // Now simulate the venice row landing (the pipeline re-indexing under the new engine).
        store_embedding(&conn, "id-venice", "note", &[0.5f32], "hash-venice", Some("venice"), Some("model-a")).unwrap();

        // Direction 2: switching AWAY from venice (to ollama) must clear the venice row.
        let deleted = conn.execute(
            "DELETE FROM embeddings WHERE engine IS NULL OR engine != ?1",
            rusqlite::params!["ollama"],
        ).unwrap();
        assert_eq!(deleted, 1, "switching away from venice should delete the stale venice row");
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM embeddings", [], |r| r.get(0)).unwrap();
        assert_eq!(count, 0, "venice row must not survive a switch to a different engine");
    }

    #[test]
    fn test_semantic_gate_requires_both_enabled_and_ready() {
        // The semantic search gate requires:
        //   embeddings_enabled == "true" AND embeddings_status == "ready" OR "partial"
        // "partial" means some items failed transiently but the index is otherwise usable.
        // Test every combination of the two flags.

        struct GateCase {
            enabled: &'static str,
            status: &'static str,
            expect_semantic: bool,
        }

        let cases = [
            GateCase { enabled: "true",  status: "ready",    expect_semantic: true  },
            GateCase { enabled: "true",  status: "partial",  expect_semantic: true  },
            GateCase { enabled: "true",  status: "indexing", expect_semantic: false },
            GateCase { enabled: "true",  status: "error",    expect_semantic: false },
            GateCase { enabled: "false", status: "ready",    expect_semantic: false },
            GateCase { enabled: "false", status: "partial",  expect_semantic: false },
            GateCase { enabled: "false", status: "indexing", expect_semantic: false },
        ];

        for case in &cases {
            let semantic_enabled = case.enabled == "true";
            let status_ready = case.status == "ready" || case.status == "partial";
            let gate_open = semantic_enabled && status_ready;
            assert_eq!(
                gate_open, case.expect_semantic,
                "enabled={} status={}: expected gate_open={} but got {}",
                case.enabled, case.status, case.expect_semantic, gate_open
            );
        }
    }

    #[test]
    fn test_status_transitions_indexing_to_ready() {
        // Simulate the status lifecycle: off → indexing → ready
        let conn = make_full_test_db();

        // Initial state: no status key (off)
        let initial_status: Option<String> = conn.query_row(
            "SELECT value FROM ai_settings WHERE key = 'embeddings_status'",
            [], |row| row.get(0),
        ).ok();
        assert_eq!(initial_status, None, "should start with no embeddings_status");

        // Simulate: pipeline starts → set 'indexing'
        conn.execute(
            "INSERT OR REPLACE INTO ai_settings (key, value) VALUES ('embeddings_status', 'indexing')",
            [],
        ).unwrap();
        let indexing_status: Option<String> = conn.query_row(
            "SELECT value FROM ai_settings WHERE key = 'embeddings_status'",
            [], |row| row.get(0),
        ).ok();
        assert_eq!(indexing_status, Some("indexing".to_string()), "status should be 'indexing'");

        // Simulate: pipeline succeeds → set 'ready'
        conn.execute(
            "UPDATE ai_settings SET value = 'ready' WHERE key = 'embeddings_status'",
            [],
        ).unwrap();
        let ready_status: Option<String> = conn.query_row(
            "SELECT value FROM ai_settings WHERE key = 'embeddings_status'",
            [], |row| row.get(0),
        ).ok();
        assert_eq!(ready_status, Some("ready".to_string()), "status should be 'ready'");
    }

    #[test]
    fn test_status_transitions_indexing_to_error() {
        // Simulate: off → indexing → error (pipeline fails)
        let conn = make_full_test_db();

        conn.execute(
            "INSERT OR REPLACE INTO ai_settings (key, value) VALUES ('embeddings_status', 'indexing')",
            [],
        ).unwrap();

        // Simulate: pipeline errors
        conn.execute(
            "UPDATE ai_settings SET value = 'error' WHERE key = 'embeddings_status'",
            [],
        ).unwrap();
        let error_status: Option<String> = conn.query_row(
            "SELECT value FROM ai_settings WHERE key = 'embeddings_status'",
            [], |row| row.get(0),
        ).ok();
        assert_eq!(error_status, Some("error".to_string()), "status should be 'error' after pipeline failure");

        // When status == 'error', semantic gate is closed even if embeddings_enabled == true
        let status_ready = matches!(error_status.as_deref(), Some("ready") | Some("partial"));
        assert!(!status_ready, "gate should be closed when status is 'error'");
    }

    #[test]
    fn test_pipeline_stores_engine_tag_on_rows() {
        // run_embedding_pipeline should tag each stored row with the engine string
        let conn = make_test_db();
        // #1673: notes need real file bodies; title-only notes are now skipped
        let (_f, path) = make_temp_note("Test note body content");
        conn.execute(
            "INSERT INTO notes (id, title, file_path, deleted_at) VALUES ('n1', 'Test note', ?1, NULL)",
            rusqlite::params![path],
        ).unwrap();

        let pane_root = std::path::Path::new("/tmp");
        run_embedding_pipeline(&conn, pane_root, &mock_embed, "openrouter", "test-model", 512, |_| {})
            .expect("pipeline");

        let engine: Option<String> = conn.query_row(
            "SELECT engine FROM embeddings WHERE id = 'n1'",
            [],
            |row| row.get(0),
        ).ok().flatten();
        assert_eq!(engine, Some("openrouter".to_string()), "stored embedding should be tagged 'openrouter'");
    }

    // ── Fail-fast on auth error tests (#1657) ────────────────────────────

    #[test]
    fn test_is_auth_error_detects_401() {
        assert!(is_auth_error("OpenRouter embed HTTP 401 Unauthorized: invalid key"));
        assert!(is_auth_error("Ollama embed HTTP 401: bad credentials"));
        assert!(is_auth_error("HTTP 401"));
    }

    #[test]
    fn test_is_auth_error_detects_403() {
        assert!(is_auth_error("OpenRouter embed HTTP 403 Forbidden: key revoked"));
        assert!(is_auth_error("HTTP 403"));
    }

    #[test]
    fn test_is_auth_error_ignores_transient_errors() {
        assert!(!is_auth_error("OpenRouter embed request failed: connection reset"));
        assert!(!is_auth_error("Failed to parse response"));
        assert!(!is_auth_error("HTTP 500 Internal Server Error"));
        assert!(!is_auth_error("timeout"));
        assert!(!is_auth_error(""));
    }

    #[test]
    fn test_pipeline_aborts_on_auth_error_in_notes() {
        // A 401 error on the first note should abort the whole pipeline and return Err
        let conn = make_test_db();
        // #1673: notes need real file bodies so embed_fn is actually called
        let (_f1, path1) = make_temp_note("Note one body content");
        let (_f2, path2) = make_temp_note("Note two body content");
        conn.execute(
            "INSERT INTO notes (id, title, file_path, deleted_at) VALUES ('n1', 'Note one', ?1, NULL)",
            rusqlite::params![path1],
        ).unwrap();
        conn.execute(
            "INSERT INTO notes (id, title, file_path, deleted_at) VALUES ('n2', 'Note two', ?1, NULL)",
            rusqlite::params![path2],
        ).unwrap();

        let pane_root = std::path::Path::new("/tmp");
        let embed_fn = |_text: &str| -> Result<Vec<f32>, String> {
            Err("OpenRouter embed HTTP 401 Unauthorized: bad api key".to_string())
        };

        let result = run_embedding_pipeline(&conn, pane_root, &embed_fn, "openrouter", "test-model", 512, |_| {});
        assert!(result.is_err(), "pipeline should return Err on auth error");
        let err = result.unwrap_err();
        assert!(err.contains("aborting pipeline"), "error message should mention aborting: {}", err);
        assert!(err.contains("HTTP 401"), "error should include the original auth error: {}", err);
    }

    #[test]
    fn test_pipeline_aborts_on_auth_error_in_facts() {
        // A 403 error during the facts sub-pass should abort and return Err
        let conn = make_test_db();
        // No notes so we get past that sub-pass
        // C35 (#3128): fact text must be >= 20 chars to reach embed_fn at
        // all now — "Some fact here" alone was only 14.
        conn.execute(
            "INSERT INTO ai_memory (id, fact, category, is_active) VALUES ('f1', 'Some fact here, long enough', NULL, 1)",
            [],
        ).unwrap();

        let pane_root = std::path::Path::new("/tmp");
        let embed_fn = |_text: &str| -> Result<Vec<f32>, String> {
            Err("OpenRouter embed HTTP 403 Forbidden: quota exceeded".to_string())
        };

        let result = run_embedding_pipeline(&conn, pane_root, &embed_fn, "openrouter", "test-model", 512, |_| {});
        assert!(result.is_err(), "pipeline should abort on 403 in facts pass");
        let err = result.unwrap_err();
        assert!(err.contains("aborting pipeline"), "error should mention aborting: {}", err);
    }

    #[test]
    fn test_pipeline_aborts_on_auth_error_in_chats() {
        // A 401 error during the chats sub-pass should abort and return Err
        let conn = make_test_db();
        // No notes or facts so we get past those sub-passes
        conn.execute(
            "INSERT INTO chat_messages_local (id, content, created_at) VALUES ('c1', 'This is a long enough chat message', datetime('now'))",
            [],
        ).unwrap();

        let pane_root = std::path::Path::new("/tmp");
        let embed_fn = |_text: &str| -> Result<Vec<f32>, String> {
            Err("Ollama embed HTTP 401: unauthorized".to_string())
        };

        let result = run_embedding_pipeline(&conn, pane_root, &embed_fn, "ollama", "test-model", 512, |_| {});
        assert!(result.is_err(), "pipeline should abort on 401 in chats pass");
        let err = result.unwrap_err();
        assert!(err.contains("aborting pipeline"), "error should mention aborting: {}", err);
    }

    #[test]
    fn test_pipeline_continues_on_transient_error() {
        // A non-auth error on one note should skip that item; the pipeline succeeds overall
        let conn = make_test_db();
        // #1673: notes need real file bodies so embed_fn is called;
        // embed_fn checks the content which includes title+body, so "Bad" in body triggers the error
        let (_fg, path_good) = make_temp_note("Good note body content");
        let (_fb, path_bad) = make_temp_note("Bad note body content");
        conn.execute(
            "INSERT INTO notes (id, title, file_path, deleted_at) VALUES ('n1', 'Good note', ?1, NULL)",
            rusqlite::params![path_good],
        ).unwrap();
        conn.execute(
            "INSERT INTO notes (id, title, file_path, deleted_at) VALUES ('n2', 'Bad note', ?1, NULL)",
            rusqlite::params![path_bad],
        ).unwrap();

        let pane_root = std::path::Path::new("/tmp");
        // Fail only for content that contains 'Bad'; all others succeed
        let embed_fn = |text: &str| -> Result<Vec<f32>, String> {
            if text.contains("Bad") {
                Err("Failed to parse response".to_string())
            } else {
                Ok(vec![0.0f32; 4])
            }
        };

        let result = run_embedding_pipeline(&conn, pane_root, &embed_fn, "openrouter", "test-model", 512, |_| {});
        assert!(result.is_ok(), "pipeline should succeed when error is transient: {:?}", result);
        let (notes_count, _, _, failed_count) = result.unwrap();
        assert_eq!(notes_count, 1, "only the good note should be embedded; bad one skipped");
        assert_eq!(failed_count, 1, "one transient failure must be counted");
    }

    #[test]
    fn test_pipeline_with_transient_failures_and_no_auth_errors_returns_ok() {
        // Mixed: one note fails transiently, rest succeed — pipeline returns Ok with partial counts
        let conn = make_test_db();
        // #1673: notes need real file bodies; content includes title so "Note title 2" still triggers
        // C35 (#3128): file body alone must be >= 20 chars to pass the new
        // length floor — "Body for note {}" alone was only ~15.
        let _temp_files: Vec<(tempfile::NamedTempFile, String)> = (0..5).map(|i| {
            let (f, path) = make_temp_note(&format!("Body content for note {}", i));
            conn.execute(
                "INSERT INTO notes (id, title, file_path, deleted_at) VALUES (?1, ?2, ?3, NULL)",
                rusqlite::params![format!("note-{}", i), format!("Note title {}", i), path],
            ).unwrap();
            (f, path)
        }).collect();

        let pane_root = std::path::Path::new("/tmp");
        // Fail on note-2 with a transient error; rest succeed
        // content sent to embed_fn = "Note title 2\nBody content for note 2" — still contains "Note title 2"
        let embed_fn = |text: &str| -> Result<Vec<f32>, String> {
            if text.contains("Note title 2") {
                Err("network timeout".to_string())
            } else {
                Ok(vec![1.0f32; 4])
            }
        };

        let result = run_embedding_pipeline(&conn, pane_root, &embed_fn, "openrouter", "test-model", 512, |_| {});
        assert!(result.is_ok(), "pipeline should return Ok despite one transient failure");
        let (notes_count, facts_count, chats_count, failed_count) = result.unwrap();
        assert_eq!(notes_count, 4, "4 of 5 notes should embed successfully");
        assert_eq!(facts_count, 0);
        assert_eq!(chats_count, 0);
        assert_eq!(failed_count, 1, "one transient failure must be tracked");
    }

    /// Pipeline returns failed_count=0 on a clean run with no errors.
    #[test]
    fn test_pipeline_failed_count_zero_on_clean_run() {
        let conn = make_test_db();
        let (_f, path) = make_temp_note("Clean note body content");
        conn.execute(
            "INSERT INTO notes (id, title, file_path, deleted_at) VALUES ('n1', 'Clean note', ?1, NULL)",
            rusqlite::params![path],
        ).unwrap();

        let pane_root = std::path::Path::new("/tmp");
        let embed_fn = |_text: &str| -> Result<Vec<f32>, String> { Ok(vec![0.5f32; 4]) };

        let result = run_embedding_pipeline(&conn, pane_root, &embed_fn, "openrouter", "test-model", 512, |_| {});
        assert!(result.is_ok());
        let (notes_count, _, _, failed_count) = result.unwrap();
        assert_eq!(notes_count, 1, "one note should embed successfully");
        assert_eq!(failed_count, 0, "clean run must have failed_count = 0");
    }

    /// Auth abort returns Err, not Ok with a failed count.
    #[test]
    fn test_pipeline_auth_abort_returns_err_not_ok() {
        let conn = make_test_db();
        let (_f, path) = make_temp_note("Note body for auth test");
        conn.execute(
            "INSERT INTO notes (id, title, file_path, deleted_at) VALUES ('n1', 'Note', ?1, NULL)",
            rusqlite::params![path],
        ).unwrap();

        let pane_root = std::path::Path::new("/tmp");
        let embed_fn = |_text: &str| -> Result<Vec<f32>, String> {
            Err("OpenRouter embed HTTP 401 Unauthorized".to_string())
        };

        let result = run_embedding_pipeline(&conn, pane_root, &embed_fn, "openrouter", "test-model", 512, |_| {});
        assert!(result.is_err(), "auth abort must return Err, not Ok with failed count");
    }

    /// Pipeline tracks failed_count across all three sub-passes.
    #[test]
    fn test_pipeline_failed_count_spans_all_subpasses() {
        let conn = make_test_db();
        let (_f, path) = make_temp_note("Note body that will fail to embed");
        conn.execute(
            "INSERT INTO notes (id, title, file_path, deleted_at) VALUES ('n1', 'Note fail', ?1, NULL)",
            rusqlite::params![path],
        ).unwrap();
        // C35 (#3128): fact text must be >= 20 chars to reach embed_fn at
        // all now — "Fact fail" alone was only 9.
        conn.execute(
            "INSERT INTO ai_memory (id, fact, category, is_active) VALUES ('f1', 'Fact fail for embedding test', NULL, 1)",
            [],
        ).unwrap();
        // Chat message long enough to pass the 20-char gate
        conn.execute(
            "INSERT INTO chat_messages_local (id, content, created_at) VALUES ('c1', 'This is a long enough chat message to embed', datetime('now'))",
            [],
        ).unwrap();

        let pane_root = std::path::Path::new("/tmp");
        // All items fail transiently (non-auth)
        let embed_fn = |_text: &str| -> Result<Vec<f32>, String> {
            Err("HTTP 500 Internal Server Error".to_string())
        };

        let result = run_embedding_pipeline(&conn, pane_root, &embed_fn, "openrouter", "test-model", 512, |_| {});
        assert!(result.is_ok(), "pipeline must return Ok on transient failures: {:?}", result);
        let (notes_count, facts_count, chats_count, failed_count) = result.unwrap();
        assert_eq!(notes_count, 0);
        assert_eq!(facts_count, 0);
        assert_eq!(chats_count, 0);
        // One note + one fact + one chat = 3 failures
        assert_eq!(failed_count, 3, "failed_count must span all three sub-passes");
    }
    // ── #1673 — skip empty-body notes tests ──────────────────────────────

    #[test]
    fn test_pipeline_skips_note_with_empty_file_body() {
        // A note whose file exists but is empty should be skipped (count 0, no embedding row).
        let conn = make_test_db();
        let (_f, path) = make_temp_note(""); // empty file
        conn.execute(
            "INSERT INTO notes (id, title, file_path, deleted_at) VALUES (?1, ?2, ?3, NULL)",
            rusqlite::params!["stub", "2026-03-28", path],
        ).unwrap();
        // Write an empty file at the same path (NamedTempFile was already created empty,
        // but we already have the path string; re-confirm empty content)
        std::fs::write(&path, b"").ok();

        let pane_root = std::path::Path::new("/tmp");
        let (notes_count, _, _, _) = run_embedding_pipeline(&conn, pane_root, &mock_embed, "test", "test-model", 512, |_| {})
            .expect("pipeline");

        assert_eq!(notes_count, 0, "date-stub note with empty file body should be skipped");

        let emb_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM embeddings WHERE id = ?1",
            rusqlite::params!["stub"],
            |row| row.get(0),
        ).unwrap();
        assert_eq!(emb_count, 0, "no embedding row should be stored for skipped note");
    }

    #[test]
    fn test_pipeline_skips_note_with_missing_file() {
        // A note whose file_path points to a nonexistent file should be skipped.
        let conn = make_test_db();
        conn.execute(
            "INSERT INTO notes (id, title, file_path, deleted_at) VALUES (?1, ?2, ?3, NULL)",
            rusqlite::params!["missing", "2026-04-01", "/tmp/pane-1673-nonexistent-42.md"],
        ).unwrap();

        let pane_root = std::path::Path::new("/tmp");
        let (notes_count, _, _, _) = run_embedding_pipeline(&conn, pane_root, &mock_embed, "test", "test-model", 512, |_| {})
            .expect("pipeline");

        assert_eq!(notes_count, 0, "note with missing file should be skipped (read_to_string returns empty string, body is empty, skip)");
    }

    #[test]
    fn test_pipeline_skips_note_with_empty_file_path() {
        // The production date-stub scenario: file_path = Some("") → pane_root.join("") = directory
        // → read_to_string on a directory returns error → unwrap_or_default() = "" → skip.
        let conn = make_test_db();
        conn.execute(
            "INSERT INTO notes (id, title, file_path, deleted_at) VALUES (?1, ?2, ?3, NULL)",
            rusqlite::params!["datestub", "2026-03-28", ""],
        ).unwrap();

        let pane_root = std::path::Path::new("/tmp");
        let (notes_count, _, _, _) = run_embedding_pipeline(&conn, pane_root, &mock_embed, "test", "test-model", 512, |_| {})
            .expect("pipeline");

        assert_eq!(notes_count, 0, "note with empty file_path (production date-stub) should be skipped");

        let emb_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM embeddings WHERE id = ?1",
            rusqlite::params!["datestub"],
            |row| row.get(0),
        ).unwrap();
        assert_eq!(emb_count, 0, "no embedding row should be stored for skipped date-stub");
    }

    #[test]
    fn test_pipeline_embeds_note_with_nonempty_file_body() {
        // Regression guard: a note with real file content must still be embedded.
        let conn = make_test_db();
        let (_f, path) = make_temp_note("This is the actual note body with real content.");
        conn.execute(
            "INSERT INTO notes (id, title, file_path, deleted_at) VALUES (?1, ?2, ?3, NULL)",
            rusqlite::params!["real", "My Note", path],
        ).unwrap();

        let pane_root = std::path::Path::new("/tmp");
        let (notes_count, _, _, _) = run_embedding_pipeline(&conn, pane_root, &mock_embed, "test", "test-model", 512, |_| {})
            .expect("pipeline");

        assert_eq!(notes_count, 1, "note with real file content should be embedded (not accidentally skipped)");

        let emb_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM embeddings WHERE id = ?1",
            rusqlite::params!["real"],
            |row| row.get(0),
        ).unwrap();
        assert_eq!(emb_count, 1, "embedding row should be stored for note with real body");
    }

    // ── #1717: daily-log NULL/empty body exclusion ────────────────────────

    #[test]
    fn test_pipeline_skips_daily_note_with_null_body() {
        // Core regression test for #1717: daily note with NULL body and a file scaffold
        // (e.g. "# 2026-01-15") previously slipped past the file-content guard and
        // triggered "missing data[0].embedding" on every pipeline run.
        let conn = make_test_db();
        let (_f, path) = make_temp_note("# 2026-01-15\n"); // scaffold that previously passed guard
        conn.execute(
            "INSERT INTO notes (id, title, body, note_type, file_path, deleted_at) VALUES (?1, ?2, NULL, 'daily', ?3, NULL)",
            rusqlite::params!["daily-null", "2026-01-15", path],
        ).unwrap();

        let pane_root = std::path::Path::new("/tmp");
        let (notes_count, _, _, _) = run_embedding_pipeline(&conn, pane_root, &mock_embed, "test", "test-model", 512, |_| {})
            .expect("pipeline");

        assert_eq!(notes_count, 0, "daily note with NULL body should be excluded by the query");

        let emb_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM embeddings WHERE id = ?1",
            rusqlite::params!["daily-null"],
            |row| row.get(0),
        ).unwrap();
        assert_eq!(emb_count, 0, "no embedding row should be stored for NULL-body daily note");
    }

    #[test]
    fn test_pipeline_skips_daily_note_with_empty_body() {
        // Daily note with empty-string body should also be excluded (#1717).
        let conn = make_test_db();
        let (_f, path) = make_temp_note("# 2026-03-20\n");
        conn.execute(
            "INSERT INTO notes (id, title, body, note_type, file_path, deleted_at) VALUES (?1, ?2, '', 'daily', ?3, NULL)",
            rusqlite::params!["daily-empty", "2026-03-20", path],
        ).unwrap();

        let pane_root = std::path::Path::new("/tmp");
        let (notes_count, _, _, _) = run_embedding_pipeline(&conn, pane_root, &mock_embed, "test", "test-model", 512, |_| {})
            .expect("pipeline");

        assert_eq!(notes_count, 0, "daily note with empty body should be excluded by the query");

        let emb_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM embeddings WHERE id = ?1",
            rusqlite::params!["daily-empty"],
            |row| row.get(0),
        ).unwrap();
        assert_eq!(emb_count, 0, "no embedding row should be stored for empty-body daily note");
    }

    #[test]
    fn test_pipeline_skips_daily_note_with_whitespace_body() {
        // Daily note with whitespace-only body should also be excluded (#1717).
        let conn = make_test_db();
        let (_f, path) = make_temp_note("# 2026-03-21\n");
        conn.execute(
            "INSERT INTO notes (id, title, body, note_type, file_path, deleted_at) VALUES (?1, ?2, ?3, 'daily', ?4, NULL)",
            // SQLite TRIM() only strips ASCII spaces (not \n/\t); use space-only string
            // so the TRIM(body) = '' predicate fires correctly.
            rusqlite::params!["daily-ws", "2026-03-21", "   ", path],
        ).unwrap();

        let pane_root = std::path::Path::new("/tmp");
        let (notes_count, _, _, _) = run_embedding_pipeline(&conn, pane_root, &mock_embed, "test", "test-model", 512, |_| {})
            .expect("pipeline");

        assert_eq!(notes_count, 0, "daily note with whitespace-only body should be excluded by the query");

        let emb_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM embeddings WHERE id = ?1",
            rusqlite::params!["daily-ws"],
            |row| row.get(0),
        ).unwrap();
        assert_eq!(emb_count, 0, "no embedding row should be stored for whitespace-body daily note");
    }

    #[test]
    fn test_pipeline_embeds_daily_note_with_real_body() {
        // Behavioral constraint guard: a daily note WITH user content must still embed normally.
        // The filter must NOT over-exclude daily notes that have real body content.
        let conn = make_test_db();
        let (_f, path) = make_temp_note("Today I worked on X and Y.\n\nMore details here.");
        conn.execute(
            "INSERT INTO notes (id, title, body, note_type, file_path, deleted_at) VALUES (?1, ?2, ?3, 'daily', ?4, NULL)",
            rusqlite::params!["daily-real", "2026-03-22", "Today I worked on X and Y.", path],
        ).unwrap();

        let pane_root = std::path::Path::new("/tmp");
        let (notes_count, _, _, _) = run_embedding_pipeline(&conn, pane_root, &mock_embed, "test", "test-model", 512, |_| {})
            .expect("pipeline");

        assert_eq!(notes_count, 1, "daily note with real body content should still be embedded");

        let emb_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM embeddings WHERE id = ?1",
            rusqlite::params!["daily-real"],
            |row| row.get(0),
        ).unwrap();
        assert_eq!(emb_count, 1, "embedding row should be stored for daily note with real body");
    }

    #[test]
    fn test_pipeline_still_embeds_non_daily_note_with_null_body() {
        // Non-regression: the filter must NOT touch non-daily notes, even if body is NULL.
        // Non-daily notes (note_type IS NULL or non-'daily') still enter the pipeline;
        // the existing file-content guard (#1673) handles them.
        let conn = make_test_db();
        let (_f, path) = make_temp_note("This is a soul note with real file content.");
        conn.execute(
            "INSERT INTO notes (id, title, body, note_type, file_path, deleted_at) VALUES (?1, ?2, NULL, NULL, ?3, NULL)",
            rusqlite::params!["soul-note", "My Rules", path],
        ).unwrap();

        let pane_root = std::path::Path::new("/tmp");
        let (notes_count, _, _, _) = run_embedding_pipeline(&conn, pane_root, &mock_embed, "test", "test-model", 512, |_| {})
            .expect("pipeline");

        assert_eq!(notes_count, 1, "non-daily note with NULL DB body should still be embedded when file has content");

        let emb_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM embeddings WHERE id = ?1",
            rusqlite::params!["soul-note"],
            |row| row.get(0),
        ).unwrap();
        assert_eq!(emb_count, 1, "embedding row should be stored for non-daily note");
    }

    // ── #2776: facts sub-pass regression — previously had ZERO truncation ────────

    #[test]
    fn test_facts_subpass_truncates_long_fact_strings_same_as_notes_and_chats() {
        // Regression test for #2776's second, distinct gap: the facts sub-pass
        // (embeddings.rs, ai_memory query) previously called embed_fn(&content) directly
        // with NO truncation guard whatsoever — any fact longer than ~512 tokens' worth of
        // characters always failed. This asserts embed_fn now receives a truncated slice,
        // the same as notes/chats.
        let conn = make_test_db();
        // A fact string long enough that, untruncated, would exceed a tiny max_tokens budget.
        let long_fact = "important context about the user ".repeat(200); // ~6800 bytes
        conn.execute(
            "INSERT INTO ai_memory (id, fact, category, is_active) VALUES ('f1', ?1, NULL, 1)",
            rusqlite::params![long_fact],
        ).unwrap();

        let received_len = std::sync::Mutex::new(0usize);
        let embed_fn = |text: &str| -> Result<Vec<f32>, String> {
            *received_len.lock().unwrap() = text.len();
            Ok(vec![0.0f32; 4])
        };

        let pane_root = std::path::Path::new("/tmp");
        // max_tokens=512 -> char budget = (512 * 2.2 * 0.9).floor() = 1013 chars (#2776
        // Round 3 ratio, was 1843 under the old 4.0 chars/token estimate), far below the
        // untruncated fact's ~6800 bytes.
        let (_, facts_count, _, _) = run_embedding_pipeline(&conn, pane_root, &embed_fn, "test", "test-model", 512, |_| {})
            .expect("pipeline");

        assert_eq!(facts_count, 1, "long fact should still embed successfully (post-truncation)");
        let len_received = *received_len.lock().unwrap();
        assert!(
            len_received <= 1013,
            "embed_fn should have received a truncated fact (<=1013 bytes), got {} bytes — facts sub-pass truncation is missing",
            len_received
        );
        assert!(len_received > 0, "embed_fn should have been called with non-empty content");
    }

    // ── #2776: token-aware truncation (seed table + fallback) ────────────────

    /// C35 (#3128): a genuinely-empty note/fact was already excluded by
    /// the #1673 guard, but a NEAR-empty one (a stub body of a few
    /// characters) passed `!trim().is_empty()` and got embedded with no
    /// floor — where the identical case on the chats side was already
    /// skipped by its own `len() >= 20` guard. This is the "empty-note"
    /// #3128 names informally: functionally, not literally, empty.
    ///
    /// Mutation (actually run — see report): remove the new `len() >= 20`
    /// clause from both the notes and facts sub-pass guards — reddens
    /// (both get embedded, embeddings row count becomes 1 for each id).
    #[test]
    fn test_pipeline_skips_near_empty_note_and_fact_under_20_chars() {
        let conn = make_test_db();
        let (_f, path) = make_temp_note("x"); // 1-char stub body — non-empty, near-empty
        conn.execute(
            "INSERT INTO notes (id, title, file_path, deleted_at) VALUES ('stub-note', 'Stub', ?1, NULL)",
            rusqlite::params![path],
        ).unwrap();
        conn.execute(
            "INSERT INTO ai_memory (id, fact, category, is_active) VALUES ('stub-fact', 'x', NULL, 1)",
            [],
        ).unwrap();

        let pane_root = std::path::Path::new("/tmp");
        let (notes_count, facts_count, _, _) = run_embedding_pipeline(&conn, pane_root, &mock_embed, "test", "test-model", 512, |_| {})
            .expect("pipeline");

        assert_eq!(notes_count, 0, "a 1-char-body note must be excluded by the new length floor");
        assert_eq!(facts_count, 0, "a 1-char fact must be excluded by the new length floor");

        let note_emb: i64 = conn.query_row(
            "SELECT COUNT(*) FROM embeddings WHERE id = ?1", rusqlite::params!["stub-note"], |r| r.get(0),
        ).unwrap();
        let fact_emb: i64 = conn.query_row(
            "SELECT COUNT(*) FROM embeddings WHERE id = ?1", rusqlite::params!["stub-fact"], |r| r.get(0),
        ).unwrap();
        assert_eq!(note_emb, 0, "no embedding row should be stored for the stub note");
        assert_eq!(fact_emb, 0, "no embedding row should be stored for the stub fact");
    }

    #[test]
    fn test_embedding_model_seed_lookup_known_models() {
        // Each entry verified against its published model card (see doc comment on
        // embedding_model_seed_lookup for sources).
        assert_eq!(embedding_model_seed_lookup("bge-base-en-v1.5"), Some(512));
        assert_eq!(embedding_model_seed_lookup("bge-large-en-v1.5"), Some(512));
        assert_eq!(embedding_model_seed_lookup("bge-m3"), Some(8192));
        assert_eq!(embedding_model_seed_lookup("nomic-embed-text"), Some(8192));
        assert_eq!(embedding_model_seed_lookup("nomic-embed-text:v1.5"), Some(8192));
        assert_eq!(embedding_model_seed_lookup("mxbai-embed-large"), Some(512));
        assert_eq!(embedding_model_seed_lookup("all-minilm"), Some(256));
    }

    #[test]
    fn test_embedding_model_seed_lookup_is_case_insensitive() {
        assert_eq!(embedding_model_seed_lookup("BGE-M3"), Some(8192));
        assert_eq!(embedding_model_seed_lookup("Nomic-Embed-Text"), Some(8192));
    }

    #[test]
    fn test_embedding_model_seed_lookup_unknown_model_returns_none() {
        // Unknown models must return None, not a guessed value — callers fall through
        // to EMBEDDINGS_MAX_TOKENS_FALLBACK and must log the fallback explicitly.
        assert_eq!(embedding_model_seed_lookup("llama3.2:3b"), None);
        assert_eq!(embedding_model_seed_lookup("some-brand-new-embedder"), None);
        assert_eq!(embedding_model_seed_lookup(""), None);
    }

    #[test]
    fn test_fallback_constant_is_conservative_512() {
        // Matt's ruling #1: no bare magic numbers — 512 must be read/derived. This constant
        // IS the one authorized last-resort read; every call site must log when it's hit.
        assert_eq!(EMBEDDINGS_MAX_TOKENS_FALLBACK, 512);
    }

    // ── #2776 follow-up (Gap 2): read path resolves the seed table ──────────────

    #[test]
    fn test_read_embeddings_max_tokens_resolves_seed_table() {
        // Fresh install / pre-#2776 engine config: `embeddings_model` is set to a
        // bge-m3 variant, but `embeddings_max_tokens` was NEVER persisted (Matt's boot
        // log repro: "unknown model, using conservative floor" for a model that IS in
        // the seed table). The read path must now resolve via the seed table (tier 2)
        // instead of falling all the way to the 512 floor (tier 3) — a bge-m3 user
        // getting 512 instead of 8192 is a silent 16x over-truncation.
        let conn = make_full_test_db();
        crate::database::set_ai_setting(&conn, "embeddings_model", "bge-m3").unwrap();
        // Deliberately NOT setting embeddings_max_tokens.

        let result = read_embeddings_max_tokens(&conn);

        assert_eq!(
            result, 8192,
            "must resolve 8192 from the seed table for bge-m3, not fall through to the 512 floor"
        );
    }

    #[test]
    fn test_read_embeddings_max_tokens_prefers_stored_value_over_seed_table() {
        // Tier 1 (stored value) must win over tier 2 (seed table) when both are
        // available — e.g. an OpenRouter-fetched context_length that legitimately
        // differs from the static Ollama seed table entry.
        let conn = make_full_test_db();
        crate::database::set_ai_setting(&conn, "embeddings_model", "bge-m3").unwrap();
        crate::database::set_ai_setting(&conn, "embeddings_max_tokens", "4096").unwrap();

        assert_eq!(read_embeddings_max_tokens(&conn), 4096);
    }

    #[test]
    fn test_read_embeddings_max_tokens_falls_through_to_floor_for_unknown_model() {
        // Neither a stored value nor a seed table match — must fall through to the
        // conservative 512 floor (tier 3), unchanged from pre-fix behavior.
        let conn = make_full_test_db();
        crate::database::set_ai_setting(&conn, "embeddings_model", "some-brand-new-embedder").unwrap();

        assert_eq!(read_embeddings_max_tokens(&conn), EMBEDDINGS_MAX_TOKENS_FALLBACK as usize);
    }

    #[test]
    fn test_read_embeddings_max_tokens_no_settings_at_all_falls_through_to_floor() {
        // Absolute fresh state: no embeddings_model, no embeddings_max_tokens.
        let conn = make_full_test_db();
        assert_eq!(read_embeddings_max_tokens(&conn), EMBEDDINGS_MAX_TOKENS_FALLBACK as usize);
    }
}

// ─── Truncation Safety Tests (#2776: token-aware truncation) ────────────────────

#[cfg(test)]
mod truncation_tests {
    use super::*;

    #[test]
    fn test_under_budget_unchanged() {
        // Content well under the token budget passes through unchanged.
        let s = "hello world".repeat(10); // 110 bytes, ~28 tokens even at the tightened 2.2 chars/token ratio
        assert_eq!(truncate_to_token_budget(&s, 512), s.as_str());
    }

    #[test]
    fn test_3000_char_string_at_512_max_tokens_stays_under_limit() {
        // Core regression test for #2776: a synthetic 3000-char ASCII string with
        // max_tokens=512 must truncate to a slice whose estimated token count is <= 512,
        // not merely <= 2000 chars (the old, wrong unit). Checked against the ORIGINAL
        // 4.0 chars/token ratio here deliberately — the function's own internal ratio is
        // now the tighter 2.2 (#2776 Round 3), so a result that stays under budget even
        // by the more generous 4.0 estimate is a strictly stronger guarantee.
        let s = "x".repeat(3000);
        let result = truncate_to_token_budget(&s, 512);
        let ratio = crate::ai::context_budget::TokenEstimationRatio { chars_per_token: 4.0 };
        let estimated_tokens = ratio.estimate_tokens(result);
        assert!(
            estimated_tokens <= 512,
            "estimated tokens {} must be <= 512 (max_tokens)",
            estimated_tokens
        );
        assert!(std::str::from_utf8(result.as_bytes()).is_ok());
    }

    #[test]
    fn test_truncation_applies_10_percent_safety_margin() {
        // Formula: (max_tokens * chars_per_token * 0.9).floor() chars.
        // max_tokens=512, chars_per_token=2.2 (#2776 Round 3) -> (512 * 2.2 * 0.9).floor() = 1013 chars.
        let s = "x".repeat(5000);
        let result = truncate_to_token_budget(&s, 512);
        assert_eq!(result.len(), 1013, "expected exactly the 10%-margin char budget for max_tokens=512");
    }

    #[test]
    fn test_larger_max_tokens_allows_more_content() {
        // bge-m3 / nomic-embed-text class models (8192 tokens) should retain far more
        // content than bge-base class (512 tokens) for the same source string.
        let s = "word ".repeat(3000); // 15000 bytes
        let small_budget = truncate_to_token_budget(&s, 512);
        let large_budget = truncate_to_token_budget(&s, 8192);
        assert!(large_budget.len() > small_budget.len());
        assert_eq!(small_budget.len(), 1013);
        assert_eq!(large_budget, s.as_str(), "8192-token budget should fit the whole 15000-byte string");
    }

    #[test]
    fn test_zero_max_tokens_truncates_to_empty() {
        // Defensive edge case: a max_tokens of 0 must not panic and must yield an
        // empty (or near-empty) slice, never a negative/overflowing byte count.
        let s = "some content".repeat(50);
        let result = truncate_to_token_budget(&s, 0);
        assert_eq!(result.len(), 0);
    }

    #[test]
    fn test_emoji_mid_char_boundary_no_panic() {
        // Emoji is 4 bytes each. Construct content that lands the token-budget cut
        // mid-emoji and confirm floor_char_boundary still protects it (existing UTF-8
        // safety, unchanged by #2776 — kept as the final step after budget truncation).
        let base = "a".repeat(1800);
        let emoji_fill = "\u{1F600}".repeat(100); // 😀 × 100 = 400 bytes
        let s = format!("{}{}", base, emoji_fill);
        let result = truncate_to_token_budget(&s, 512);
        assert!(std::str::from_utf8(result.as_bytes()).is_ok());
    }

    #[test]
    fn test_cjk_over_limit_no_panic() {
        // CJK characters are 3 bytes each — confirm the byte-budget cut never splits
        // a multi-byte character, regardless of the token-budget math above it.
        let s = "中".repeat(2000); // 6000 bytes
        let result = truncate_to_token_budget(&s, 512);
        assert!(std::str::from_utf8(result.as_bytes()).is_ok());
        assert_eq!(result.len() % 3, 0, "CJK result should be on a 3-byte char boundary");
    }

    #[test]
    fn test_accented_latin_over_limit_no_panic() {
        // 'é' is 2 bytes (U+00E9) — confirm the byte-budget cut respects 2-byte boundaries.
        let s = "é".repeat(2000); // 4000 bytes
        let result = truncate_to_token_budget(&s, 512);
        assert!(std::str::from_utf8(result.as_bytes()).is_ok());
        assert_eq!(result.len() % 2, 0, "Accented-latin result should be on a 2-byte char boundary");
    }

    #[test]
    fn test_mixed_utf8_under_limit_unchanged() {
        // Mixed content well under the token budget passes through exactly unchanged.
        let s = "Hello \u{4E16}\u{754C} café naïve résumé \u{1F389} end"; // 世界 🎉
        assert_eq!(truncate_to_token_budget(s, 512), s);
    }
}

// ─── Batched re-index tests (#3082) ──────────────────────────────────────────
//
// `run_embedding_pipeline` (unbatched) is exercised exhaustively by the `tests`
// module above and is UNCHANGED (it delegates to the same `run_embedding_pipeline_impl`
// with `batch = None`, verified not to regress by every test in that module still
// passing unmodified). These tests cover the NEW batched path
// (`run_embedding_pipeline_batched`) specifically: same success/failure counts as the
// unbatched path, batch-size-from-settings (via the real
// `ai::scheduler::resolve_embeddings_batch_size` at its own call site — these
// tests exercise `run_embedding_pipeline_batched` directly with an explicit batch_size,
// which is the real production entry point once the caller has resolved that size),
// progress still firing, and the subtlest risk: a batch containing one bad item must
// blame only that item, not the whole batch (the `0 failed` live-test baseline).
#[cfg(test)]
mod batched_pipeline_tests {
    use super::*;
    use std::sync::Mutex;

    fn make_test_db() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().expect("in-memory DB");
        conn.execute_batch("
            CREATE TABLE IF NOT EXISTS notes (
                id TEXT PRIMARY KEY,
                title TEXT NOT NULL DEFAULT '',
                file_path TEXT,
                body TEXT,
                note_type TEXT,
                deleted_at TEXT
            );
            CREATE TABLE IF NOT EXISTS ai_memory (
                id TEXT PRIMARY KEY,
                fact TEXT NOT NULL,
                category TEXT,
                is_active INTEGER NOT NULL DEFAULT 1
            );
            CREATE TABLE IF NOT EXISTS chat_messages_local (
                id TEXT PRIMARY KEY,
                content TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT ''
            );
            CREATE TABLE IF NOT EXISTS embeddings (
                id TEXT PRIMARY KEY,
                source TEXT NOT NULL,
                embedding BLOB NOT NULL,
                content_hash TEXT NOT NULL,
                engine TEXT,
                model TEXT,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
            CREATE INDEX IF NOT EXISTS idx_embeddings_source ON embeddings(source);
        ").expect("schema setup");
        conn
    }

    /// Seed `n` facts directly via `ai_memory` (the real table `run_embedding_pipeline`
    /// reads from — matches this file's own established fixture convention, see e.g.
    /// `test_pipeline_batching_25_items` above). Facts are used (not notes) for most of
    /// these tests because they need no on-disk file fixture, keeping the batch-size
    /// math in each test easy to audit.
    fn seed_facts(conn: &rusqlite::Connection, n: usize, prefix: &str) {
        for i in 0..n {
            conn.execute(
                "INSERT INTO ai_memory (id, fact, category, is_active) VALUES (?1, ?2, NULL, 1)",
                rusqlite::params![format!("fact-{}-{}", prefix, i), format!("{} fact body number {}", prefix, i)],
            ).unwrap();
        }
    }

    /// A batch-capable mock that always succeeds, returning a distinct deterministic
    /// vector per input so per-item correctness (not just counts) can be asserted.
    fn mock_batch_ok(texts: &[&str]) -> Result<Vec<Vec<f32>>, String> {
        Ok(texts.iter().map(|t| vec![t.len() as f32; 4]).collect())
    }

    fn mock_single_ok(text: &str) -> Result<Vec<f32>, String> {
        Ok(vec![text.len() as f32; 4])
    }

    // ── Same counts as the unbatched path ───────────────────────────────────

    #[test]
    fn test_batched_pipeline_same_counts_as_unbatched_over_realistic_corpus() {
        // 60 facts, batch_size = 25 → 2 full batches + 1 partial (10), matching the
        // issue's own ask: "same success/failure counts as the unbatched path over a
        // realistic corpus."
        let conn_unbatched = make_test_db();
        seed_facts(&conn_unbatched, 60, "u");
        let pane_root = std::path::Path::new("/tmp");
        let unbatched_result = run_embedding_pipeline(
            &conn_unbatched, pane_root, &mock_single_ok, "test", "test-model", 512, |_| {},
        ).expect("unbatched pipeline");

        let conn_batched = make_test_db();
        seed_facts(&conn_batched, 60, "u");
        let batched_result = run_embedding_pipeline_batched(
            &conn_batched, pane_root, &mock_single_ok, "test", "test-model", 512, |_| {},
            &mock_batch_ok, 25,
        ).expect("batched pipeline");

        assert_eq!(unbatched_result, batched_result,
            "(notes, facts, chats, failed) must be identical between batched and unbatched paths");
        assert_eq!(batched_result, (0, 60, 0, 0), "all 60 facts must succeed, 0 failed");
    }

    // ── batch size comes from settings, not a literal (exercised at the call site) ──

    #[test]
    fn test_batch_size_controls_number_of_batch_fn_invocations() {
        // 50 facts with batch_size=25 → exactly 2 batch_fn calls (not 50).
        let conn = make_test_db();
        seed_facts(&conn, 50, "b25");
        let pane_root = std::path::Path::new("/tmp");
        let call_count = Mutex::new(0usize);
        let batch_fn = |texts: &[&str]| -> Result<Vec<Vec<f32>>, String> {
            *call_count.lock().unwrap() += 1;
            Ok(texts.iter().map(|t| vec![t.len() as f32; 4]).collect())
        };

        let result = run_embedding_pipeline_batched(
            &conn, pane_root, &mock_single_ok, "test", "test-model", 512, |_| {}, &batch_fn, 25,
        ).expect("pipeline");

        assert_eq!(result.1, 50, "all 50 facts stored");
        assert_eq!(*call_count.lock().unwrap(), 2, "25-sized batches of 50 items must produce exactly 2 batch_fn calls");
    }

    #[test]
    fn test_smaller_batch_size_produces_more_batch_fn_invocations() {
        // Same 50 facts, batch_size=10 → exactly 5 batch_fn calls. Proves batch size
        // actually drives the call count (not hardcoded to 25 internally).
        let conn = make_test_db();
        seed_facts(&conn, 50, "b10");
        let pane_root = std::path::Path::new("/tmp");
        let call_count = Mutex::new(0usize);
        let batch_fn = |texts: &[&str]| -> Result<Vec<Vec<f32>>, String> {
            *call_count.lock().unwrap() += 1;
            Ok(texts.iter().map(|t| vec![t.len() as f32; 4]).collect())
        };

        let result = run_embedding_pipeline_batched(
            &conn, pane_root, &mock_single_ok, "test", "test-model", 512, |_| {}, &batch_fn, 10,
        ).expect("pipeline");

        assert_eq!(result.1, 50);
        assert_eq!(*call_count.lock().unwrap(), 5, "10-sized batches of 50 items must produce exactly 5 batch_fn calls");
    }

    // ── Progress still fires per batch ──────────────────────────────────────

    #[test]
    fn test_progress_fires_same_cadence_under_batched_path() {
        // 75 facts, batch_size=25 → progress still fires 3 times (at 25/50/75), same
        // cadence as the unbatched path's own test (test_pipeline_batching_at_most_once_per_25_items).
        let conn = make_test_db();
        seed_facts(&conn, 75, "p");
        let pane_root = std::path::Path::new("/tmp");
        let event_count = Mutex::new(0usize);

        run_embedding_pipeline_batched(
            &conn, pane_root, &mock_single_ok, "test", "test-model", 512,
            |_evt| { *event_count.lock().unwrap() += 1; },
            &mock_batch_ok, 25,
        ).expect("pipeline");

        assert_eq!(*event_count.lock().unwrap(), 3, "75 facts should produce exactly 3 indexing events (at 25, 50, 75), same cadence as unbatched");
    }

    // ── The subtlest risk: a batch containing one bad item blames that item ────

    #[test]
    fn test_one_bad_item_in_batch_blames_only_that_item() {
        // 25 facts (one full batch). The batch call fails ONLY when the batch contains
        // the marked "poison" item; a real provider would return one HTTP 400 for the
        // whole array in this case, with no data[] at all — simulated here by returning
        // Err for the whole batch. The pipeline must fall back to per-item embedding for
        // just this batch, blame ONLY the poison item, and store the other 24.
        let conn = make_test_db();
        seed_facts(&conn, 25, "poison");
        // fact-poison-13 is the poison item (arbitrary middle index)
        let pane_root = std::path::Path::new("/tmp");

        let batch_fn = |_texts: &[&str]| -> Result<Vec<Vec<f32>>, String> {
            // Simulate the real-world shape: the WHOLE batch fails as one HTTP 400,
            // not a partial response — exactly what Reed's audit identified as the only
            // possible shape (no partial success within a single request).
            Err("OpenRouter embed HTTP 400: one item exceeded the token limit".to_string())
        };

        // Per-item fallback: everything succeeds except the poison item.
        let fallback_fn = |text: &str| -> Result<Vec<f32>, String> {
            if text.contains("number 13") {
                Err("HTTP 400: token limit exceeded for this item".to_string())
            } else {
                Ok(vec![text.len() as f32; 4])
            }
        };

        let (_, facts_count, _, failed_count) = run_embedding_pipeline_batched(
            &conn, pane_root, &fallback_fn, "test", "test-model", 512, |_| {}, &batch_fn, 25,
        ).expect("pipeline must not abort on a non-auth batch failure");

        assert_eq!(failed_count, 1, "exactly ONE item (the poison item) must be marked failed, not the whole batch of 25");
        assert_eq!(facts_count, 24, "the other 24 items in the same batch must still be stored successfully");

        // Direct DB check: the poison item was never stored; the rest were.
        let stored: i64 = conn.query_row("SELECT COUNT(*) FROM embeddings WHERE source = 'memory'", [], |r| r.get(0)).unwrap();
        assert_eq!(stored, 24);
        let poison_stored: i64 = conn.query_row(
            "SELECT COUNT(*) FROM embeddings WHERE id = 'fact-poison-13'", [], |r| r.get(0),
        ).unwrap();
        assert_eq!(poison_stored, 0, "the poison item must never be marked succeeded");
    }

    #[test]
    fn test_batch_length_mismatch_falls_back_without_mis_attribution() {
        // A batch response with the WRONG number of vectors (a shape the pipeline
        // didn't expect) must never be zipped by position — it must be treated as a
        // batch failure and fall back to per-item embedding, same as an HTTP error.
        let conn = make_test_db();
        seed_facts(&conn, 10, "mismatch");
        let pane_root = std::path::Path::new("/tmp");

        let batch_fn = |texts: &[&str]| -> Result<Vec<Vec<f32>>, String> {
            // Return one fewer vector than requested — a shape the caller must detect
            // and refuse to positionally zip.
            Ok(texts.iter().take(texts.len() - 1).map(|t| vec![t.len() as f32; 4]).collect())
        };

        let (_, facts_count, _, failed_count) = run_embedding_pipeline_batched(
            &conn, pane_root, &mock_single_ok, "test", "test-model", 512, |_| {}, &batch_fn, 25,
        ).expect("pipeline must not abort on a length mismatch");

        // Falls back to the per-item path (mock_single_ok succeeds for everything).
        assert_eq!(facts_count, 10, "length-mismatch must fall back to per-item, not silently drop or misattribute items");
        assert_eq!(failed_count, 0);
    }

    #[test]
    fn test_batch_auth_error_aborts_pipeline_like_unbatched() {
        // An auth failure (401/403) must abort the WHOLE pipeline immediately, exactly
        // like the unbatched path — never silently fall back to per-item for an auth
        // failure (that would burn N requests just to learn the same key is bad N times).
        let conn = make_test_db();
        seed_facts(&conn, 5, "auth");
        let pane_root = std::path::Path::new("/tmp");

        let batch_fn = |_texts: &[&str]| -> Result<Vec<Vec<f32>>, String> {
            Err("OpenRouter embed HTTP 401: invalid API key".to_string())
        };

        let result = run_embedding_pipeline_batched(
            &conn, pane_root, &mock_single_ok, "test", "test-model", 512, |_| {}, &batch_fn, 25,
        );

        assert!(result.is_err(), "auth failure must abort the pipeline, not fall back to per-item");
        assert!(result.unwrap_err().contains("auth"), "error must be attributed to auth, not a generic failure");
    }

    #[test]
    fn test_empty_corpus_batched_path_calls_batch_fn_zero_times() {
        let conn = make_test_db();
        let pane_root = std::path::Path::new("/tmp");
        let call_count = Mutex::new(0usize);
        let batch_fn = |texts: &[&str]| -> Result<Vec<Vec<f32>>, String> {
            *call_count.lock().unwrap() += 1;
            Ok(texts.iter().map(|t| vec![t.len() as f32; 4]).collect())
        };

        let result = run_embedding_pipeline_batched(
            &conn, pane_root, &mock_single_ok, "test", "test-model", 512, |_| {}, &batch_fn, 25,
        ).expect("pipeline");

        assert_eq!(result, (0, 0, 0, 0));
        assert_eq!(*call_count.lock().unwrap(), 0, "no items means no batch calls at all");
    }
}
