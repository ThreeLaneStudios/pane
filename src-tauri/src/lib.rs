// Pane v10.9.0 — Main Library
// Architecture per pane-architecture-spec.md

mod agents;
mod ai;
mod archive;
mod attachments;
mod backup;
mod conflict_resolution;
mod context_pins;
mod dashboard;
mod database;
mod embeddings;
mod file_ops;
mod filesystem_policy;
// #2339 PR 6/7 — formula parser/evaluator (PR 6) + dependency graph/
// recompute engine (PR 7, `formula::recompute`). Wired into `sheets.rs`
// (parse-on-write validation, live recompute calls) and `sync/engine.rs`
// (post-sync recompute hook) as of PR 7 — no longer dead code, so the
// PR 6-era `#[allow(dead_code)]` is removed.
mod formula;
mod gateway_client;
mod import;
mod install_uuid;
mod integrity;
mod keychain;
mod logs;
#[cfg(test)]
mod migration_tests;
mod migrations;
mod notes;
mod notifications;
mod og_fetch; // #1780: og-enrichment for link previews (ADR-0049)
#[cfg(test)]
mod onboarding_restore_tests;
mod projects;
// #2972 stage 1 (tracked on #2874, docs/spec/data-refresh.md §34): the one
// data-change event's shared name/entity-type/reason constants and the
// backend `emit()` helper. Used by notes.rs, projects.rs, ai/chat.rs,
// ai/ingest.rs, ai/scheduler.rs, ai/tools.rs (#3097 split of ai/commands.rs).
mod refresh_event;
mod sheets;
mod sheets_snapshots;
// #2723 PR B: window-first startup orchestration (StartupStatus state,
// get_startup_status command, the mount_db_state_then ordering primitive).
mod startup;
mod subtasks;
mod sync;
mod sync_client;
mod system_files;
mod tags;
mod templates;
#[cfg(test)]
mod test_p2p_sync; // pre-existing harness file, was never wired into the module tree until #2339 PR 5a needed it for the sheet_cells LWW test
#[cfg(test)]
mod tests;
mod versions;
mod watcher;

use ai::provider::AiProvider;
use database::DbState;
use std::sync::{Arc, Mutex};
use sync::auto_sync::AutoSync;
use sync::commands::{AutoSyncState, SyncManagerState};
use sync::manager::SyncManager;
use tauri::Manager;

// ─── Keychain Commands ───────────────────────────────────────────────────

#[tauri::command]
fn store_supabase_password(email: String, password: String) -> Result<(), String> {
    keychain::store_password(&email, &password)
}

#[tauri::command]
fn get_supabase_password(email: String) -> Result<Option<String>, String> {
    keychain::get_password(&email)
}

#[tauri::command]
fn clear_supabase_password(email: String) -> Result<(), String> {
    keychain::delete_password(&email)
}

// ─── Backup Commands (§25) ────────────────────────────────────────────────

#[tauri::command]
async fn create_backup(app: tauri::AppHandle) -> Result<String, String> {
    eprintln!("[create_backup] called");
    let handle = app.clone();
    tokio::task::spawn_blocking(move || {
        let db_path = database::get_db_path();
        let backup_path = backup::create_manual_backup(&db_path, Some(&handle))?;
        Ok(backup_path.to_string_lossy().to_string())
    })
    .await
    .map_err(|e| {
        eprintln!("[create_backup] thread panicked: {}", e);
        // Surface the JoinError (thread panic) as a backup_error event so the
        // frontend knows the backup did not complete.
        use tauri::Emitter;
        let _ = app.emit(
            "backup://progress",
            serde_json::json!({
                "phase": "backup_error",
                "message": format!("Backup thread panicked: {}", e),
                "pct": null
            }),
        );
        format!("Backup thread panicked: {}", e)
    })?
}

#[tauri::command]
fn list_backups() -> Result<Vec<backup::BackupInfo>, String> {
    backup::list_backups()
}

#[tauri::command]
fn restore_backup(
    backup_path: String,
    db: tauri::State<DbState>,
) -> Result<backup::RestoreSummary, String> {
    eprintln!("[restore_backup] called with: {}", backup_path);
    let db_path = database::get_db_path();
    let path = std::path::PathBuf::from(backup_path);
    backup::restore_backup(&path, &db_path, &db).map_err(|e| e.to_string())
}

// ─── HTTP Embedding Backend (ADR-0048 / §5.9) ───────────────────────────────

// Test-only override for the venice arm's hardcoded `https://api.venice.ai/api/v1`
// base (#2988 PR1) — same `#[cfg(test)]` thread_local carve-out shape as
// `ai::connections::test_set_venice_validate_base` (ADR-0065 D8 live-key probe) and
// `web::test_set_brave_api_url`. Lets the Venice embed arm be exercised against a
// mockito server without touching the real Venice API.
#[cfg(test)]
thread_local! {
    static TEST_VENICE_EMBED_BASE: std::cell::RefCell<Option<String>> = std::cell::RefCell::new(None);
}

#[cfg(test)]
pub(crate) fn test_set_venice_embed_base(url: &str) {
    TEST_VENICE_EMBED_BASE.with(|c| *c.borrow_mut() = Some(url.to_string()));
}

#[cfg(test)]
pub(crate) fn test_clear_venice_embed_base() {
    TEST_VENICE_EMBED_BASE.with(|c| *c.borrow_mut() = None);
}

fn venice_embed_base() -> String {
    #[cfg(test)]
    {
        if let Some(u) = TEST_VENICE_EMBED_BASE.with(|c| c.borrow().clone()) {
            return u;
        }
    }
    "https://api.venice.ai/api/v1".to_string()
}

/// Build an HTTP `embed_fn` closure from the configured engine and model.
///
/// Returns `(embed_fn, engine_tag)` where `engine_tag` is the string used to
/// tag rows in the `embeddings.engine` column (e.g. `"openrouter"`, `"ollama"`,
/// `"venice"`).
///
/// This function is **engine-agnostic at the call site** — callers in lib.rs and
/// commands.rs call this helper; `embeddings.rs` never touches HTTP directly.
pub(crate) fn build_http_embed_fn(
    conn: &rusqlite::Connection,
    engine: &str,
    model: &str,
) -> Result<
    (
        Box<dyn Fn(&str) -> Result<Vec<f32>, String> + Send + Sync + 'static>,
        String,
    ),
    String,
> {
    let model = model.to_string();
    let engine = engine.to_string();

    match engine.as_str() {
        "openrouter" => {
            // C50 (#3171 PR 1): read the OpenRouter key from Provider Connections'
            // own keychain slot (`read_provider_key`, the same function image
            // generation's §39.2 arm already uses) — NOT the retired
            // `ai_settings.embeddings_api_key` plaintext slot, which this
            // function used to read before C50. That slot was a credential-
            // handling defect (an API key sitting in the database in the
            // clear, never in the keychain) ADR-0044 fixed everywhere else but
            // left standing here; `credential_migration::run_feature_key_consolidation`
            // backfills any pre-existing value into the connection once, then
            // clears the plaintext row for good — see that function's doc
            // comment. No empty-key fallback: a missing connection key errors
            // here, before any network call, same as every other arm's missing-
            // credential guard.
            let api_key = crate::ai::credential_migration::read_provider_key(conn, "openrouter")
                .ok_or_else(|| {
                    "OpenRouter is not connected — add it in Settings → AI → Provider Connections"
                        .to_string()
                })?;
            if api_key.is_empty() {
                return Err(
                    "OpenRouter is not connected — add it in Settings → AI → Provider Connections"
                        .to_string(),
                );
            }
            let fn_box: Box<dyn Fn(&str) -> Result<Vec<f32>, String> + Send + Sync + 'static> =
                Box::new(move |text: &str| {
                    let client = reqwest::blocking::Client::builder()
                        .timeout(std::time::Duration::from_secs(30))
                        .build()
                        .map_err(|e| format!("HTTP client build failed: {}", e))?;
                    let body = serde_json::json!({
                        "model": model,
                        "input": text
                    });
                    // #2776 Round 3: wire-level log — reports what is ACTUALLY about to go
                    // over the wire (serialized body byte length) alongside the closure
                    // parameter's own char count, so a source-vs-wire divergence (lifetime,
                    // borrow, or serialization bug) shows up in production logs even when
                    // the source code around this call site looks correct.
                    let wire_body_bytes = serde_json::to_vec(&body).map(|b| b.len()).unwrap_or(0);
                    eprintln!(
                        "[EMBEDDINGS] Wire: sending {} body bytes, input text {} chars",
                        wire_body_bytes,
                        text.chars().count()
                    );
                    let resp = client
                        .post("https://openrouter.ai/api/v1/embeddings")
                        .header("Authorization", format!("Bearer {}", api_key))
                        .header("Content-Type", "application/json")
                        .json(&body)
                        .send()
                        .map_err(|e| format!("OpenRouter embed request failed: {}", e))?;
                    if !resp.status().is_success() {
                        let status = resp.status();
                        let body_err = resp.text().unwrap_or_default();
                        return Err(format!("OpenRouter embed HTTP {}: {}", status, body_err));
                    }
                    let json: serde_json::Value = resp
                        .json()
                        .map_err(|e| format!("OpenRouter embed JSON parse failed: {}", e))?;
                    let emb = json["data"][0]["embedding"]
                        .as_array()
                        .ok_or_else(|| "OpenRouter embed: missing data[0].embedding".to_string())?;
                    let vec: Vec<f32> = emb
                        .iter()
                        .map(|v| v.as_f64().unwrap_or(0.0) as f32)
                        .collect();
                    if vec.is_empty() {
                        return Err("OpenRouter embed returned empty vector".to_string());
                    }
                    Ok(vec)
                });
            Ok((fn_box, "openrouter".to_string()))
        }
        "ollama" => {
            // Read Ollama endpoint from the Semantic Search card's own saved value (§5.9.2 / #1657).
            let endpoint = database::get_ai_setting(conn, "embeddings_endpoint")
                .unwrap_or_else(|| "http://127.0.0.1:11434".to_string());
            if endpoint.is_empty() {
                return Err("Ollama endpoint not configured".to_string());
            }
            let fn_box: Box<dyn Fn(&str) -> Result<Vec<f32>, String> + Send + Sync + 'static> =
                Box::new(move |text: &str| {
                    let client = reqwest::blocking::Client::builder()
                        .timeout(std::time::Duration::from_secs(60))
                        .build()
                        .map_err(|e| format!("HTTP client build failed: {}", e))?;
                    let body = serde_json::json!({
                        "model": model,
                        "input": text
                    });
                    // #2776 Round 3: wire-level log — see matching comment in the OpenRouter
                    // arm above. Same purpose: catch a wire-vs-source divergence in the
                    // production Ollama path too.
                    let wire_body_bytes = serde_json::to_vec(&body).map(|b| b.len()).unwrap_or(0);
                    eprintln!(
                        "[EMBEDDINGS] Wire: sending {} body bytes, input text {} chars",
                        wire_body_bytes,
                        text.chars().count()
                    );
                    let url = format!("{}/api/embed", endpoint.trim_end_matches('/'));
                    let resp = client
                        .post(&url)
                        .header("Content-Type", "application/json")
                        .json(&body)
                        .send()
                        .map_err(|e| format!("Ollama embed request failed: {}", e))?;
                    if !resp.status().is_success() {
                        let status = resp.status();
                        let body_text = resp.text().unwrap_or_default();
                        return Err(format!("Ollama embed HTTP {}: {}", status, body_text));
                    }
                    let json: serde_json::Value = resp
                        .json()
                        .map_err(|e| format!("Ollama embed JSON parse failed: {}", e))?;
                    let emb = json["embeddings"][0]
                        .as_array()
                        .ok_or_else(|| "Ollama embed: missing embeddings[0]".to_string())?;
                    let vec: Vec<f32> = emb
                        .iter()
                        .map(|v| v.as_f64().unwrap_or(0.0) as f32)
                        .collect();
                    if vec.is_empty() {
                        return Err("Ollama embed returned empty vector".to_string());
                    }
                    Ok(vec)
                });
            Ok((fn_box, "ollama".to_string()))
        }
        "venice" => {
            // #2988 §1e: DELIBERATE DEVIATION — reads the chat-provider Venice key
            // (`credential_migration::read_venice_key`, keychain-only, ADR-0065 D4)
            // directly, NOT the Semantic Search card's own `embeddings_api_key` slot
            // used by the OpenRouter/Ollama arms above. Venice's `/embeddings`
            // endpoint shares the same single Bearer key as `/chat/completions` and
            // `/models` (confirmed against Venice's docs — no separate embeddings
            // scope), and Matt's instruction was to reuse the EXISTING Venice key so
            // the user never re-enters a key just for Semantic Search. This is
            // intentional asymmetry, not a bug — see the PR description for the full
            // rationale before "fixing" it to match the other two arms.
            let api_key = crate::ai::credential_migration::read_venice_key(conn).ok_or_else(|| {
                "Venice API key not configured (set it in Settings → AI → Primary Provider or Internal Provider)"
                    .to_string()
            })?;
            let fn_box: Box<dyn Fn(&str) -> Result<Vec<f32>, String> + Send + Sync + 'static> =
                Box::new(move |text: &str| {
                    let client = reqwest::blocking::Client::builder()
                        .timeout(std::time::Duration::from_secs(30))
                        .build()
                        .map_err(|e| format!("HTTP client build failed: {}", e))?;
                    let body = serde_json::json!({
                        "model": model,
                        "input": text
                    });
                    let wire_body_bytes = serde_json::to_vec(&body).map(|b| b.len()).unwrap_or(0);
                    eprintln!(
                        "[EMBEDDINGS] Wire: sending {} body bytes, input text {} chars",
                        wire_body_bytes,
                        text.chars().count()
                    );
                    let url = format!("{}/embeddings", venice_embed_base());
                    let resp = client
                        .post(&url)
                        .header("Authorization", format!("Bearer {}", api_key))
                        .header("Content-Type", "application/json")
                        .json(&body)
                        .send()
                        .map_err(|e| format!("Venice embed request failed: {}", e))?;
                    if !resp.status().is_success() {
                        let status = resp.status();
                        let body_err = resp.text().unwrap_or_default();
                        return Err(format!("Venice embed HTTP {}: {}", status, body_err));
                    }
                    let json: serde_json::Value = resp
                        .json()
                        .map_err(|e| format!("Venice embed JSON parse failed: {}", e))?;
                    let emb = json["data"][0]["embedding"]
                        .as_array()
                        .ok_or_else(|| "Venice embed: missing data[0].embedding".to_string())?;
                    let vec: Vec<f32> = emb
                        .iter()
                        .map(|v| v.as_f64().unwrap_or(0.0) as f32)
                        .collect();
                    if vec.is_empty() {
                        return Err("Venice embed returned empty vector".to_string());
                    }
                    Ok(vec)
                });
            Ok((fn_box, "venice".to_string()))
        }
        other => Err(format!(
            "Unknown embeddings engine: '{}' (expected 'openrouter', 'ollama', or 'venice')",
            other
        )),
    }
}

/// Batch-capable variant of `build_http_embed_fn` (#3082) — sends up to N texts in one
/// `/embeddings` call instead of one call per item. Used ONLY by the re-index pipeline
/// path (`switch_embedding_engine` → `run_embedding_pipeline_batched`); query-time
/// semantic search (`semantic_search`, always a single query string) and the two
/// already-correct callers of `run_embedding_pipeline` (`run_integrity_check`, the
/// startup pipeline) keep using the unbatched `build_http_embed_fn` above, untouched.
///
/// Mirrors `build_http_embed_fn`'s three engine arms exactly (same credential sources,
/// same endpoints, same error-message shapes) — only the wire shape changes: `input` is
/// an array, not a single string, and the response's `data[]`/`embeddings[]` array is
/// returned in full (validated by length against the request by the caller —
/// `embeddings.rs::embed_and_store_chunk` — not here; this function's job is just to
/// speak the batch wire protocol for each engine).
pub(crate) fn build_http_embed_batch_fn(
    conn: &rusqlite::Connection,
    engine: &str,
    model: &str,
) -> Result<
    (
        Box<dyn Fn(&[&str]) -> Result<Vec<Vec<f32>>, String> + Send + Sync + 'static>,
        String,
    ),
    String,
> {
    let model = model.to_string();
    let engine = engine.to_string();

    match engine.as_str() {
        "openrouter" => {
            // C50 (#3171 PR 1): same credential source as `build_http_embed_fn`'s
            // openrouter arm above — see its doc comment for the full rationale.
            let api_key = crate::ai::credential_migration::read_provider_key(conn, "openrouter")
                .ok_or_else(|| {
                    "OpenRouter is not connected — add it in Settings → AI → Provider Connections"
                        .to_string()
                })?;
            if api_key.is_empty() {
                return Err(
                    "OpenRouter is not connected — add it in Settings → AI → Provider Connections"
                        .to_string(),
                );
            }
            let fn_box: Box<dyn Fn(&[&str]) -> Result<Vec<Vec<f32>>, String> + Send + Sync + 'static> =
                Box::new(move |texts: &[&str]| {
                    let client = reqwest::blocking::Client::builder()
                        .timeout(std::time::Duration::from_secs(30))
                        .build()
                        .map_err(|e| format!("HTTP client build failed: {}", e))?;
                    let body = serde_json::json!({
                        "model": model,
                        "input": texts
                    });
                    let wire_body_bytes = serde_json::to_vec(&body).map(|b| b.len()).unwrap_or(0);
                    eprintln!(
                        "[EMBEDDINGS] Batch wire: sending {} body bytes, {} input item(s)",
                        wire_body_bytes,
                        texts.len()
                    );
                    let resp = client
                        .post("https://openrouter.ai/api/v1/embeddings")
                        .header("Authorization", format!("Bearer {}", api_key))
                        .header("Content-Type", "application/json")
                        .json(&body)
                        .send()
                        .map_err(|e| format!("OpenRouter embed request failed: {}", e))?;
                    if !resp.status().is_success() {
                        let status = resp.status();
                        let body_err = resp.text().unwrap_or_default();
                        return Err(format!("OpenRouter embed HTTP {}: {}", status, body_err));
                    }
                    let json: serde_json::Value = resp
                        .json()
                        .map_err(|e| format!("OpenRouter embed JSON parse failed: {}", e))?;
                    let data = json["data"]
                        .as_array()
                        .ok_or_else(|| "OpenRouter embed: missing data[]".to_string())?;
                    let vecs: Vec<Vec<f32>> = data
                        .iter()
                        .map(|item| {
                            item["embedding"]
                                .as_array()
                                .map(|emb| emb.iter().map(|v| v.as_f64().unwrap_or(0.0) as f32).collect())
                                .unwrap_or_default()
                        })
                        .collect();
                    if vecs.iter().any(|v| v.is_empty()) {
                        return Err("OpenRouter batch embed returned an empty vector for at least one item".to_string());
                    }
                    Ok(vecs)
                });
            Ok((fn_box, "openrouter".to_string()))
        }
        "ollama" => {
            let endpoint = database::get_ai_setting(conn, "embeddings_endpoint")
                .unwrap_or_else(|| "http://127.0.0.1:11434".to_string());
            if endpoint.is_empty() {
                return Err("Ollama endpoint not configured".to_string());
            }
            let fn_box: Box<dyn Fn(&[&str]) -> Result<Vec<Vec<f32>>, String> + Send + Sync + 'static> =
                Box::new(move |texts: &[&str]| {
                    let client = reqwest::blocking::Client::builder()
                        .timeout(std::time::Duration::from_secs(60))
                        .build()
                        .map_err(|e| format!("HTTP client build failed: {}", e))?;
                    let body = serde_json::json!({
                        "model": model,
                        "input": texts
                    });
                    let wire_body_bytes = serde_json::to_vec(&body).map(|b| b.len()).unwrap_or(0);
                    eprintln!(
                        "[EMBEDDINGS] Batch wire: sending {} body bytes, {} input item(s)",
                        wire_body_bytes,
                        texts.len()
                    );
                    let url = format!("{}/api/embed", endpoint.trim_end_matches('/'));
                    let resp = client
                        .post(&url)
                        .header("Content-Type", "application/json")
                        .json(&body)
                        .send()
                        .map_err(|e| format!("Ollama embed request failed: {}", e))?;
                    if !resp.status().is_success() {
                        let status = resp.status();
                        let body_text = resp.text().unwrap_or_default();
                        return Err(format!("Ollama embed HTTP {}: {}", status, body_text));
                    }
                    let json: serde_json::Value = resp
                        .json()
                        .map_err(|e| format!("Ollama embed JSON parse failed: {}", e))?;
                    let data = json["embeddings"]
                        .as_array()
                        .ok_or_else(|| "Ollama embed: missing embeddings[]".to_string())?;
                    let vecs: Vec<Vec<f32>> = data
                        .iter()
                        .map(|emb| {
                            emb.as_array()
                                .map(|a| a.iter().map(|v| v.as_f64().unwrap_or(0.0) as f32).collect())
                                .unwrap_or_default()
                        })
                        .collect();
                    if vecs.iter().any(|v| v.is_empty()) {
                        return Err("Ollama batch embed returned an empty vector for at least one item".to_string());
                    }
                    Ok(vecs)
                });
            Ok((fn_box, "ollama".to_string()))
        }
        "venice" => {
            let api_key = crate::ai::credential_migration::read_venice_key(conn).ok_or_else(|| {
                "Venice API key not configured (set it in Settings → AI → Primary Provider or Internal Provider)"
                    .to_string()
            })?;
            let fn_box: Box<dyn Fn(&[&str]) -> Result<Vec<Vec<f32>>, String> + Send + Sync + 'static> =
                Box::new(move |texts: &[&str]| {
                    let client = reqwest::blocking::Client::builder()
                        .timeout(std::time::Duration::from_secs(30))
                        .build()
                        .map_err(|e| format!("HTTP client build failed: {}", e))?;
                    let body = serde_json::json!({
                        "model": model,
                        "input": texts
                    });
                    let wire_body_bytes = serde_json::to_vec(&body).map(|b| b.len()).unwrap_or(0);
                    eprintln!(
                        "[EMBEDDINGS] Batch wire: sending {} body bytes, {} input item(s)",
                        wire_body_bytes,
                        texts.len()
                    );
                    let url = format!("{}/embeddings", venice_embed_base());
                    let resp = client
                        .post(&url)
                        .header("Authorization", format!("Bearer {}", api_key))
                        .header("Content-Type", "application/json")
                        .json(&body)
                        .send()
                        .map_err(|e| format!("Venice embed request failed: {}", e))?;
                    if !resp.status().is_success() {
                        let status = resp.status();
                        let body_err = resp.text().unwrap_or_default();
                        return Err(format!("Venice embed HTTP {}: {}", status, body_err));
                    }
                    let json: serde_json::Value = resp
                        .json()
                        .map_err(|e| format!("Venice embed JSON parse failed: {}", e))?;
                    let data = json["data"]
                        .as_array()
                        .ok_or_else(|| "Venice embed: missing data[]".to_string())?;
                    let vecs: Vec<Vec<f32>> = data
                        .iter()
                        .map(|item| {
                            item["embedding"]
                                .as_array()
                                .map(|emb| emb.iter().map(|v| v.as_f64().unwrap_or(0.0) as f32).collect())
                                .unwrap_or_default()
                        })
                        .collect();
                    if vecs.iter().any(|v| v.is_empty()) {
                        return Err("Venice batch embed returned an empty vector for at least one item".to_string());
                    }
                    Ok(vecs)
                });
            Ok((fn_box, "venice".to_string()))
        }
        other => Err(format!(
            "Unknown embeddings engine: '{}' (expected 'openrouter', 'ollama', or 'venice')",
            other
        )),
    }
}

// ─── Onboarding Backup Restore (§6.1.4) ───────────────────────────────────

#[tauri::command]
fn check_fresh_install() -> Result<bool, String> {
    eprintln!("[check_fresh_install] called");
    let db_path = database::get_db_path();

    // If DB doesn't exist, definitely fresh
    if !db_path.exists() {
        eprintln!("[check_fresh_install] DB doesn't exist → fresh install");
        return Ok(true);
    }

    // If DB exists but empty (no notes, no projects, no chat), still fresh
    let conn = rusqlite::Connection::open(&db_path)
        .map_err(|e| format!("Failed to open database: {}", e))?;

    let note_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM notes", [], |r| r.get(0))
        .unwrap_or(0);
    let project_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM projects", [], |r| r.get(0))
        .unwrap_or(0);
    let chat_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM chat_messages_local", [], |r| r.get(0))
        .unwrap_or(0);

    let is_fresh = note_count == 0 && project_count == 0 && chat_count == 0;
    eprintln!(
        "[check_fresh_install] notes={}, projects={}, chat={} → fresh={}",
        note_count, project_count, chat_count, is_fresh
    );

    Ok(is_fresh)
}

#[tauri::command]
fn restore_backup_from_file(
    backup_path: String,
    db: tauri::State<DbState>,
) -> Result<backup::RestoreSummary, String> {
    eprintln!("[restore_backup_from_file] called with: {}", backup_path);
    let path = std::path::PathBuf::from(&backup_path);
    let db_path = database::get_db_path();

    backup::restore_backup(&path, &db_path, &db).map_err(|e| {
        // Convert RestoreError to a serializable error string
        // Include the error variant name so frontend can distinguish
        match e {
            backup::RestoreError::SchemaTooNew { backup, current } => {
                format!("SCHEMA_TOO_NEW: Backup schema {} is newer than current {}. Please update Pane first.", backup, current)
            }
            backup::RestoreError::SchemaOlderConfirmationRequired { backup, current } => {
                format!("SCHEMA_OLDER: Backup schema {} is older than current {}. Restore anyway?", backup, current)
            }
            backup::RestoreError::UserCancelled => {
                format!("USER_CANCELLED: Restore cancelled")
            }
            backup::RestoreError::DbRestoreFailed(msg) => {
                format!("DB_FAILED: {}", msg)
            }
            backup::RestoreError::InstallUuidRestoreFailed(msg) => {
                format!("UUID_FAILED: {}", msg)
            }
        }
    })
}

#[tauri::command]
fn restore_backup_confirmed_cmd(
    backup_path: String,
    db: tauri::State<DbState>,
) -> Result<backup::RestoreSummary, String> {
    eprintln!(
        "[restore_backup_confirmed_cmd] called with: {}",
        backup_path
    );
    let path = std::path::PathBuf::from(&backup_path);
    let db_path = database::get_db_path();

    backup::restore_backup_confirmed(&path, &db_path, &db).map_err(|e| {
        match e {
            backup::RestoreError::SchemaTooNew { backup, current } => {
                format!("SCHEMA_TOO_NEW: Backup schema {} is newer than current {}. Please update Pane first.", backup, current)
            }
            backup::RestoreError::SchemaOlderConfirmationRequired { backup, current } => {
                format!("SCHEMA_OLDER: Backup schema {} is older than current {}. This should not happen after confirmation.", backup, current)
            }
            backup::RestoreError::UserCancelled => {
                format!("USER_CANCELLED: Restore cancelled")
            }
            backup::RestoreError::DbRestoreFailed(msg) => {
                format!("DB_FAILED: {}", msg)
            }
            backup::RestoreError::InstallUuidRestoreFailed(msg) => {
                format!("UUID_FAILED: {}", msg)
            }
        }
    })
}

// ─── Health Dashboard Commands (§27.13) ───────────────────────────────────

#[derive(Debug, Clone, serde::Serialize)]
struct ExtractionStats {
    status: String, // "healthy" | "behind" | "degraded"
    queue_depth: usize,
    last_processed: Option<String>,
    facts_today: usize,
    failed_count: usize,
    total_processed: usize,
    total_facts: usize,
    failure_rate: f64,
}

#[derive(Debug, Clone, serde::Serialize)]
struct CompactionStats {
    thread_name: String,
    messages_in_conversation: usize,
    messages_in_context: usize,
    compaction_summary_count: usize,
    last_compaction: Option<String>,
}

#[tauri::command]
fn get_context_budget_stats(
    budget_state: tauri::State<ai::context_budget::BudgetState>,
) -> Result<Option<ai::context_budget::BudgetResult>, String> {
    Ok(ai::context_budget::get_latest_budget_stats(&budget_state))
}

#[tauri::command]
fn get_extraction_stats(db: tauri::State<DbState>) -> Result<ExtractionStats, String> {
    let conn =
        db.0.lock()
            .map_err(|e| format!("Failed to lock DB: {}", e))?;

    // Query extraction queue
    let queue_depth: usize = conn
        .query_row(
            "SELECT COUNT(*) FROM extraction_queue WHERE status = 'pending'",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);

    let last_processed: Option<String> = conn
        .query_row(
            "SELECT processed_at FROM extraction_queue WHERE status = 'completed' ORDER BY processed_at DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .ok();

    let failed_count: usize = conn
        .query_row(
            "SELECT COUNT(*) FROM extraction_queue WHERE status = 'failed'",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);

    let total_processed: usize = conn
        .query_row(
            "SELECT COUNT(*) FROM extraction_queue WHERE status = 'completed'",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);

    // Query ai_memory for today's facts
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let facts_today: usize = conn
        .query_row(
            "SELECT COUNT(*) FROM ai_memory WHERE date(created_at) = date(?) AND deleted_at IS NULL",
            [&today],
            |row| row.get(0),
        )
        .unwrap_or(0);

    let total_facts: usize = conn
        .query_row(
            "SELECT COUNT(*) FROM ai_memory WHERE deleted_at IS NULL",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);

    // Calculate failure rate
    let total_attempts = total_processed + failed_count;
    let failure_rate = if total_attempts > 0 {
        (failed_count as f64 / total_attempts as f64) * 100.0
    } else {
        0.0
    };

    // Determine status
    let status = if queue_depth > 20 || failure_rate > 10.0 {
        "degraded"
    } else if queue_depth >= 5 {
        "behind"
    } else {
        "healthy"
    }
    .to_string();

    Ok(ExtractionStats {
        status,
        queue_depth,
        last_processed,
        facts_today,
        failed_count,
        total_processed,
        total_facts,
        failure_rate,
    })
}

#[tauri::command]
fn get_compaction_stats(
    app: tauri::AppHandle,
    db: tauri::State<DbState>,
) -> Result<CompactionStats, String> {
    let conn =
        db.0.lock()
            .map_err(|e| format!("Failed to lock DB: {}", e))?;

    // Get the active thread name (from the most recent message)
    let thread_name: String = conn
        .query_row(
            "SELECT project_id FROM chat_messages_local ORDER BY created_at DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .unwrap_or_else(|_| "No active thread".to_string());

    // Count total messages in this thread
    let messages_in_conversation: usize = conn
        .query_row(
            "SELECT COUNT(*) FROM chat_messages_local WHERE project_id = ?",
            [&thread_name],
            |row| row.get(0),
        )
        .unwrap_or(0);

    // For "messages in context", we use the latest budget stats
    let budget_state = app.state::<ai::context_budget::BudgetState>();
    let messages_in_context = ai::context_budget::get_latest_budget_stats(&budget_state)
        .map(|b| b.messages_included)
        .unwrap_or(0);

    // Count compaction summaries for this thread
    let compaction_summary_count: usize = conn
        .query_row(
            "SELECT COUNT(*) FROM compaction_summaries WHERE thread_id = ?",
            [&thread_name],
            |row| row.get(0),
        )
        .unwrap_or(0);

    // Get last compaction timestamp
    let last_compaction: Option<String> = conn
        .query_row(
            "SELECT created_at FROM compaction_summaries WHERE thread_id = ? ORDER BY created_at DESC LIMIT 1",
            [&thread_name],
            |row| row.get(0),
        )
        .ok();

    Ok(CompactionStats {
        thread_name,
        messages_in_conversation,
        messages_in_context,
        compaction_summary_count,
        last_compaction,
    })
}

// ─── Manual Sync Command (#2522, ADR-0063 Stage 7a) ────────────────────────────────

/// Frontend-triggered "Sync Now" command. Wraps the same pull-then-flush
/// sequence the app already runs on startup (~line 1304-1307) and on the
/// 5-minute periodic timer (~line 1330-1339).
///
/// Deliberately opens a *fresh* standalone connection via
/// `database::open_connection(None)` rather than locking `db.0` directly —
/// `pull_pending_changes_blocking`/`flush_sync_queue_blocking` perform
/// blocking HTTP I/O, and holding the shared `db.0` mutex across that I/O
/// is the exact deadlock class fixed in #1180 (see sync_client.rs Fix A/B
/// regression tests and docs/audits/2026-06-02-freeze-cluster-1347-1180.md).
/// `db` is still taken as `State<DbState>` to match the periodic-thread
/// pattern: its `Arc` is cloned into a `DbState` passed down for API
/// compatibility, but its mutex is never locked on this thread.
#[tauri::command]
fn sync_now(
    db: tauri::State<DbState>,
    app_handle: tauri::AppHandle,
) -> Result<sync_client::SyncNowResult, String> {
    eprintln!("[sync_now] called");

    let conn = database::open_connection(None)
        .map_err(|e| format!("sync_now: failed to open DB connection: {}", e))?;
    let db_state = DbState(db.0.clone());

    sync_client::sync_now_blocking(&conn, Some(&app_handle), &db_state)
}


/// Outcome of a single `run_notes_fts_drift_gate` invocation — used by tests
/// to pin "startup does NOT reindex when no rebuild is warranted" (#3072),
/// since a clean rebuild and a skipped rebuild both leave `notes_fts` content
/// identical (the correct content, either way) and can't be told apart by
/// querying search results alone.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum NotesFtsDriftOutcome {
    /// `integrity-check` reported no problem — no rebuild was attempted.
    Clean,
    /// `integrity-check` reported drift and the native `'rebuild'` succeeded.
    Rebuilt,
    /// `integrity-check` reported drift and the native `'rebuild'` failed.
    RebuildFailed,
}

/// `notes_fts` drift gate (#3072).
///
/// `notes_fts` is an external-content table (`content='notes'`,
/// `migrations.rs::create_fts_virtual_tables`) — its triggers
/// (`migrations.rs::create_fts_triggers`) are the only writer that should
/// ever touch it during normal operation, and have been correct since #486
/// (9df638a4) restored them to index the full `notes.body`. This function
/// replaces a block that was leftover from the standalone-table design #486
/// superseded (305afa11): that block ran unconditionally on EVERY launch,
/// non-transactionally — `DELETE FROM notes_fts` (empties the shadow index),
/// then a per-row disk read + `INSERT` loop to refill it from the `.md`
/// files, each statement its own autocommit. Any reader landing in the gap
/// between the `DELETE` and the loop finishing saw an index/`notes` drift
/// and reported "malformed" (#2352/#2566/#2610) — which self-healed on the
/// next launch because the same unconditional block ran again, which is why
/// it was reported four times and fixed zero times.
///
/// Fix: run the FTS5-native `'integrity-check'` first (cheap — walks the
/// shadow b-trees, does not re-tokenize content). Only if THAT reports a
/// problem do we run the FTS5-native `'rebuild'` (also native, sources from
/// `notes.body` exactly like the triggers do — no more disk reads, no more
/// second source of truth), wrapped in one explicit transaction together
/// with the status write below so no reader ever observes a partial/empty
/// index and the recorded outcome always matches what's actually in the
/// index.
///
/// "Empty is not zero" (#3072 scope item 4): `notes_fts_rebuild_status`
/// (`ai_settings` key) records what happened on the LAST attempt this logic
/// ran, not whether a rebuild happened to be literally necessary. Absence of
/// the key means this logic never ran at all this install (e.g. an older
/// binary, or a startup crash before reaching here) — distinct from `"ok"`
/// (index confirmed healthy, whether via a clean check or a successful
/// repair) and `"failed"` (a rebuild was required and did not complete; see
/// the `"[FTS-REBUILD-FAILED]"` log line, matching the existing
/// `"[STARTUP-ERROR]"` convention one line away).
pub(crate) fn run_notes_fts_drift_gate(conn: &rusqlite::Connection) -> NotesFtsDriftOutcome {
    // `rank=1` is NOT optional here: per the FTS5 docs (§6.5), for an
    // EXTERNAL CONTENT table, 'integrity-check' only compares the index
    // against the content table (`notes`) when rank=1 is supplied — without
    // it, the command only checks the index's own internal b-tree structure
    // and would never notice the index/content drift this whole PR exists to
    // catch (confirmed empirically: a direct `DELETE FROM notes_fts WHERE
    // rowid=N` passes a rank-less integrity-check every time, but correctly
    // fails a rank=1 check).
    let integrity_ok = conn
        .execute("INSERT INTO notes_fts(notes_fts, rank) VALUES('integrity-check', 1)", [])
        .is_ok();

    if integrity_ok {
        eprintln!("[FTS] notes_fts integrity-check passed — no rebuild needed");
        if let Err(e) = database::set_ai_setting(conn, "notes_fts_rebuild_status", "ok") {
            eprintln!("[STARTUP-ERROR] Failed to record notes_fts_rebuild_status: {}", e);
        }
        if let Err(e) = database::set_ai_setting(conn, "notes_fts_rebuild_at", &chrono::Utc::now().to_rfc3339()) {
            eprintln!("[STARTUP-ERROR] Failed to record notes_fts_rebuild_at: {}", e);
        }
        return NotesFtsDriftOutcome::Clean;
    }

    eprintln!("[FTS] notes_fts integrity-check FAILED — drift detected, rebuilding via native FTS5 'rebuild' (transactional)");
    if let Err(e) = conn.execute_batch("BEGIN;") {
        eprintln!("[STARTUP-ERROR] Failed to open notes_fts rebuild transaction: {}", e);
        // We couldn't even open a transaction — don't claim an outcome we
        // didn't actually reach. Leave the status key as whatever it already
        // was (absent = never ran, or a stale prior value) rather than write
        // a misleading one here.
        return NotesFtsDriftOutcome::RebuildFailed;
    }

    // Reuse migrations.rs's rebuild_fts_indexes — same native 'rebuild' SQL
    // already used at fresh-schema init (migrations.rs::init_schema); don't
    // fork a second copy.
    let rebuild_result = migrations::rebuild_fts_indexes(conn);
    let (status, outcome) = match &rebuild_result {
        Ok(_) => {
            eprintln!("[FTS] notes_fts rebuild succeeded");
            ("ok", NotesFtsDriftOutcome::Rebuilt)
        }
        Err(e) => {
            eprintln!("[FTS-REBUILD-FAILED] notes_fts rebuild failed: {}", e);
            ("failed", NotesFtsDriftOutcome::RebuildFailed)
        }
    };

    // Status write happens inside the same transaction as the rebuild
    // attempt so the two always agree on COMMIT — a committed "failed"
    // status is itself a correct, durable outcome (distinguishable from
    // "never ran"), not something to roll back. Rollback is reserved for
    // the case where we can't even persist the status (true DB-level
    // failure), so we don't commit a half-written status pair.
    let status_write = database::set_ai_setting(conn, "notes_fts_rebuild_status", status)
        .and_then(|_| database::set_ai_setting(conn, "notes_fts_rebuild_at", &chrono::Utc::now().to_rfc3339()));
    match status_write {
        Ok(_) => {
            if let Err(e) = conn.execute_batch("COMMIT;") {
                eprintln!("[STARTUP-ERROR] Failed to commit notes_fts rebuild transaction: {}", e);
            }
        }
        Err(e) => {
            eprintln!("[STARTUP-ERROR] Failed to record notes_fts_rebuild_status ({}): {}", status, e);
            let _ = conn.execute_batch("ROLLBACK;");
        }
    }

    outcome
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default();

    #[cfg(debug_assertions)]
    let builder = builder.plugin(tauri_plugin_playwright::init());

    builder
        .plugin(tauri_plugin_dialog::init())
        // #2339 PR 15b-alpha: registers the `tauri-plugin-fs` plugin so the
        // frontend's `@tauri-apps/plugin-fs` `writeTextFile()` call (used by
        // the sheet "Export as CSV…" flow, Editor.svelte's
        // `handleExportSheetCsv`) actually works at runtime. The Cargo
        // dependency and the `fs:default` capability grant already existed
        // in `Cargo.toml`/`capabilities/default.json` before this PR — this
        // is the one missing `.plugin()` registration call, not a new
        // capability or a new command. No schema change, no new Tauri
        // command.
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_deep_link::init())
        .setup(|app| {
            // #2723 PR B: register BEFORE anything else in this closure —
            // `get_startup_status` has no `DbState` dependency and must be
            // safely callable from the very first instant the frontend can
            // reach it, on every launch, regardless of which startup path
            // (fast/synchronous or window-first/background) this run takes.
            app.manage(startup::StartupStatus::new());

            // Listen for deep link events (password recovery, etc.)
            #[cfg(desktop)]
            {
                use tauri::{Listener, Emitter};
                let handle = app.handle().clone();
                app.listen("deep-link://new-url", move |event| {
                    let urls = event.payload();
                    eprintln!("[deep-link] Received: {}", urls);
                    // Emit to frontend
                    let _ = handle.emit("deep-link-received", urls);
                });
            }

            // Listen for router trigger events from extraction worker
            {
                use tauri::Listener;
                let handle = app.handle().clone();
                app.listen("trigger-router", move |_event| {
                    eprintln!("[ROUTER] Trigger event received from extraction worker");
                    let handle_clone = handle.clone();
                    tauri::async_runtime::spawn(async move {
                        // Get state from app handle
                        let db_state = handle_clone.state::<database::DbState>();

                        // Call router
                        let conn_result = db_state.0.lock();
                        match conn_result {
                            Ok(conn) => {
                                if let Err(e) = ai::memory_router::route_memory_facts(&*conn, handle_clone.clone()) {
                                    eprintln!("[ROUTER] Failed: {}", e);
                                }
                            }
                            Err(e) => {
                                eprintln!("[ROUTER] Failed to acquire DB lock: {}", e);
                            }
                        }
                    });
                });
            }

            // #671: Listen for trigger-consolidator events from router
            {
                use tauri::Listener;
                let handle = app.handle().clone();
                app.listen("trigger-consolidator", move |_event| {
                    eprintln!("[CONSOLIDATOR] Trigger event received from router");
                    let handle_clone = handle.clone();
                    tauri::async_runtime::spawn(async move {
                        let db_state = handle_clone.state::<database::DbState>();

                        // Resolve provider config.
                        // #2522 ADR-0063 Stage 2: the consolidation thread is sessionless
                        // (fired from a global event, not a chat session), so per D5
                        // ("session agent where derivable, designated default otherwise")
                        // it resolves the designated default agent's runtime instead of
                        // locking the retired AiStateWrapper singleton.
                        let (provider_name, api_key, model, endpoint, oc_runtime_ref) = {
                            let conn = db_state.0.lock().unwrap();
                            let cfg = match ai::runtime_resolution::resolve_runtime_config_for_default_agent(&conn) {
                                Ok(cfg) => cfg,
                                Err(e) => {
                                    eprintln!("[CONSOLIDATOR] No runtime configured: {}", e);
                                    return;
                                }
                            };
                            let (name, key, ep) = match &cfg.provider {
                                AiProvider::OpenRouter { api_key } => ("openrouter", api_key.clone(), None),
                                AiProvider::Anthropic { api_key } => ("anthropic", api_key.clone(), None),
                                AiProvider::OpenClaw { base_url, token, .. } => ("openclaw", token.clone(), Some(base_url.clone())),
                                AiProvider::Venice { api_key, .. } => ("venice", api_key.clone(), None),
                                _ => {
                                    eprintln!("[CONSOLIDATOR] Unsupported provider");
                                    return;
                                }
                            };
                            // ADR-0067 D2/D5: never cfg.agent_id — that's Pane's local UUID.
                            (name.to_string(), key, cfg.model.clone(), ep, cfg.runtime_ref.clone())
                        };

                        // Run staleness check to find which files need consolidation
                        let targets = {
                            let conn = db_state.0.lock().unwrap();
                            ai::consolidator::check_consolidation_staleness(&*conn)
                        };

                        for target in targets {
                            if let Err(e) = ai::consolidator::consolidate_file(
                                &*db_state,
                                &handle_clone,
                                &target.file_path,
                                target.project_id.as_deref(),
                                &provider_name,
                                &api_key,
                                &model,
                                endpoint.as_deref(),
                                oc_runtime_ref.as_deref(),
                            ).await {
                                eprintln!("[CONSOLIDATOR] Failed for {}: {}", target.file_path.display(), e);
                            }
                        }
                    });
                });
            }

            // Resolve crsqlite.dylib resource path
            #[cfg(target_os = "macos")]
            const CRSQLITE_FILENAME: &str = "crsqlite.dylib";

            #[cfg(target_os = "linux")]
            const CRSQLITE_FILENAME: &str = "crsqlite.so";

            #[cfg(target_os = "windows")]
            const CRSQLITE_FILENAME: &str = "crsqlite.dll";

            let crsqlite_path = app
                .path()
                .resource_dir()
                .ok()
                .map(|dir| dir.join("resources").join(CRSQLITE_FILENAME));

            // Cache the crsqlite path so restore can reopen with the same extension (ADR-0056 R2).
            database::set_crsqlite_path(crsqlite_path.clone());

            // #2723 PR B: window-first startup when a migration is pending.
            //
            // Everything that used to run right here, inline, through the
            // end of this closure (§486 note-body integrity, the two orphan
            // sweeps, every other `DbState` consumer below) blocked
            // `.setup()` from returning — which blocked Tauri's native event
            // loop from starting (`.setup()` runs before `.run()` pumps
            // events per Tauri's own lifecycle), so the declarative window
            // in `tauri.conf.json` (no `WebviewWindowBuilder`, no
            // `"visible": false` — it paints as soon as the event loop
            // starts) never painted until the whole chain — including the
            // pre-migration backup and schema migration — finished. On a
            // migrating launch with a large DB that was a multi-minute
            // blank window with zero feedback (Reed's audit,
            // docs/audits/2026-10-23-scope-slow-launch-updating-screen.md §3).
            //
            // Fix: reuse #353's already-shipped shape (commit `0880f573`,
            // "Move daily backup to background thread after window
            // renders") rather than invent a new one. A cheap, read-only
            // pre-check (`database::is_migration_pending` — opens a
            // throwaway connection, compares `schema_version` to
            // `SCHEMA_VERSION`, nothing more) decides BEFORE any real work
            // starts which of two paths this launch takes:
            //
            //   - No migration pending (the overwhelming majority of every
            //     launch): `complete_db_dependent_startup` runs inline, on
            //     this same setup thread, exactly as this code always did —
            //     same function body, same thread, same `?` propagation on
            //     a hard failure. NO new thread is spawned on this path.
            //     Regression-proved, not just asserted: see
            //     `test_is_migration_pending_false_when_schema_current` /
            //     `test_is_migration_pending_false_for_fresh_install`
            //     (database.rs) for the predicate itself, and the
            //     `smoke-fresh-db` run in the PR description for the real
            //     compiled binary taking this exact path end to end.
            //   - Migration pending: `.setup()` records
            //     `StartupStatus::MigrationRequired` (so the frontend's
            //     very next `get_startup_status()` call — which cannot
            //     possibly happen before `.setup()` returns, since the
            //     event loop that lets the webview run JS hasn't started
            //     yet — sees it, no event-ordering race) and returns
            //     immediately, without having touched the DB at all, so
            //     `.run()` can start pumping events and the window can
            //     paint. The real work then runs on a background
            //     `std::thread::spawn` — `#353`'s exact shape.
            //
            // ORDERING GUARANTEE (#2723 PR B item 3 — Reed's explicit
            // requirement: "`set_schema_version` completes before any
            // dependent setup block runs"). This holds on BOTH paths,
            // structurally, not by convention: `complete_db_dependent_startup`
            // (below) calls `database::init_database_with_progress(...)?` —
            // which cannot return a `Connection` without
            // `migrations::run_migrations_with_progress` (and therefore
            // every `set_schema_version` call on every reachable match arm)
            // having already run to completion on it — and only THEN calls
            // `startup::mount_db_state_then`, whose own body manages
            // `DbState` and invokes the dependent-block closure in that
            // fixed order, enforced by the Rust compiler (see
            // `startup.rs`'s doc comment on `mount_db_state_then`, and
            // `startup::test_schema_version_committed_before_db_state_is_managed`,
            // which exercises this exact function against a real v91→v92
            // migration).
            let migration_pending = database::is_migration_pending(crsqlite_path.clone());

            if migration_pending {
                eprintln!("[STARTUP] #2723: schema migration pending — opening window first, deferring DB init + migration to a background thread");
                app.state::<startup::StartupStatus>().set_migration_required();
                {
                    use tauri::Emitter;
                    let _ = app.emit("startup://migration-required", ());
                }

                let app_handle_bg = app.handle().clone();
                let crsqlite_path_bg = crsqlite_path.clone();
                std::thread::spawn(move || {
                    use tauri::Emitter;
                    match complete_db_dependent_startup(&app_handle_bg, crsqlite_path_bg) {
                        Ok(()) => {
                            app_handle_bg.state::<startup::StartupStatus>().set_ready();
                            let _ = app_handle_bg.emit("startup://ready", ());
                            eprintln!("[STARTUP] #2723: background migration + startup complete — app ready");
                        }
                        Err(e) => {
                            eprintln!("[STARTUP] #2723: FATAL on background migration thread: {}", e);
                            app_handle_bg.state::<startup::StartupStatus>().set_failed(e.clone());
                            let _ = app_handle_bg.emit("startup://failed", serde_json::json!({ "message": e }));
                        }
                    }
                });
            } else {
                // Fast path — byte-for-byte what this closure always did:
                // runs inline, on this setup thread, propagates a hard
                // failure via `?` exactly as before. No new thread.
                complete_db_dependent_startup(&app.handle().clone(), crsqlite_path)
                    .map_err(|e| format!("Failed to initialize database: {}", e))?;
                app.state::<startup::StartupStatus>().set_ready();
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            // #2723 PR B: window-first startup — no DbState dependency, safe
            // to call at any point, including while a migration is still
            // running on the background thread.
            startup::get_startup_status,

            // Note CRUD (§17.1)
            notes::create_note,
            notes::get_note,
            notes::get_notes,
            notes::get_project_notes_for_kanban,
            notes::get_tasks,
            notes::list_upcoming_tasks,
            notes::search_notes,
            notes::update_note,
            notes::delete_note,
            notes::force_delete_note,
            notes::restore_note,
            notes::purge_note,
            notes::get_trash,
            notes::empty_trash,
            notes::reorder_notes,
            notes::get_subtasks,
            notes::rebuild_index,

            // Subtask CRUD (§1.9, ADR-0057, #2092)
            subtasks::create_subtask,
            subtasks::get_subtasks_for_note,
            subtasks::update_subtask,
            subtasks::delete_subtask,
            subtasks::get_due_today_items,
            subtasks::get_subtask_parent_note_id,

            // Sheets CRUD Backend (#2339 PR 5a, §33.2–§33.8)
            sheets::create_sheet,
            sheets::get_sheet_data,
            sheets::create_sheet_column,
            sheets::create_sheet_columns,
            sheets::update_sheet_column,
            sheets::delete_sheet_column,
            sheets::reorder_sheet_columns,
            sheets::create_sheet_row,
            sheets::create_sheet_rows,
            sheets::update_sheet_row,
            sheets::delete_sheet_row,
            sheets::reorder_sheet_rows,
            sheets::update_sheet_cell,
            sheets::update_sheet_cells_style_batch,
            sheets::delete_sheet_cell,

            // Sheets Snapshot Capture/Restore Backend (#2339 PR 8, §33.9)
            sheets_snapshots::restore_sheet_snapshot,
            sheets_snapshots::get_sheet_snapshot_computed_values,

            // Sheets CSV Export (#2339 PR 5c, §33.12) — callable-but-unwired, design-gated UI
            sheets::export_sheet_csv,

            // Sheets board card face (#2339 PR 15a) — read-only counts for the
            // Kanban/AllTasks/folder card micro-grid + cols × rows footer metric
            sheets::get_sheet_card_summary,
            sheets::get_sheet_card_summaries,

            // Template CRUD + Apply (§32, ADR-0058, #193)
            templates::list_templates,
            templates::get_template,
            templates::save_template,
            templates::overwrite_template,
            templates::rename_template,
            templates::delete_template,
            templates::apply_template,
            templates::list_templates_by_name,
            notes::get_vital_tasks,
            notes::toggle_vital,
            notes::get_project_note_counts,

            // Settings Stubs (v9.0.12)
            notes::check_openclaw_status,
            notes::get_ai_setting,
            notes::set_ai_setting,
            notes::should_show_onboarding,
            // #3106 M1 (Reed's security review): notes::ai_save_config
            // registration removed -- the command was dead (only caller was
            // the unimported AiSection.svelte.new draft file); the function
            // itself is deleted too, not just unregistered.
            notes::ai_load_config,
            notes::get_chat_message_count,
            notes::get_chat_memory,
            notes::save_chat_memory,
            notes::delete_old_chat_messages,

            // Gateway Pairing (§22C.11A) — auto_discover_openclaw_config removed per #1108
            notes::gateway_fetch_fingerprint,
            notes::gateway_exchange_pairing_code,

            notes::get_vault_path_setting,
            notes::set_note_type,
            notes::get_system_note,
            notes::get_project_memory,
            notes::get_unrouted_memories,
            notes::mark_memories_routed,
            notes::get_daily_note,
            notes::move_note,
            notes::move_folder,
            notes::set_vault_path_setting,

            // Project/Folder CRUD (§17.2)
            // Import commands
            import::scan_import_folder,
            import::import_folder,

            projects::create_project,
            projects::get_projects,
            projects::update_project,
            projects::delete_project,
            projects::create_folder,
            projects::get_folders,
            projects::get_project_or_folder,
            projects::update_folder,
            projects::delete_folder,
            projects::get_folder_delete_preview,
            projects::toggle_folder_hidden,
            projects::get_hidden_folder_ids,
            projects::list_categories,
            projects::set_project_category,
            projects::set_category_color,
            projects::rename_category,
            projects::delete_category,

            // Context Pins (#471)
            context_pins::pin_note_to_context,
            context_pins::unpin_note_from_context,
            context_pins::is_note_pinned,
            context_pins::list_context_pins,
            context_pins::get_context_budget_usage,

            // Tag Management (§17.3)
            tags::get_tags,
            tags::create_tag,
            tags::update_tag,
            tags::delete_tag,
            tags::add_note_tags,
            tags::remove_note_tags,
            tags::get_notes_by_tag,

            // Dashboard (v9.0.2)
            dashboard::get_dashboard_modules,
            dashboard::get_dashboard_config,
            dashboard::create_dashboard_module,
            dashboard::update_dashboard_module,
            dashboard::update_module_content_cmd,
            dashboard::delete_dashboard_module,
            dashboard::toggle_dashboard_module,
            dashboard::reorder_dashboard_modules,
            dashboard::get_module_items_cmd,
            dashboard::create_module_item_cmd,
            dashboard::update_module_item_cmd,
            dashboard::delete_module_item_cmd,
            dashboard::get_daily_entries_cmd,
            dashboard::save_daily_entry_cmd,
            dashboard::get_habit_entries_cmd,
            dashboard::save_habit_entry_cmd,
            dashboard::get_reflection_entries_cmd,
            dashboard::save_reflection_entry_cmd,
            dashboard::get_gratitude_entries_cmd,
            dashboard::save_gratitude_entry_cmd,
            dashboard::delete_gratitude_entry_cmd,
            dashboard::get_home_state,

            // Sync status (v9.1.0)
            database::get_sync_status,

            // Auth & Device Registration (v9.1.2)
            sync::commands::auth_sign_in,
            sync::commands::auth_sign_out,
            sync::commands::auth_refresh,
            sync::commands::auth_request_password_reset,
            sync::commands::auth_change_password,
            sync::commands::register_device_cmd,
            sync::commands::get_auth_session,
            sync::commands::save_auth_session,
            sync::commands::clear_auth_session,
            sync::commands::get_hostname,
            sync::commands::get_install_uuid,
            sync::commands::init_encryption,
            sync::commands::is_encryption_ready,
            sync::commands::clear_encryption,
            sync::commands::test_encryption,

            // Sync Engine (v9.1.3)
            sync::commands::sync_push,
            sync::commands::sync_pull,
            sync::commands::sync_configure,
            sync::commands::get_sync_info,
            sync::commands::sync_files_push,
            sync::commands::sync_files_pull,

            // Manual Sync Now (#2522, ADR-0063 Stage 7a)
            sync_now,

            // Auto-sync (v9.2.7)
            sync::commands::start_auto_sync,
            sync::commands::stop_auto_sync,
            sync::commands::notify_sync_change,

            // AI Provider (v9.3.0)
            ai::connections::ai_configure_and_save,
            ai::connections::ai_status,
            ai::connections::ai_configure_and_save,
            ai::connections::ai_status,
            ai::chat::get_latency_stats,
            ai::chat::get_chat_retry_count,
            ai::connections::list_available_models,
            ai::connections::fetch_available_models,
            ai::connections::fetch_internal_provider_models,
            ai::connections::ai_check_ollama,
            ai::connections::ollama_probe_two_step,
            ai::connections::ai_get_config,

            // Provider Connection Cards (ADR-0063 Stage 5, #2522) — net-new, see PR body
            ai::connections::ai_list_runtime_connections,
            ai::connections::ai_disconnect_runtime,
            ai::connections::get_project_settings,
            ai::connections::update_project_settings,

            // Web Search Settings card (§4.6.8, Part 2, #2591) — net-new, see PR body
            ai::connections::save_brave_key,
            ai::connections::brave_key_status,
            ai::connections::remove_brave_key,
            // Venice API key management (ADR-0065 D4/D8)
            ai::connections::validate_venice_key,
            ai::connections::save_venice_key,
            ai::connections::venice_key_status,
            ai::connections::remove_venice_key,
            ai::connections::save_searxng_endpoint,
            ai::connections::clear_searxng_endpoint,
            ai::connections::test_web_search,
            ai::image_gen::fetch_openrouter_image_models_cmd,
            ai::image_gen::fetch_venice_image_models_cmd,
            ai::image_gen::test_generate_image,

            // AI Chat Thread/Message Management (v9.3.1 — surface-keyed commands removed in #395 PR4)

            // AI Streaming (v10.4.0)
            ai::chat::ai_chat_stream,
            ai::chat::stop_turn,
            ai::chat::set_viewing_session,

            // AI Memory System (v10.6.2)
            ai::scheduler::ai_extract_memory,
            ai::scheduler::ai_enqueue_extraction,
            ai::scheduler::ai_synthesize_memory,
            ai::scheduler::ai_consolidate_memory,
            ai::scheduler::ai_synthesize_all_memories,
            ai::scheduler::ai_save_memory,
            ai::scheduler::ai_get_memory,
            ai::scheduler::ai_run_decay_check,
            // #2277 Fix 1: ai_get_soul/ai_set_soul/ai_get_workspace_memory/ai_set_workspace_memory
            // removed — they read/wrote the global .pane/ai/soul.md and workspace-memory.md files.
            // Per-agent SOUL.md/MEMORY.md under .pane/system/{UUID}/ are now the sole source.
            // ai_save_upload removed — global uploads path retired (#2288 PR 4, §24.10.1b).
            ai::scheduler::ai_dismiss_memory,
            ai::chat::ai_get_default_soul,

            // Memory Management (v11.27.0 — #68, #69)
            ai::scheduler::get_all_memories,
            ai::scheduler::update_memory,
            ai::scheduler::delete_memory,
            ai::scheduler::save_to_memory,

            // Chat Message Persistence (v10.9.0)
            ai::chat::save_chat_message,
            ai::chat::load_chat_messages,
            ai::chat::get_compaction_cutoff,

            // §36.4/§36.4.5 (#3050 PR 5) — live chip status (Matt's addition):
            // the spawn/schedule chip's own job_id read, by id.
            ai::scheduler::get_job_status,

            // §36.3.8 (#3050 PR 8) — Home Scheduled panel: the one query
            // (every job except spawns, across projects) and "Run now".
            ai::scheduler::list_scheduled_jobs,
            ai::scheduler::run_job_now,
            // §36.3.7 carry-over (#3050 PR 9) — Pause/Delete on a running
            // row calls this first, then the normal pause/cancel command.
            ai::scheduler::wait_for_running_job_to_stop,

            // §36.3.8 (#3050 PR 8) — the panel's inline pause/resume/
            // delete (thin wrappers over the existing AI tool handlers)
            // and the preset-card/New-schedule create path (thin wrapper
            // over execute_schedule_job, same path the schedule_job AI
            // tool uses).
            ai::tools::pause_job_command,
            ai::tools::resume_job_command,
            ai::tools::cancel_job_command,
            ai::tools::create_scheduled_job,
            // #3134 C26 — Edit action: rewrites the same agent_jobs row
            // (never a new one), sibling to create_scheduled_job above.
            ai::tools::update_job,
            // #3151 PR2/C24 — Done tab: "Clear done" (archive, shared
            // mechanism with the tick's auto-archive sweep) and "Delete"
            // on a Done card (hard delete job + its own session, NOT
            // cancel_job_command relabeled).
            ai::scheduler::clear_done_jobs_command,
            ai::tools::delete_archived_job_command,

            // External Message Ingestion & Proactive Delivery (§30.11, #2410)
            ai::ingest::ingest_external_messages,
            // PR 3 (#2410): frontend 60s poll wrappers around PR 2's internal
            // pull_oc_transcript_for_session / session_uses_openclaw_runtime.
            ai::ingest::pull_oc_transcript_for_session_cmd,
            ai::ingest::session_uses_openclaw_runtime_cmd,

            // #2794: backend echo for resolveSenderName()'s 'Pane' fallback hit —
            // see docs/audits/2026-08-31-2794-2783-formal-diagnosis.md, "Diagnostic
            // visibility gap". Unconditional eprintln!, no pane-debug gate.
            ai::connections::log_sender_name_fallback_cmd,

            // Chat Session Commands (§30.2, #395)
            ai::chat::create_session,
            ai::chat::create_session_with_crew,
            ai::chat::update_session_crew,
            ai::chat::list_sessions,
            ai::chat::switch_session,
            ai::chat::hide_session,
            ai::chat::unhide_session,
            ai::chat::rename_session,
            ai::chat::get_active_session,

            // OpenClaw Onboarding (v11.3.0)
            ai::ingest::detect_openclaw_agents,
            ai::ingest::sync_initial_import,

            // #2647/#2648/#2644 (PR1 5a): unified OC pairing fix — first-pair/re-pair signal
            ai::ingest::oc_agent_row_exists,

            // Provider-Switch Re-import (v1.0, #956, §24.5.5)
            ai::ingest::check_oc_agent_collisions,
            ai::ingest::reimport_oc_agents,

            // ADR-0045 D4 restore_non_oc_main (#1492) retired by ADR-0059 D3 (picker/
            // forced-create re-targeting) and removed entirely per ADR-0062 D8 (#2522) —
            // dead command since 2026-07-26 (#2030 PR B) — already removed, no trace remains.

            // Integrity Check (v11.26.8)
            ai::ingest::run_integrity_check,
            ai::ingest::check_identity_divergence,

            // Embedding Engine Switch (ADR-0048 / §5.9 / PR 3 of 3)
            ai::scheduler::switch_embedding_engine,
            ai::scheduler::fetch_embedding_models,
            ai::scheduler::get_embeddings_counts,

            // System Files (v11.17.0)
            notes::write_system_file,
            system_files::read_openclaw_workspace,
            system_files::read_workspace_from_path,
            system_files::read_openclaw_agents,
            system_files::discover_pane_agents,
            system_files::gateway_chat_completions,
            system_files::bridge_health_check,
            system_files::ensure_logs_directory,
            system_files::write_log_file,
            system_files::ensure_tasks_directory,
            system_files::fetch_gateway_models,
            system_files::validate_openrouter_key,
            system_files::validate_anthropic_key,

            // Attachments (v10.47.0)
            attachments::save_attachment,
            attachments::copy_attachment_from_path,  // #1799: native fs copy for large files
            attachments::get_attachment,
            attachments::get_attachment_data,
            attachments::delete_attachment,
            attachments::list_project_attachments,
            attachments::resolve_attachment_path,
            attachments::open_attachment_externally, // #401 PR1: open in OS default app

            // og-enrichment / link previews (#1780, ADR-0049 Decision 2)
            og_fetch::fetch_link_preview,
            og_fetch::get_cached_link_preview,

            // Notifications (v11.1.0)
            notifications::get_notifications,
            notifications::mark_notification_read,
            notifications::mark_all_notifications_read,
            notifications::dismiss_notification,
            notifications::get_unread_count,
            notifications::push_notification,
            notifications::check_due_dates_cmd,

            // Keychain (v11.10.12)
            versions::get_note_versions,
            versions::get_note_version,
            versions::restore_note_version,

            store_supabase_password,
            get_supabase_password,
            clear_supabase_password,

            // Backup (v0.17.71 — §25)
            create_backup,
            list_backups,
            restore_backup,

            // Onboarding Backup Restore (§6.1.4)
            check_fresh_install,
            restore_backup_from_file,
            restore_backup_confirmed_cmd,

            // Health Dashboard (v0.17.82 — §27.13)
            get_context_budget_stats,
            get_extraction_stats,
            get_compaction_stats,

            // Daily Logs (v11.29.4)
            logs::list_daily_logs,
            logs::read_daily_log,

            // Agents (v0.17.9 — §24)
            agents::get_agents,
            agents::get_agent,
            agents::get_main_agent,
            agents::upsert_agent,
            agents::update_agent,
            agents::reactivate_agent,
            agents::get_system_username,
            // #2030 PR B: Agent create/edit UI (§24.10, ADR-0059 D2/D4)
            agents::create_local_agent,
            // seed_agent_uploads / seed_agent_interview removed (#2288 PR 3, §24.10.1c) —
            // AgentCreateOverlay.svelte now calls the shared seedGeneration.ts pipeline
            // instead of these Rust commands.
            agents::set_main_agent,
            agents::check_name_collision,

            // Cleanup Pending Flag (v0.65.32 — #1079)
            sync::commands::check_cleanup_pending,
            sync::commands::write_cleanup_flag,
            sync::commands::clear_cleanup_flag,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

/// #2723 PR B: the entire DB-dependent portion of startup — init + migrate,
/// mount `DbState`, then every dependent setup block (§486 note-body
/// integrity, the §33.8/workshop orphan sweeps, and everything after:
/// sync/auto-sync/budget/model-registry state, credential migration, the
/// §24.1 agent integrity check, and the already-background-thread work —
/// FTS rebuild, consolidation staleness check, sync pull, periodic pull,
/// notification watcher, due-date check, daily backup, file watcher).
///
/// Called either inline from `.setup()` (fast path, main/setup thread — see
/// `run()`'s doc comment at its call site) or from a background
/// `std::thread::spawn` (window-first path, #2723 PR B, when
/// `database::is_migration_pending` said yes). This function's body is
/// IDENTICAL either way — that identity is the ordering guarantee's whole
/// point: there is no separate "slow path" implementation to drift out of
/// sync with the fast path, only one function called from two places.
fn complete_db_dependent_startup(
    app: &tauri::AppHandle,
    crsqlite_path: Option<std::path::PathBuf>,
) -> Result<(), String> {
    // Initialize database with cr-sqlite extension, with migration/backup
    // progress wired to `app` (#2723 PR B — `backup://progress` already
    // existed from PR A; `migration://progress` is new, see migrations.rs's
    // `migration_step`). On the fast (non-migrating) path this is the exact
    // same call `init_database` always made — progress emission is a no-op
    // when there is nothing to report.
    let conn = database::init_database_with_progress(crsqlite_path, Some(app))?;

    // #2723 PR B ordering guarantee (see this function's call site in
    // `run()` for the full explanation): `conn` cannot exist on this line
    // without `init_database_with_progress` having already run
    // `set_schema_version` to completion on every reachable match arm, and
    // `mount_db_state_then` will not run the dependent-block closure below
    // until `DbState::new(conn)` has already been mounted on `app`.
    startup::mount_db_state_then(app, conn, |app| {
            // §486: Verify note body integrity
            {
                let db_state = app.state::<DbState>();
                let conn = db_state.0.lock().unwrap();
                let null_body_count: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM notes WHERE body IS NULL AND deleted_at IS NULL AND (note_type IS NULL OR note_type NOT IN ('soul','memory','rules','identity','user','tools','agents','heartbeat','daily','sheet'))",
                    [],
                    |row| row.get(0),
                ).unwrap_or(0);

                if null_body_count > 0 {
                    eprintln!("[INTEGRITY] Found {} notes with NULL body, attempting repair...", null_body_count);
                    // Auto-repair from files
                    let notes: Vec<(String, String)> = conn.prepare(
                        "SELECT id, file_path FROM notes WHERE body IS NULL AND deleted_at IS NULL AND (note_type IS NULL OR note_type NOT IN ('soul','memory','rules','identity','user','tools','agents','heartbeat','daily','sheet'))"
                    ).unwrap()
                    .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
                    .unwrap()
                    .filter_map(|r| r.ok())
                    .collect();

                    for (id, file_path) in notes {
                        if let Ok(content) = database::read_note_content(&file_path) {
                            let _ = conn.execute("UPDATE notes SET body = ?1 WHERE id = ?2", rusqlite::params![content, id]);
                        }
                    }
                    println!("[INTEGRITY] Repair complete");
                }
            }

            // §33.8 (#2339 PR 4): startup orphan sweep for sheet side tables
            // (sheet_columns/sheet_rows/sheet_cells). Concurrent delete-vs-edit
            // (CRR, no FK) can leave dangling side-table rows behind after sync
            // converges — sweep them on every launch, same phase as the §486
            // body-integrity block above. Single implementation in
            // integrity::sweep_orphaned_sheet_rows, shared with the post-sync
            // trigger in sync/engine.rs.
            {
                let db_state = app.state::<DbState>();
                let conn = db_state.0.lock().unwrap();
                integrity::sweep_orphaned_sheet_rows(&conn);
            }

            // #2288 PR 2 (agents.md §24.10.1e): startup orphan sweep for
            // workshop-scoped notes. These are DB-only notes created during
            // onboarding/create-agent's Memory→Soul review step; they're
            // purged via purge_note when the workshop finishes normally
            // (saveAndComplete), but a crashed/abandoned session leaves them
            // behind with project_id = WORKSHOP_SCOPE_ID and no way to reach
            // them again (the workshop UI holds the only reference to their
            // note ids). Sweep them on every startup — they're DB-only
            // (file_path = ""), so deleting them is never a data-loss risk.
            //
            // Cascade parity with purge_note (Reed audit WARN, #2288 PR 2):
            // purge_note deletes the note's file, cascades subtasks +
            // subtask notifications via cascade_delete_subtasks_for_note,
            // then deletes the notes row. Sentinel-scoped notes are always
            // DB-only (file_path = "") so there's no file to delete here,
            // but the sweep still owes the same subtask/notification/
            // note_versions cascade purge_note performs for every other
            // note it deletes.
            {
                let db_state = app.state::<DbState>();
                let conn = db_state.0.lock().unwrap();

                let orphan_ids: Vec<String> = conn
                    .prepare("SELECT id FROM notes WHERE project_id = ?1")
                    .and_then(|mut stmt| {
                        stmt.query_map(
                            rusqlite::params![notes::WORKSHOP_SCOPE_ID],
                            |row| row.get::<_, String>(0),
                        )?
                        .collect::<Result<Vec<_>, _>>()
                    })
                    .unwrap_or_default();

                if !orphan_ids.is_empty() {
                    match conn.execute("BEGIN", []) {
                        Ok(_) => {}
                        Err(e) => {
                            eprintln!("[startup] Failed to begin orphan sweep transaction: {}", e);
                        }
                    }

                    let mut sweep_failed = false;
                    for note_id in &orphan_ids {
                        if let Err(e) =
                            subtasks::cascade_delete_subtasks_for_note(&conn, note_id)
                        {
                            eprintln!(
                                "[startup] Orphan sweep: subtask cascade failed for note {}: {}",
                                note_id, e
                            );
                            sweep_failed = true;
                            break;
                        }

                        if let Err(e) = conn.execute(
                            "DELETE FROM note_versions WHERE note_id = ?1",
                            rusqlite::params![note_id],
                        ) {
                            eprintln!(
                                "[startup] Orphan sweep: note_versions delete failed for note {}: {}",
                                note_id, e
                            );
                            sweep_failed = true;
                            break;
                        }
                    }

                    if !sweep_failed {
                        match conn.execute(
                            "DELETE FROM notes WHERE project_id = ?1",
                            rusqlite::params![notes::WORKSHOP_SCOPE_ID],
                        ) {
                            Ok(_) => sweep_failed = false,
                            Err(e) => {
                                eprintln!(
                                    "[startup] Failed to clean up orphaned workshop notes: {}",
                                    e
                                );
                                sweep_failed = true;
                            }
                        }
                    }

                    if sweep_failed {
                        conn.execute("ROLLBACK", []).ok();
                        eprintln!(
                            "[startup] Orphan sweep rolled back — {} workshop note(s) left in place for retry on next startup",
                            orphan_ids.len()
                        );
                    } else if let Err(e) = conn.execute("COMMIT", []) {
                        eprintln!("[startup] Failed to commit orphan sweep transaction: {}", e);
                        conn.execute("ROLLBACK", []).ok();
                    } else {
                        eprintln!(
                            "[startup] Cleaned up {} orphaned workshop note(s) (cascaded subtasks, notifications, note_versions)",
                            orphan_ids.len()
                        );
                    }
                }
            }

            // Initialize sync manager state
            app.manage(SyncManagerState(Arc::new(Mutex::new(SyncManager::new()))));

            // Initialize auto-sync state
            app.manage(AutoSyncState(Arc::new(AutoSync::new())));

            // Initialize budget state for context budget manager
            app.manage(ai::context_budget::BudgetState::new());
            // Initialize model registry for context-window lookup (#1402-C)
            app.manage(ai::provider::ModelRegistry::new());

            // #2522 ADR-0063 Stage 2: the `AiConfig`/`AiStateWrapper` startup cache is
            // retired — runtime + model are now resolved fresh from `ai_agents` on every
            // dispatch (see `ai::runtime_resolution`), so there is no in-memory provider
            // state to warm here. What remains from the old block is the one-time
            // migration work that has to run before any credential read, regardless of
            // whether a provider ends up resolvable.
            let db_for_ai = app.state::<DbState>();
            if let Ok(conn) = db_for_ai.0.lock() {
                // ADR-0044: Run credential migration before reading credentials.
                // Idempotent — fast-paths on State B (flag set).
                ai::credential_migration::run_credential_migration(&conn);

                // C50 (#3171 PR 1): Feature-key consolidation — Smart Features
                // (`ai_internal_key`) and Semantic Search (`embeddings_api_key`)
                // each held their own copy of a provider key instead of reading
                // Provider Connections' own keychain slot. Backfill-if-absent-
                // else-discard, copy-confirm-clear. Must run AFTER the migration
                // above (needs its keychain-aware reads already routing
                // correctly) and BEFORE anything reads a feature's credential.
                // Idempotent — fast-paths once its own flag is set.
                ai::credential_migration::run_feature_key_consolidation(&conn);

                // ADR-0044 Addendum A (#1498): Sync master key migration.
                // Moves sync_key.bin → OS keychain (account "sync_master_key").
                // Must run before sync periodic loop (lib.rs:~1001) and
                // notification poller (lib.rs:~1024) reach steady state.
                // Idempotent — fast-paths when DB flag is set (State B).
                sync::keystore_migration::run_sync_key_migration(&conn);
            }

            // #1410: Eagerly populate ModelRegistry on startup — background async task.
            // Fires unconditionally on every launch; no UI gate, no window dependency.
            // #2522 ADR-0063 Stage 3+4 (D7.3): iterates every connected runtime (all
            // agents' distinct `runtime_type`s), not just the default agent's — see
            // `populate_model_registry_from_provider` for the enumeration. No connected
            // runtimes is an expected first-launch state, not an error worth surfacing.
            {
                let registry_arc = app.state::<ai::provider::ModelRegistry>().0.clone();
                let db_arc = app.state::<DbState>().0.clone();
                tauri::async_runtime::spawn(async move {
                    let db_wrapper = DbState(db_arc);
                    if let Err(e) = ai::connections::populate_model_registry_from_provider(
                        &db_wrapper, registry_arc,
                    ).await {
                        eprintln!("[startup] ModelRegistry population skipped: {}", e);
                    }
                });
            }

            // Start background extraction worker (§27.7)
            {
                let db_for_worker = db_for_ai.0.clone();
                let app_for_worker = app.clone();
                tauri::async_runtime::spawn(async move {
                    ai::extraction_worker::start_extraction_worker(
                        db_for_worker,
                        app_for_worker,
                    )
                    .await;
                });
            }

            // §36.3/§36.11 (#3050 PR 2): the scheduler tick — same shape as the
            // extraction worker above (free-standing async task via
            // `tauri::async_runtime::spawn`, never a `#[tauri::command]`, never on
            // the main thread — #3085's lesson). `start_scheduler_tick` runs one
            // launch pass immediately, then ticks every `RUNTIME_TICK_SECS`.
            {
                let db_for_scheduler = db_for_ai.0.clone();
                let app_for_scheduler = app.clone();
                tauri::async_runtime::spawn(async move {
                    ai::scheduler::start_scheduler_tick(
                        db_for_scheduler,
                        app_for_scheduler,
                    )
                    .await;
                });
            }

            println!("✓ Pane v{} initialized", env!("CARGO_PKG_VERSION"));
            println!("✓ Database: ~/Pane/.pane/pane.db");
            println!("✓ Projects: ~/Pane/Projects/");


            // §24.1: SYNCHRONOUS agent integrity check (must complete before UI shows)
            // This is fast — just reads a few folders from .pane/system/
            // Running it async caused a race condition where onboarding would call get_main_agent
            // before agents were discovered, creating phantom "My Agent" folders
            {
                let db_for_integrity = app.state::<DbState>();
                let conn_guard = db_for_integrity.0.lock();
                if let Ok(conn) = conn_guard {
                    let has_agents_table: bool = conn.query_row(
                        "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='ai_agents'",
                        [],
                        |row| row.get(0),
                    ).unwrap_or(false);

                    // §24: ai_agents table is created by migration_009 — always exists on fresh DB
                    // legacy system notes function removed — no pre-§24 databases exist
                    if has_agents_table {
                        // BUG 7 FIX: Skip integrity check if this is a fresh onboarding
                        // Check if DB has agents OR if .pane/system/ directory exists
                        let agent_count: i64 = conn.query_row(
                            "SELECT COUNT(*) FROM ai_agents WHERE is_active = 1",
                            [],
                            |row| row.get(0),
                        ).unwrap_or(0);

                        let home = std::env::var("HOME").unwrap_or_default();
                        let system_dir = std::path::PathBuf::from(&home).join("Pane").join(".pane").join("system");
                        let has_system_files = system_dir.exists() &&
                            std::fs::read_dir(&system_dir).map(|mut d| d.next().is_some()).unwrap_or(false);

                        // Only run integrity check if we have agents OR system files already exist
                        // Skip on fresh install to avoid creating phantom agents during onboarding
                        if agent_count > 0 || has_system_files {
                            ai::ingest::verify_agents_integrity(&conn);
                        } else {
                            eprintln!("[startup] Skipping integrity check - fresh install detected");
                        }
                    }

                    // Clean up legacy Pane System project (§24 — agents managed via ai_agents table, not projects)
                    conn.execute(
                        "DELETE FROM projects WHERE name = 'Pane System' AND is_system = 1",
                        [],
                    ).ok();
                    // Also clean up any orphaned sub-projects that were children of Pane System
                    conn.execute(
                        "DELETE FROM projects WHERE parent_id NOT IN (SELECT id FROM projects) AND is_system = 1",
                        [],
                    ).ok();
                }
            }

            // ASYNC: FTS rebuild can happen in background (slow, UI doesn't need it immediately)
            // INTEGRITY_DONE signals the watcher to start processing
            let db_for_fts = app.state::<DbState>();
            let fts_conn = db_for_fts.0.clone();
            // Clone the app handle so the embedding thread can emit `embeddings://status` events (#637)
            let embed_app_handle = app.clone();
            std::thread::spawn(move || {
                use tauri::Emitter;
                // SCOPE 1: FTS rebuild + cleanup (must complete before UI renders)
                // These operations are quick and require data integrity
                let embeddings_enabled = {
                    let conn_guard = fts_conn.lock();
                    if conn_guard.is_err() {
                        eprintln!("[startup] Failed to lock DB for FTS rebuild");
                        watcher::INTEGRITY_DONE.store(true, std::sync::atomic::Ordering::Release);
                        return;
                    }
                    let conn = conn_guard.unwrap();

                    // notes_fts drift gate (#3072) — see `run_notes_fts_drift_gate`'s
                    // doc comment for the full rationale (replaces the unconditional
                    // DELETE+disk-read-loop that produced #2352/#2566/#2610's
                    // "malformed" reports). Extracted to a standalone function so it's
                    // unit-testable without the Tauri setup closure around it.
                    let _drift_outcome = run_notes_fts_drift_gate(&conn);

                    // Rebuild other FTS indexes
                    if let Err(e) = conn.execute("INSERT INTO ai_memory_fts(ai_memory_fts) VALUES('rebuild')", []) {
                        eprintln!("[STARTUP-ERROR] Failed to rebuild ai_memory_fts: {}", e);
                    }
                    if let Err(e) = conn.execute("INSERT INTO chat_fts(chat_fts) VALUES('rebuild')", []) {
                        eprintln!("[STARTUP-ERROR] Failed to rebuild chat_fts: {}", e);
                    }
                    eprintln!("[FTS] Rebuilt ai_memory_fts and chat_fts indexes");

                    // === Memory Decay (v11.26.0) ===
                    // Run decay check weekly — check if last decay was >7 days ago
                    let last_decay: Option<String> = database::get_ai_setting(&conn, "last_decay_check");

                    let should_decay = match last_decay {
                        None => true,
                        Some(ref d) => {
                            chrono::NaiveDateTime::parse_from_str(d, "%Y-%m-%dT%H:%M:%S%.fZ")
                                .or_else(|_| chrono::NaiveDateTime::parse_from_str(d, "%Y-%m-%dT%H:%M:%S%.f%:z"))
                                .or_else(|_| chrono::NaiveDateTime::parse_from_str(d, "%Y-%m-%d %H:%M:%S"))
                                .map(|dt| chrono::Utc::now().naive_utc() - dt > chrono::Duration::days(7))
                                .unwrap_or(true)
                        }
                    };

                    if should_decay {
                        crate::ai::scheduler::run_memory_decay(&conn);
                        if let Err(e) = database::set_ai_setting(&conn, "last_decay_check", &chrono::Utc::now().to_rfc3339()) {
                            eprintln!("[STARTUP-ERROR] Failed to update last_decay_check: {}", e);
                        }
                    }

                    // === Compaction Fact Cleanup (v11.28.21) ===
                    // Deactivate facts that are compaction summaries (one-time + ongoing safety net)
                    let compaction_cleaned: usize = conn.execute(
                        "UPDATE ai_memory SET is_active = 0 WHERE category = 'compaction_summary' AND is_active = 1",
                        [],
                    ).unwrap_or(0);
                    // Also catch compaction facts that were miscategorized
                    let compaction_cleaned2: usize = conn.execute(
                        "UPDATE ai_memory SET is_active = 0 WHERE is_active = 1 AND length(fact) > 500 AND (fact LIKE '%Conversation Summary%' OR fact LIKE '%Key Decisions%' OR fact LIKE '%Current Status%' OR fact LIKE '%Content Added%')",
                        [],
                    ).unwrap_or(0);
                    if compaction_cleaned + compaction_cleaned2 > 0 {
                        eprintln!("[CLEANUP] Deactivated {} compaction summary facts ({} by category, {} by content)", compaction_cleaned + compaction_cleaned2, compaction_cleaned, compaction_cleaned2);
                    }

                    // === v11.28.22: One-time cleanup — wipe polluted ai_memory for clean rebuild ===
                    {
                        let cleanup_key = "cleanup_v11_28_22_done";
                        let already_cleaned: bool = conn.query_row(
                            "SELECT COUNT(*) > 0 FROM ai_settings WHERE key = ?1",
                            rusqlite::params![cleanup_key],
                            |row| row.get::<_, bool>(0),
                        ).unwrap_or(false);

                        if !already_cleaned {
                            let deleted = conn.execute("DELETE FROM ai_memory WHERE 1=1", []).unwrap_or(0);
                            eprintln!("[CLEANUP] v11.28.22: Wiped {} polluted facts from ai_memory for clean rebuild", deleted);
                            if let Err(e) = conn.execute(
                                "INSERT INTO ai_settings (key, value) VALUES (?1, '1')",
                                rusqlite::params![cleanup_key],
                            ) {
                                eprintln!("[STARTUP-ERROR] Failed to record cleanup_v11_28_22_done: {}", e);
                            }
                        }
                    }

                    // === Archive Migration (§5.14.2) ===
                    // Move messages >30 days old to chat_archive (runs weekly)
                    archive::run_archive_migration(&conn);

                    // === Embeddings Table Setup ===
                    if let Err(e) = embeddings::create_embeddings_table(&conn) {
                        eprintln!("[STARTUP-ERROR] Failed to create embeddings table: {}", e);
                    }

                    // Embeddings are opt-in only (ADR-0048 / §5.9).
                    // Read explicit user opt-in from DB; if absent, default to disabled.
                    let embeddings_enabled: bool = database::get_ai_setting(&conn, "embeddings_enabled")
                        .map(|v| v == "true")
                        .unwrap_or(false);

                    eprintln!("[EMBEDDINGS] embeddings_enabled = {} (from DB)", embeddings_enabled);

                    embeddings_enabled
                }; // ← Lock dropped here, UI can now render

                // Signal that integrity operations are complete — UI can proceed
                watcher::INTEGRITY_DONE.store(true, std::sync::atomic::Ordering::Release);
                eprintln!("[startup] Integrity check complete, UI unblocked");

                // SCOPE 2: Embeddings pipeline (ADR-0048 / §5.9)
                // Semantic search is opt-in. Requires embeddings_enabled == "true" AND
                // a configured engine (openrouter or ollama) with valid credentials.
                if embeddings_enabled {
                    // Read engine + credentials from DB
                    let db_path = crate::database::get_db_path();
                    let cred_conn = match rusqlite::Connection::open(&db_path) {
                        Ok(c) => c,
                        Err(e) => {
                            eprintln!("[EMBEDDINGS] Failed to open DB for engine config: {}", e);
                            let _ = embed_app_handle.emit("embeddings://status", serde_json::json!({
                                "stage": "error",
                                "reason": format!("db_open_failed: {}", e)
                            }));
                            return;
                        }
                    };

                    let embeddings_engine = crate::database::get_ai_setting(&cred_conn, "embeddings_engine")
                        .unwrap_or_default();
                    let embeddings_model = crate::database::get_ai_setting(&cred_conn, "embeddings_model")
                        .unwrap_or_default();

                    if embeddings_engine.is_empty() || embeddings_model.is_empty() {
                        eprintln!("[EMBEDDINGS] Engine or model not configured — skipping pipeline");
                        let _ = embed_app_handle.emit("embeddings://status", serde_json::json!({
                            "stage": "not_configured",
                            "reason": "no_engine_or_model"
                        }));
                        return;
                    }

                    // #2776: read the persisted token limit (fallback 512 for installs
                    // that configured their embedding engine/model before this setting
                    // existed — read_embeddings_max_tokens logs the fallback explicitly).
                    let embeddings_max_tokens = embeddings::read_embeddings_max_tokens(&cred_conn);

                    eprintln!("[EMBEDDINGS] Starting pipeline: engine={} model={} max_tokens={}", embeddings_engine, embeddings_model, embeddings_max_tokens);

                    let embed_fn_result = crate::build_http_embed_fn(&cred_conn, &embeddings_engine, &embeddings_model);
                    drop(cred_conn);

                    let (embed_fn, engine_tag) = match embed_fn_result {
                        Ok(pair) => pair,
                        Err(e) => {
                            eprintln!("[EMBEDDINGS] Failed to build embed_fn: {}", e);
                            let _ = embed_app_handle.emit("embeddings://status", serde_json::json!({
                                "stage": "not_configured",
                                "reason": e
                            }));
                            return;
                        }
                    };

                    let _ = embed_app_handle.emit("embeddings://status", serde_json::json!({
                        "stage": "indexing"
                    }));

                    let pipeline_conn = match rusqlite::Connection::open(&db_path) {
                        Ok(c) => c,
                        Err(e) => {
                            eprintln!("[EMBEDDINGS] Failed to open DB for pipeline: {}", e);
                            let _ = embed_app_handle.emit("embeddings://status", serde_json::json!({
                                "stage": "error",
                                "reason": format!("pipeline_db_open_failed: {}", e)
                            }));
                            return;
                        }
                    };

                    let pane_root = crate::database::get_pane_root();
                    let embed_app_handle_inner = embed_app_handle.clone();

                    let pipeline_result = embeddings::run_embedding_pipeline(
                        &pipeline_conn,
                        &pane_root,
                        &embed_fn,
                        &engine_tag,
                        &embeddings_model,
                        embeddings_max_tokens,
                        move |progress| {
                            let embeddings::EmbeddingProgress::Indexing { notes, facts, chats, notes_total, facts_total, chats_total } = &progress;
                            let _ = embed_app_handle_inner.emit("embeddings://status", serde_json::json!({
                                "stage": "indexing",
                                "notes": notes,
                                "facts": facts,
                                "chats": chats,
                                "notes_total": notes_total,
                                "facts_total": facts_total,
                                "chats_total": chats_total
                            }));
                        },
                    );

                    match pipeline_result {
                        Ok((notes, facts, chats, failed_count)) => {
                            eprintln!("[EMBEDDINGS] Pipeline complete: {} notes, {} facts, {} chats, {} failed", notes, facts, chats, failed_count);
                            let terminal_stage = if failed_count > 0 { "partial" } else { "ready" };
                            let db_path2 = crate::database::get_db_path();
                            if let Ok(status_conn) = rusqlite::Connection::open(&db_path2) {
                                let _ = crate::database::set_ai_setting(&status_conn, "embeddings_status", terminal_stage);
                                let _ = crate::database::set_ai_setting(&status_conn, "embeddings_failed_count", &failed_count.to_string());
                            }
                            let _ = embed_app_handle.emit("embeddings://status", serde_json::json!({
                                "stage": terminal_stage
                            }));
                        }
                        Err(e) => {
                            eprintln!("[EMBEDDINGS] Pipeline error: {}", e);
                            let db_path2 = crate::database::get_db_path();
                            if let Ok(status_conn) = rusqlite::Connection::open(&db_path2) {
                                let _ = crate::database::set_ai_setting(&status_conn, "embeddings_status", "error");
                                // Do not write embeddings_failed_count on auth abort — leave prior state intact
                            }
                            let _ = embed_app_handle.emit("embeddings://status", serde_json::json!({
                                "stage": "error",
                                "reason": e
                            }));
                        }
                    }
                } else {
                    // Emit disabled — not opted in (§3 coarse stage, direct emit)
                    let _ = embed_app_handle.emit("embeddings://status", serde_json::json!({
                        "stage": "disabled",
                        "reason": "not_opted_in"
                    }));
                }
            });

            // === Consolidation Staleness Check (#671, §5.5.3) ===
            // Runs async after 15s delay — provider config must be initialized first
            {
                let consolidation_handle = app.clone();
                tauri::async_runtime::spawn(async move {
                    // Poll for provider readiness: 2s interval, 30s ceiling (15 iterations).
                    // #2522 ADR-0063 Stage 2: readiness now means "the designated default
                    // agent (D5) resolves to a runtime config with a dispatchable provider",
                    // resolved fresh from the DB each poll — there is no more in-memory
                    // AiStateWrapper to become ready; startup credential migration racing
                    // this thread is the actual thing being waited out.
                    let provider_ready = {
                        let mut ready = false;
                        for _ in 0..15 {
                            let handle_for_poll = consolidation_handle.clone();
                            let provider_ok = tokio::task::spawn_blocking(move || {
                                let db_state = handle_for_poll.state::<database::DbState>();
                                let conn = db_state.0.lock().unwrap();
                                matches!(
                                    ai::runtime_resolution::resolve_runtime_config_for_default_agent(&conn)
                                        .map(|cfg| cfg.provider),
                                    Ok(AiProvider::OpenRouter { .. })
                                        | Ok(AiProvider::Anthropic { .. })
                                        | Ok(AiProvider::OpenClaw { .. })
                                )
                            })
                            .await
                            .unwrap_or(false);
                            if provider_ok {
                                ready = true;
                                break;
                            }
                            tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
                        }
                        ready
                    };
                    if !provider_ready {
                        eprintln!("[CONSOLIDATOR] Provider not ready after 30s, skipping startup consolidation");
                        return;
                    }

                    // Check staleness in a blocking thread (Connection is not Send)
                    let handle_for_check = consolidation_handle.clone();
                    let targets = tokio::task::spawn_blocking(move || {
                        let db_state = handle_for_check.state::<database::DbState>();
                        let conn = db_state.0.lock().unwrap();
                        ai::consolidator::check_consolidation_staleness(&*conn)
                    }).await.unwrap_or_default();

                    if targets.is_empty() {
                        return;
                    }
                    eprintln!("[CONSOLIDATOR] Startup: {} file(s) need consolidation", targets.len());

                    let db_state = consolidation_handle.state::<database::DbState>();

                    let (provider_name, api_key, model, endpoint, oc_runtime_ref) = {
                        let conn = db_state.0.lock().unwrap();
                        let cfg = match ai::runtime_resolution::resolve_runtime_config_for_default_agent(&conn) {
                            Ok(cfg) => cfg,
                            Err(e) => {
                                eprintln!("[CONSOLIDATOR] No runtime configured for startup consolidation: {}", e);
                                return;
                            }
                        };
                        let (name, key, ep) = match &cfg.provider {
                            AiProvider::OpenRouter { api_key } => ("openrouter", api_key.clone(), None),
                            AiProvider::Anthropic { api_key } => ("anthropic", api_key.clone(), None),
                            AiProvider::OpenClaw { base_url, token, .. } => ("openclaw", token.clone(), Some(base_url.clone())),
                            AiProvider::Venice { api_key, .. } => ("venice", api_key.clone(), None),
                            _ => {
                                eprintln!("[CONSOLIDATOR] Unsupported provider for startup consolidation");
                                return;
                            }
                        };
                        (name.to_string(), key, cfg.model.clone(), ep, cfg.runtime_ref.clone())
                    };

                    for target in targets {
                        // Resolve relative path from notes table against pane_root (ADR-0017 #795)
                        let absolute_path = crate::database::get_pane_root().join(&target.file_path);
                        eprintln!("[CONSOLIDATOR] Startup: consolidating {}", absolute_path.display());
                        if let Err(e) = ai::consolidator::consolidate_file(
                            &*db_state,
                            &consolidation_handle,
                            &absolute_path,
                            target.project_id.as_deref(),
                            &provider_name,
                            &api_key,
                            &model,
                            endpoint.as_deref(),
                            oc_runtime_ref.as_deref(),
                        ).await {
                            eprintln!("[CONSOLIDATOR] Startup failed for {}: {}", absolute_path.display(), e);
                        }
                    }
                });
            }

            // Pull pending changes from gateway + flush sync queue on startup (after 10s delay)
            // Uses tauri::async_runtime::spawn with spawn_blocking to avoid nested runtime panic (#324)
            // MUST NOT crash app if gateway unreachable or sync fails — graceful degradation
            let startup_app_handle = app.clone();
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(tokio::time::Duration::from_secs(10)).await;

                // Move DB work to a blocking thread (Connection is not Send)
                // Wrap entire sync in panic catch — failures should log, not crash
                let app_handle_for_sync = startup_app_handle.clone();
                let _ = tokio::task::spawn_blocking(move || {
                    let sync_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        match database::open_connection(None) {
                            Ok(conn) => {
                                // Get DbState from app handle
                                let db_state = app_handle_for_sync.state::<DbState>();

                                // ADR-0040 Decision 7: one-shot migration to fix stub files
                                // created by the old hardcoded workspace path placeholder.
                                // Runs only when: provider=openclaw, split_machine, flag unset.
                                if sync_client::should_run_workspace_migration(&conn) {
                                    eprintln!("[STARTUP] ADR-0040 workspace migration conditions met — running migration");
                                    if let Err(e) = sync_client::run_workspace_migration(&conn) {
                                        eprintln!("[STARTUP] Workspace migration failed (non-fatal): {}", e);
                                    }
                                } else {
                                    eprintln!("[STARTUP] ADR-0040 workspace migration: conditions not met or already complete — skipped");
                                }

                                // Use blocking variants (no nested runtime)
                                if let Err(e) = sync_client::pull_pending_changes_blocking(&conn, Some(&app_handle_for_sync), &db_state) {
                                    eprintln!("[SYNC] Startup pull failed (non-fatal): {}", e);
                                }
                                if let Err(e) = sync_client::flush_sync_queue_blocking(&conn, Some(&app_handle_for_sync), &db_state) {
                                    eprintln!("[SYNC] Startup flush failed (non-fatal): {}", e);
                                }
                            }
                            Err(e) => eprintln!("[SYNC] Failed to open DB (non-fatal): {}", e),
                        }
                    }));

                    if let Err(e) = sync_result {
                        eprintln!("[SYNC] Startup sync panicked (caught, non-fatal): {:?}", e);
                    }
                }).await;
            });

            // Periodic pull + flush (every 5 minutes) — fix #1370
            // Replaces the old periodic-flush-only thread. pull_pending_changes_blocking
            // internally calls flush_sync_queue_blocking at sync_client.rs:838, so
            // this single thread covers both inbound pull AND outbound retry in one pass.
            // Pull subsumes flush — one thread, not two.
            //
            // Fix B (#1180): fresh DB connection per iteration (never share across threads).
            // OS thread (not tokio) — freeze-cluster lessons from #1180 mandate this.
            // See: docs/audits/2026-06-02-freeze-cluster-1347-1180.md
            let db_for_sync_periodic = app.state::<DbState>();
            let periodic_app_handle = app.clone();
            let periodic_db_state = DbState(db_for_sync_periodic.0.clone());
            std::thread::spawn(move || {
                loop {
                    std::thread::sleep(std::time::Duration::from_secs(300)); // 5 minutes
                    eprintln!("[PERIODIC_PULL] Starting periodic pull+flush cycle...");
                    match crate::database::open_connection(None) {
                        Ok(conn) => {
                            let result = sync_client::pull_pending_changes_blocking(&conn, Some(&periodic_app_handle), &periodic_db_state);
                            eprintln!("[PERIODIC_PULL] Cycle complete: {:?}", result);
                            if let Err(e) = result {
                                eprintln!("[PERIODIC_PULL] Periodic pull failed (non-fatal): {}", e);
                            }
                        }
                        Err(e) => {
                            eprintln!("[PERIODIC_PULL] Failed to open fresh connection: {}", e);
                        }
                    }
                }
            });

            // Start notification watcher
            let db_for_notifications = app.state::<DbState>();
            notifications::start_notification_watcher(app.clone(), database::DbState(db_for_notifications.0.clone()));
            notifications::start_supabase_notification_poller(app.clone(), database::DbState(db_for_notifications.0.clone()));

            // PR3b: One-shot 5-min startup due-date check (§19.5.1, ADR-0021)
            // Not a loop — fires once, exits. Handles "user opens Pane after 8h closed."
            {
                let handle_for_due = app.clone();
                let db_for_due = database::DbState(db_for_notifications.0.clone());
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_secs(300)); // 5 min
                    notifications::check_due_dates(&handle_for_due, &db_for_due);
                    // thread exits — no loop, no interval
                });
            }

            // §25: Daily backup in background (non-blocking) — issue #353
            // Runs after window renders instead of during database init
            std::thread::spawn(move || {
                let db_path = crate::database::get_db_path();
                match crate::backup::create_daily_backup(&db_path) {
                    Ok(path) => eprintln!("[BACKUP] Daily backup created: {}", path.display()),
                    Err(e) if e.contains("Already have a backup") => {
                        eprintln!("[BACKUP] Skipped — already backed up today");
                    },
                    Err(e) => eprintln!("[BACKUP] Daily backup failed: {}", e),
                }
            });

            // Start file watcher in background thread
            let app_handle_for_watcher = app.clone();
            std::thread::spawn(move || {
                if let Err(e) = watcher::start_file_watcher(app_handle_for_watcher) {
                    eprintln!("[watcher] Failed to start: {}", e);
                }
            });

        Ok(())
    })
}

// ─── #2339 PR 4: lib.rs:716 integrity-sweep allowlist test (§33.1.1) ─────────
#[cfg(test)]
mod integrity_sweep_allowlist_tests {
    use rusqlite::Connection;

    /// A `note_type = 'sheet'` note with `body IS NULL` must be excluded
    /// from the §486 integrity-sweep count at lib.rs:716 — the allowlist
    /// entry added for #2339 PR 4 means sheets are expected to legitimately
    /// have no on-disk file to repair `body` from (fileless, §33.2.1), so
    /// they must never be counted as a malformed row needing repair.
    #[test]
    fn test_null_body_sheet_note_excluded_from_integrity_sweep_count() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE notes (
                 id TEXT PRIMARY KEY,
                 body TEXT,
                 file_path TEXT NOT NULL DEFAULT '',
                 note_type TEXT,
                 deleted_at TEXT
             );",
        )
        .unwrap();

        conn.execute(
            "INSERT INTO notes (id, body, file_path, note_type) VALUES ('sheet-1', NULL, '', 'sheet')",
            [],
        )
        .unwrap();

        // Same SQL string as lib.rs's §486 count query (post-#2339-PR-4 allowlist).
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM notes WHERE body IS NULL AND deleted_at IS NULL AND (note_type IS NULL OR note_type NOT IN ('soul','memory','rules','identity','user','tools','agents','heartbeat','daily','sheet'))",
                [],
                |row| row.get(0),
            )
            .unwrap();

        assert_eq!(count, 0, "a NULL-body sheet note must be excluded from the integrity sweep count");
    }

    /// Regression guard: a NULL-body note with a note_type NOT on the
    /// allowlist (e.g. a plain task/cabinet note, note_type IS NULL) must
    /// still be counted — the allowlist addition must not silently widen
    /// past 'sheet'.
    #[test]
    fn test_null_body_non_allowlisted_note_still_counted() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE notes (
                 id TEXT PRIMARY KEY,
                 body TEXT,
                 file_path TEXT NOT NULL DEFAULT '',
                 note_type TEXT,
                 deleted_at TEXT
             );",
        )
        .unwrap();

        conn.execute(
            "INSERT INTO notes (id, body, file_path, note_type) VALUES ('task-1', NULL, 'Projects/Proj/task-1.md', NULL)",
            [],
        )
        .unwrap();

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM notes WHERE body IS NULL AND deleted_at IS NULL AND (note_type IS NULL OR note_type NOT IN ('soul','memory','rules','identity','user','tools','agents','heartbeat','daily','sheet'))",
                [],
                |row| row.get(0),
            )
            .unwrap();

        assert_eq!(count, 1, "a plain NULL-body, NULL-note_type note must still be counted");
    }
}

/// Tests for `run_notes_fts_drift_gate` (#3072) — the replacement for the
/// unconditional DELETE+disk-read-loop that produced #2352/#2566/#2610's
/// "malformed" reports. Uses the real `notes_fts` external-content schema +
/// triggers (same DDL as `migrations.rs::create_fts_virtual_tables` /
/// `create_fts_triggers`) and creates notes through the real
/// `notes::create_note_internal` command (workshop-scope project id, which
/// is DB-only — #24.10.1e — so no on-disk file / `projects` row is needed),
/// never a raw `INSERT INTO notes`. Simulating *drift* (not user actions) by
/// directly deleting a `notes_fts` row is the one place raw SQL is used —
/// that's the test harness breaking the fixture on purpose to exercise the
/// repair path, not a substitute for a user action (see #3072 scope's test
/// guidance).
#[cfg(test)]
mod notes_fts_drift_gate_tests {
    use super::*;
    use crate::notes::{create_note_internal, CreateNoteRequest, WORKSHOP_SCOPE_ID};
    use rusqlite::Connection;

    /// Real `notes` + `notes_fts` (external-content) + triggers, matching
    /// `migrations.rs::create_notes_table` / `create_fts_virtual_tables` /
    /// `create_fts_triggers` exactly, plus `ai_settings` for the status keys.
    /// No `projects` table — workshop-scope notes never query it.
    fn create_fixture_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "
            CREATE TABLE notes (
                id              TEXT NOT NULL PRIMARY KEY,
                title           TEXT NOT NULL DEFAULT '',
                body            TEXT,
                content_preview TEXT NOT NULL DEFAULT '',
                file_path       TEXT NOT NULL DEFAULT '',
                project_id      TEXT,
                folder_id       TEXT,
                status          TEXT NOT NULL DEFAULT 'cabinet',
                priority        TEXT,
                due_date        TEXT,
                assignee        TEXT,
                completed_at    TEXT,
                sort_order      INTEGER NOT NULL DEFAULT 0,
                word_count      INTEGER NOT NULL DEFAULT 0,
                pinned          INTEGER NOT NULL DEFAULT 0,
                owner_id        TEXT,
                tags_json       TEXT NOT NULL DEFAULT '[]',
                created_at      TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at      TEXT NOT NULL DEFAULT (datetime('now')),
                deleted_at      TEXT,
                parent_id       TEXT,
                note_type       TEXT,
                is_vital        INTEGER NOT NULL DEFAULT 0,
                agent_id        TEXT
            );

            CREATE TABLE ai_settings (
                key   TEXT PRIMARY KEY NOT NULL,
                value TEXT NOT NULL
            );

            CREATE VIRTUAL TABLE notes_fts USING fts5(
                title,
                body,
                content='notes',
                content_rowid='rowid',
                tokenize='porter unicode61'
            );

            CREATE TRIGGER notes_fts_insert AFTER INSERT ON notes BEGIN
                INSERT INTO notes_fts(rowid, title, body) VALUES (new.rowid, new.title, new.body);
            END;

            CREATE TRIGGER notes_fts_delete AFTER DELETE ON notes BEGIN
                INSERT INTO notes_fts(notes_fts, rowid, title, body) VALUES('delete', old.rowid, old.title, old.body);
            END;

            CREATE TRIGGER notes_fts_update AFTER UPDATE ON notes BEGIN
                INSERT INTO notes_fts(notes_fts, rowid, title, body) VALUES('delete', old.rowid, old.title, old.body);
                INSERT INTO notes_fts(rowid, title, body) VALUES (new.rowid, new.title, new.body);
            END;
            ",
        )
        .unwrap();
        conn
    }

    /// Create a note through the real command (never a raw `INSERT INTO
    /// notes`) — workshop scope is DB-only, so this needs no filesystem and
    /// no `projects` row.
    fn create_real_note(conn: &Connection, title: &str, body: &str) -> String {
        let request = CreateNoteRequest {
            project_id: WORKSHOP_SCOPE_ID.to_string(),
            folder_id: None,
            title: title.to_string(),
            content: Some(body.to_string()),
            tags: None,
            // The real `notes.status` column is NOT NULL DEFAULT 'cabinet',
            // but `create_note_internal`'s INSERT always supplies the column
            // explicitly, so an explicit NULL binding (request.status: None)
            // doesn't fall back to the column default. Pass 'cabinet'
            // explicitly, same as a user creating a plain note would get.
            status: Some("cabinet".to_string()),
            priority: None,
            due_date: None,
            parent_id: None,
        };
        create_note_internal(conn, request).unwrap().id
    }

    fn fts_finds(conn: &Connection, needle: &str) -> bool {
        conn.query_row(
            "SELECT COUNT(*) > 0 FROM notes_fts WHERE notes_fts MATCH ?1",
            rusqlite::params![needle],
            |row| row.get(0),
        )
        .unwrap()
    }

    #[test]
    fn clean_index_is_not_rebuilt_and_is_marked_ok() {
        // Pins "startup does NOT reindex when no rebuild is warranted" — the
        // exact behavior being removed (#3072), and the easiest thing to
        // silently regress back in.
        let conn = create_fixture_db();
        create_real_note(&conn, "Alpha", "alpha needle content");
        create_real_note(&conn, "Beta", "beta needle content");

        assert!(fts_finds(&conn, "needle"), "sanity: triggers must have indexed both notes already");

        let outcome = run_notes_fts_drift_gate(&conn);

        assert_eq!(outcome, NotesFtsDriftOutcome::Clean, "a healthy index must not trigger a rebuild");
        assert_eq!(
            database::get_ai_setting(&conn, "notes_fts_rebuild_status").as_deref(),
            Some("ok"),
            "a clean check must still record a status (distinct from 'never ran')"
        );
        assert!(fts_finds(&conn, "needle"), "index must still be queryable and correct after a clean check");
    }

    #[test]
    fn drifted_index_is_rebuilt_transactionally_and_marked_ok() {
        // Pins "a rebuild leaves the index consistent with notes, with no
        // observable partial state" — simulate drift, confirm full repair
        // (not just the one broken row) with no empty-index window exposed
        // to this test (the transaction is opaque to callers: either the old,
        // drifted state or the fully-rebuilt state is visible, never partial).
        let conn = create_fixture_db();
        create_real_note(&conn, "Alpha", "alpha needle content");
        create_real_note(&conn, "Beta", "beta needle content");
        create_real_note(&conn, "Gamma", "gamma needle content");

        // Test-harness-only fault injection: break the shadow index directly
        // to simulate the drift an external-content table can suffer, NOT a
        // substitute for a user action (see module doc comment).
        conn.execute("DELETE FROM notes_fts WHERE rowid = 1", []).unwrap();
        assert!(!fts_finds(&conn, "alpha"), "sanity: drift must have actually broken lookup for the deleted row");

        let outcome = run_notes_fts_drift_gate(&conn);

        assert_eq!(outcome, NotesFtsDriftOutcome::Rebuilt, "detected drift must trigger a rebuild");
        assert_eq!(
            database::get_ai_setting(&conn, "notes_fts_rebuild_status").as_deref(),
            Some("ok"),
        );
        assert!(fts_finds(&conn, "alpha"), "rebuild must restore the row that was directly broken");
        assert!(fts_finds(&conn, "beta"), "rebuild must not have missed unrelated rows");
        assert!(fts_finds(&conn, "gamma"), "rebuild must not have missed unrelated rows");
    }

    #[test]
    fn failed_rebuild_is_recorded_as_failed_not_absent() {
        // Pins "a failed rebuild surfaces distinctly from one that never
        // ran" (#3072 scope item 4 / "empty is not zero").
        let conn = create_fixture_db();
        create_real_note(&conn, "Alpha", "alpha needle content");

        // Force BOTH the integrity-check and the rebuild to fail by removing
        // the content table notes_fts depends on — fault injection on the
        // fixture, not a raw-INSERT substitute for a user action.
        conn.execute_batch("DROP TABLE notes;").unwrap();

        let outcome = run_notes_fts_drift_gate(&conn);

        assert_eq!(outcome, NotesFtsDriftOutcome::RebuildFailed);
        assert_eq!(
            database::get_ai_setting(&conn, "notes_fts_rebuild_status").as_deref(),
            Some("failed"),
            "a failed rebuild must be durably recorded as failed, not silently swallowed"
        );
    }

    #[test]
    fn never_ran_is_absent_not_failed_and_not_ok() {
        // Pins the literal "empty is not zero" assertion: a DB on which this
        // gate has never executed must have NO status key at all — not
        // "failed", not "ok". Absence is itself meaningful.
        let conn = create_fixture_db();
        create_real_note(&conn, "Alpha", "alpha needle content");
        // Deliberately never call run_notes_fts_drift_gate.

        assert_eq!(
            database::get_ai_setting(&conn, "notes_fts_rebuild_status"),
            None,
            "a DB that never ran the drift gate must have no status key — absence must not read as 'ok' or 'failed'"
        );
    }
}
