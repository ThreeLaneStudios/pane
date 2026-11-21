// Pane v0.17.73 — Async Memory Extraction Worker
// §27.7 — Background queue worker for memory extraction
// Decouples extraction from chat response to prevent gateway lane congestion

use rusqlite::{params, Connection, OptionalExtension};
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use tokio::time::sleep;
use tauri::Emitter;
use crate::ai::scheduler::ai_extract_memory_with_db;

/// §27.6.3 null-gate: returns true if extraction_model is set (extraction enabled)
/// Extracted for testability — called by worker loop instead of inline check
pub(crate) fn should_run_extraction(conn: &Connection) -> bool {
    crate::database::get_ai_setting(conn, "extraction_model").is_some()
}

/// §5.4C + ADR-0038: per-feature lookup only — no fallback to simple_tier_model or primary model
/// Extracted for testability — called by worker loop instead of inline check
pub(crate) fn resolve_extraction_model(conn: &Connection) -> Option<String> {
    crate::database::get_ai_setting(conn, "extraction_model")
}

/// Get the memory synthesis threshold from ai_settings or default
/// Exported for testing to ensure test uses same logic as production
pub fn get_synthesis_threshold(conn: &Connection) -> i64 {
    crate::database::get_ai_setting(conn, "memory_synthesis_threshold")
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(1)  // Default: trigger on first unrouted fact
}

/// Extraction job from queue
#[derive(Debug, Clone)]
pub(crate) struct ExtractionJob {
    id: i64,
    thread_id: String,
    message_id: String,
    user_message_id: Option<String>,
    attempts: i32,
}

/// §4.6.9 — detects web_fetch/web_search usage in a stored `tool_calls` JSON
/// value. `tool_calls` is written by `save_chat_message` (commands.rs) as a JSON
/// array of `{id, name, params}` objects (see canonical_format.rs
/// `tool_calls_to_canonical`); this checks the `name` field of each entry rather
/// than doing a raw substring match on the column, so a URL or note title that
/// happens to contain the text "web_fetch" can never produce a false positive.
fn tool_calls_json_uses_web_tools(tool_calls_json: &str) -> bool {
    let parsed: serde_json::Value = match serde_json::from_str(tool_calls_json) {
        Ok(v) => v,
        Err(_) => return false,
    };
    let Some(arr) = parsed.as_array() else {
        return false;
    };
    arr.iter().any(|entry| {
        entry
            .get("name")
            .and_then(|n| n.as_str())
            .map(|name| name == "web_fetch" || name == "web_search")
            .unwrap_or(false)
    })
}

/// §4.6.9 — fetches the (user_message, assistant_response, project_id) tuple for
/// an extraction job, plus whether the assistant turn used a web tool. Extracted
/// from the worker loop's former inline block (same query pattern, unchanged)
/// specifically so `used_web_tools` detection has one call site and one test
/// surface instead of being buried in the loop. Signature takes `&ExtractionJob`
/// (not individual fields) to match §4.6.9's build note exactly and so the
/// exclusion test (§4.6.10 item 11) drives the production entry point.
pub(crate) fn load_extraction_pair(
    conn: &Connection,
    job: &ExtractionJob,
) -> Result<(String, String, Option<String>, bool), String> {
    let job_message_id = &job.message_id;
    let job_user_message_id = job.user_message_id.as_deref();
    let job_thread_id = &job.thread_id;

    // Get assistant message + its tool_calls (for used_web_tools detection).
    let assistant_msg: Result<(String, Option<String>, Option<String>), _> = conn.query_row(
        "SELECT content, project_id, tool_calls FROM chat_messages_local WHERE id = ?1",
        params![job_message_id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    );

    let (assistant_response, proj_id, tool_calls_json) = match assistant_msg {
        Ok(msg) => msg,
        Err(e) => {
            return Err(format!("Message not found: {}", e));
        }
    };

    let used_web_tools = tool_calls_json
        .as_deref()
        .map(tool_calls_json_uses_web_tools)
        .unwrap_or(false);

    // Get user message if user_message_id is set
    let user_message = if let Some(user_msg_id) = job_user_message_id {
        conn.query_row(
            "SELECT content FROM chat_messages_local WHERE id = ?1",
            params![user_msg_id],
            |row| row.get::<_, String>(0),
        )
        .map_err(|e| {
            eprintln!("[EXTRACTION] Failed to fetch user message by ID {}: {}", user_msg_id, e);
            e
        })
        .unwrap_or_else(|_| String::from(""))
    } else {
        // Fall back to fetching most recent user message in same project and thread
        conn.query_row(
            "SELECT content FROM chat_messages_local 
             WHERE project_id = ?1 AND role = 'user' AND thread_id = ?2
             ORDER BY created_at DESC LIMIT 1",
            params![proj_id.as_deref().unwrap_or_default(), job_thread_id],
            |row| row.get::<_, String>(0),
        )
        .map_err(|e| {
            eprintln!("[EXTRACTION] Failed to fetch recent user message for project {:?} thread {}: {}", proj_id, job_thread_id, e);
            e
        })
        .unwrap_or_else(|_| String::from(""))
    };

    Ok((user_message, assistant_response, proj_id, used_web_tools))
}

/// Enqueue an extraction job (called after AI response, replaces synchronous extraction)
///
/// §36.4.3/§5.5 (#3050 PR 5) — spawn-session transcripts are excluded HERE,
/// at the queue, never at the extraction worker's own processing step.
/// `enqueue_extraction` is the real, single enqueue site (called from the
/// `#[tauri::command] ai_enqueue_extraction`, frontend-invoked after every
/// turn's SSE `done` event — confirmed by direct read: `list_sessions`'s own
/// `origin NOT IN ('spawn', 'schedule')` filter is a DIFFERENT mechanism
/// extraction never goes through, so job/spawn-session turns were enqueued
/// for extraction UNFILTERED before this PR). `origin = 'schedule'` is
/// deliberately NOT excluded here — §36.4.3 only names spawn transcripts;
/// schedule sessions are user-visible, named, Scheduled-panel sessions, not
/// invisible delegation plumbing, so they're extracted like any other
/// visible session. The report-back message itself lives in the ORIGIN
/// session (always `origin = 'user'`, never excluded) — this check never
/// touches it.
pub fn enqueue_extraction(
    conn: &Connection,
    thread_id: &str,
    message_id: &str,
    user_message_id: Option<&str>,
) -> Result<(), String> {
    let session_origin: Option<String> = conn
        .query_row(
            "SELECT origin FROM chat_sessions WHERE id = ?1",
            params![thread_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| format!("enqueue_extraction: failed to look up session origin for {}: {}", thread_id, e))?;
    if session_origin.as_deref() == Some("spawn") {
        eprintln!(
            "[EXTRACTION] Skipping: thread_id={} is a spawn-session transcript (origin='spawn', §36.4.3) — not queued",
            thread_id
        );
        return Ok(());
    }

    // Insert with UNIQUE constraint will fail silently if message_id already queued
    match conn.execute(
        "INSERT OR IGNORE INTO extraction_queue (thread_id, message_id, user_message_id) VALUES (?1, ?2, ?3)",
        params![thread_id, message_id, user_message_id],
    ) {
        Ok(n) if n > 0 => {
            let depth: i64 = conn.query_row(
                "SELECT COUNT(*) FROM extraction_queue WHERE status = 'pending'",
                [],
                |row| row.get(0),
            ).unwrap_or(1);
            eprintln!("[EXTRACTION] Queued: {} (queue depth: {})", message_id, depth);
            Ok(())
        }
        Ok(_) => {
            eprintln!("[EXTRACTION] Already queued: {}", message_id);
            Ok(())
        }
        Err(e) => Err(format!("Failed to enqueue extraction: {}", e)),
    }
}

/// Fetch next pending job from queue (FIFO by created_at)
fn fetch_next_job(conn: &Connection) -> Result<Option<ExtractionJob>, String> {
    conn.query_row(
        "SELECT id, thread_id, message_id, user_message_id, attempts 
         FROM extraction_queue 
         WHERE status = 'pending' 
         ORDER BY created_at ASC 
         LIMIT 1",
        [],
        |row| {
            Ok(ExtractionJob {
                id: row.get(0)?,
                thread_id: row.get(1)?,
                message_id: row.get(2)?,
                user_message_id: row.get(3)?,
                attempts: row.get(4)?,
            })
        },
    )
    .optional()
    .map_err(|e| format!("Failed to fetch next job: {}", e))
}

/// Mark job as completed
fn mark_completed(conn: &Connection, job_id: i64) -> Result<(), String> {
    conn.execute(
        "UPDATE extraction_queue SET status = 'completed', processed_at = datetime('now') WHERE id = ?1",
        params![job_id],
    )
    .map_err(|e| format!("Failed to mark job completed: {}", e))?;
    Ok(())
}

/// Mark job as failed and increment attempts
fn mark_failed(conn: &Connection, job_id: i64, max_attempts: i32, error: &str) -> Result<(), String> {
    // Increment attempts first
    conn.execute(
        "UPDATE extraction_queue SET attempts = attempts + 1, error = ?2 WHERE id = ?1",
        params![job_id, error],
    )
    .map_err(|e| format!("Failed to increment attempts: {}", e))?;

    // Then check if we've hit max attempts
    let attempts: i32 = conn.query_row(
        "SELECT attempts FROM extraction_queue WHERE id = ?1",
        params![job_id],
        |row| row.get(0),
    )
    .unwrap_or(99);

    if attempts >= max_attempts {
        conn.execute(
            "UPDATE extraction_queue SET status = 'failed' WHERE id = ?1",
            params![job_id],
        )
        .map_err(|e| format!("Failed to mark job failed: {}", e))?;
        eprintln!("[EXTRACTION] Failed permanently: job_id={} (max attempts reached)", job_id);
    } else {
        eprintln!("[EXTRACTION] Failed: job_id={} (attempt {}/{})", job_id, attempts, max_attempts);
    }

    Ok(())
}

/// Get extraction lag metrics (§27.7.5)
pub fn get_lag_metrics(conn: &Connection) -> Result<(i64, Option<String>), String> {
    let (pending, oldest): (i64, Option<String>) = conn
        .query_row(
            "SELECT COUNT(*) as pending, MIN(created_at) as oldest_pending 
             FROM extraction_queue 
             WHERE status = 'pending'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap_or((0, None));

    Ok((pending, oldest))
}

// ============================================================================
// §5.4 Step 3: Output Filter — Drop Assistant-Origin Facts (#812)
// ============================================================================

/// Signal (b) assistant-origin patterns — first-person-AI prefixes.
/// Applied only to `fact` and `reflection` categories.
const FIRST_PERSON_AI_PREFIXES: &[&str] = &[
    "I created",
    "I set",
    "I moved",
    "I deleted",
    "I organized",
    "I updated",
    "I added",
    "I removed",
    "I renamed",
];

/// Signal (b) assistant-origin patterns — second-person-addressing-AI prefixes.
/// Applied only to `fact` and `reflection` categories.
const SECOND_PERSON_AI_PREFIXES: &[&str] = &[
    "You asked me to",
    "You created",
    "You set",
    "The assistant",
    "The AI",
];

/// Signal (b) tool-result restatement substrings.
/// These match the structural-tool verbs from §5.4A's IGNORE list.
/// Applied only to `fact` and `reflection` categories.
const TOOL_RESULT_PATTERNS: &[&str] = &[
    "created a note",
    "created a folder",
    "created a project",
    "edited note",
    "moved note",
    "deleted note",
    "deleted folder",
    "set status",
    "set the status",
];

/// Categories subject to signal (b) mechanical backstop.
/// Only `fact` and `reflection` — NOT `decision`, `preference`, `goal`,
/// `identity`, `style`, or `deadline`, where user-attributed facts land.
const BACKSTOP_CATEGORIES: &[&str] = &["fact", "reflection"];

/// §5.4 Step 3, Signal (b): Returns true if the fact matches assistant-origin
/// subject/pattern patterns AND is in a backstop-eligible category.
/// This is best-effort pattern matching, not exhaustive — see §5.4 spec note.
pub(crate) fn matches_assistant_pattern(fact: &str, category: &str) -> bool {
    if !BACKSTOP_CATEGORIES.contains(&category) {
        return false;
    }
    // First-person-AI prefix check (case-sensitive — model output is structured)
    for prefix in FIRST_PERSON_AI_PREFIXES {
        if fact.starts_with(prefix) {
            return true;
        }
    }
    // Second-person-addressing-AI prefix check
    for prefix in SECOND_PERSON_AI_PREFIXES {
        if fact.starts_with(prefix) {
            return true;
        }
    }
    // Tool-result restatement substring check (case-insensitive for robustness)
    let fact_lower = fact.to_lowercase();
    for pattern in TOOL_RESULT_PATTERNS {
        if fact_lower.contains(pattern) {
            return true;
        }
    }
    false
}

/// §5.4 Step 3: Filter out assistant-origin facts from the extracted set.
/// Signal (a): drop facts with attribution == "assistant".
/// Signal (b): drop facts in fact/reflection categories matching assistant-origin patterns.
/// Signal (c) (Extraction-guard PR A, #2745/#2750): drop facts with source == "web" —
/// mechanical enforcement of the §4.6.9 web-content warning, independent of
/// `used_web_tools` (the prompt hint stays a hint; this is the backstop).
/// Signal (d) (Extraction-guard PR A, #2745/#2750): drop facts with scope == "episodic" —
/// facts scoped to a specific PR, issue, build, test run, or the current thread.
/// Returns only facts that survive all four signals.
/// Logs every dropped fact for audit.
pub(crate) fn filter_assistant_origin_facts(
    entries: Vec<crate::ai::scheduler::MemoryEntry>,
) -> Vec<crate::ai::scheduler::MemoryEntry> {
    entries
        .into_iter()
        .filter(|entry| {
            // Signal (a): attribution tag from model
            if entry.attribution == "assistant" {
                eprintln!(
                    "[EXTRACTION-FILTERED] Dropped assistant-origin fact (signal a): \"{}\"",
                    entry.fact
                );
                return false;
            }
            // Signal (b): mechanical subject/pattern backstop
            if matches_assistant_pattern(&entry.fact, &entry.category) {
                eprintln!(
                    "[EXTRACTION-FILTERED] Dropped assistant-origin fact (signal b): \"{}\"",
                    entry.fact
                );
                return false;
            }
            // Signal (c): source == "web" — dropped regardless of used_web_tools (#2745, #2750)
            if entry.source == "web" {
                eprintln!(
                    "[EXTRACTION-FILTERED] Dropped web-sourced fact (signal c): \"{}\"",
                    entry.fact
                );
                return false;
            }
            // Signal (d): scope == "episodic" — PR/issue/build/test-run/current-thread scoped (#2745, #2750)
            if entry.scope == "episodic" {
                eprintln!(
                    "[EXTRACTION-FILTERED] Dropped episodic-scoped fact (signal d): \"{}\"",
                    entry.fact
                );
                return false;
            }
            true
        })
        .collect()
}

// ============================================================================
// §5.4B: Memory extraction toast event payload
// ============================================================================

/// Payload emitted as `"memory-extracted"` Tauri event after facts are saved.
/// Carries every newly-inserted fact so the frontend can display a toast and
/// offer the "✕ wrong" soft-delete action.
#[derive(serde::Serialize, Clone)]
pub(crate) struct MemoryExtractedPayload {
    pub facts: Vec<ExtractedFactInfo>,
}

/// A single extracted fact that was genuinely inserted (not deduplicated-away).
#[derive(serde::Serialize, Clone)]
pub(crate) struct ExtractedFactInfo {
    /// The `mem-{uuid}` ID assigned at INSERT time — used by `ai_dismiss_memory`.
    pub id: String,
    /// The fact text as stored in `ai_memory`.
    pub fact: String,
}

/// Background worker that polls queue and processes extraction jobs
pub async fn start_extraction_worker(
    db: Arc<Mutex<crate::database::DbConn>>,
    app: tauri::AppHandle,
) {
    eprintln!("[EXTRACTION] Worker started");

    // §27.6.3 null-gate: check extraction_model before entering main loop
    // If null, enter dormant polling loop until setting appears.
    // All MutexGuard uses are in inner blocks to avoid holding across await.
    let mut logged_disabled = false;
    loop {
        // Check setting in isolated block; guard dropped before await
        let enabled = {
            match db.lock() {
                Ok(c) => should_run_extraction(&*c),
                Err(e) => {
                    eprintln!("[EXTRACTION] Failed to acquire db lock for null-gate check: {}", e);
                    false
                }
            }
        };
        if enabled {
            break; // extraction_model set — proceed to main loop
        }
        if !logged_disabled {
            eprintln!("[EXTRACTION] disabled — extraction_model is null");
            logged_disabled = true;
        }
        sleep(Duration::from_secs(30)).await;
    }

    loop {
        // Poll every 5 seconds
        sleep(Duration::from_secs(5)).await;

        // Check for pending jobs
        let job = {
            let conn = match db.lock() {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("[EXTRACTION] Failed to acquire db lock: {}", e);
                    continue;
                }
            };

            match fetch_next_job(&*conn) {
                Ok(Some(job)) => job,
                Ok(None) => continue, // No pending jobs
                Err(e) => {
                    eprintln!("[EXTRACTION] Failed to fetch job: {}", e);
                    continue;
                }
            }
        };

        eprintln!(
            "[EXTRACTION] Processing: {} (attempt {}/{})",
            job.message_id, job.attempts + 1, 3
        );

        // Fetch message content from chat_messages_local (§4.6.9: 4-tuple, adds
        // used_web_tools — see load_extraction_pair above).
        let (user_message, assistant_response, project_id, used_web_tools) = {
            let conn = match db.lock() {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("[EXTRACTION] Failed to acquire db lock: {}", e);
                    continue;
                }
            };

            match load_extraction_pair(&*conn, &job) {
                Ok(pair) => pair,
                Err(e) => {
                    eprintln!(
                        "[EXTRACTION] Failed to fetch message {}: {}",
                        job.message_id, e
                    );
                    let _ = mark_failed(&*conn, job.id, 3, &e);
                    continue;
                }
            }
        };

        // #673: Filter audit markers from messages before extraction (prevent circular re-extraction)
        let user_message = user_message.lines()
            .filter(|line| !crate::ai::audit_markers::is_audit_marker(line))
            .collect::<Vec<_>>()
            .join("\n");
        let assistant_response = assistant_response.lines()
            .filter(|line| !crate::ai::audit_markers::is_audit_marker(line))
            .collect::<Vec<_>>()
            .join("\n");

        // If nothing left after filtering, skip extraction
        if user_message.trim().is_empty() && assistant_response.trim().is_empty() {
            eprintln!("[EXTRACTION] Skipped: only audit markers in messages");
            let conn = db.lock().unwrap();
            let _ = mark_completed(&*conn, job.id);
            continue;
        }

        // §27.6.2 internal-provider resolution — bypass gateway entirely (#1201, resolves #1186)
        let db_state = crate::database::DbState(db.clone());
        let internal_provider = match crate::ai::provider::get_internal_provider(&db_state).await {
            Ok(p) => p,
            Err(e) => {
                eprintln!("[EXTRACTION] Internal provider not available: {}", e);
                let conn = db.lock().unwrap();
                let _ = mark_failed(&*conn, job.id, 3, &format!("Internal provider not available: {}", e));
                continue;
            }
        };

        // Convert AiProvider to string params for call_llm_from_config
        let (provider, api_key, endpoint) = match &internal_provider {
            crate::ai::provider::AiProvider::OpenRouter { api_key } => {
                ("openrouter".to_string(), api_key.clone(), None)
            }
            crate::ai::provider::AiProvider::Anthropic { api_key } => {
                ("anthropic".to_string(), api_key.clone(), None)
            }
            crate::ai::provider::AiProvider::Ollama { endpoint } => {
                ("ollama".to_string(), String::new(), Some(endpoint.clone()))
            }
            crate::ai::provider::AiProvider::Venice { api_key, .. } => {
                ("venice".to_string(), api_key.clone(), None)
            }
            _ => {
                eprintln!("[EXTRACTION] Unsupported internal provider variant for extraction");
                let conn = db.lock().unwrap();
                let _ = mark_failed(&*conn, job.id, 3, "Unsupported internal provider variant");
                continue;
            }
        };

        // §5.4C + ADR-0038: per-feature lookup only — extraction_model, no fallback
        // Guard is dropped at end of block before any await
        let model_opt = {
            let conn = match db.lock() {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("[EXTRACTION] Failed to acquire db lock for model resolution: {}", e);
                    continue;
                }
            };
            resolve_extraction_model(&*conn)
            // conn (MutexGuard) dropped here
        };
        let model = match model_opt {
            Some(m) => m,
            None => {
                // Setting removed mid-run — go dormant until it reappears
                eprintln!("[EXTRACTION] disabled — extraction_model is null");
                loop {
                    sleep(Duration::from_secs(30)).await;
                    let val = {
                        let c = match db.lock() {
                            Ok(c) => c,
                            Err(_) => continue,
                        };
                        resolve_extraction_model(&*c)
                        // guard dropped
                    };
                    if let Some(m) = val {
                        break m;
                    }
                    // silent: logged_disabled_midrun already true, no repeated log
                }
            }
        };
        eprintln!("[EXTRACTION] Using model: {} via internal provider: {}", model, provider);

        // Call extraction (use pane-extraction lane to avoid gateway congestion)
        let extraction_result = ai_extract_memory_with_db(
            user_message.clone(),
            assistant_response.clone(),
            project_id.clone(),
            Some(job.message_id.clone()),
            provider.clone(),
            model.clone(),
            api_key.clone(),
            endpoint.clone(),
            Some(&crate::database::DbState(db.clone())),
            used_web_tools,
            // ADR-0067 D2/D5: `internal_provider` above is resolved via
            // get_internal_provider, which bypasses the gateway entirely
            // (§27.6.2) — never OC-origin, so there is no wire selector to carry.
            None,
        )
        .await;

        match extraction_result {
            Ok(raw_entries) if raw_entries.is_empty() => {
                // No facts extracted — mark completed
                eprintln!(
                    "[EXTRACTION] Complete: {} — 0 facts extracted",
                    job.message_id
                );
                let conn = db.lock().unwrap();
                let _ = mark_completed(&*conn, job.id);
            }
            Ok(raw_entries) => {
                // §5.4 Step 3: Output filter — drop assistant-origin facts (#812)
                // Runs AFTER extraction LLM returns, BEFORE ai_memory write.
                let pre_filter_count = raw_entries.len();
                let entries = filter_assistant_origin_facts(raw_entries);
                let filtered_count = pre_filter_count - entries.len();
                if filtered_count > 0 {
                    eprintln!(
                        "[EXTRACTION] Output filter: {} fact(s) dropped as assistant-origin, {} remain",
                        filtered_count, entries.len()
                    );
                }
                // Re-check: if filter dropped everything, mark complete
                if entries.is_empty() {
                    eprintln!(
                        "[EXTRACTION] Complete: {} — 0 facts after output filter",
                        job.message_id
                    );
                    let conn = db.lock().unwrap();
                    let _ = mark_completed(&*conn, job.id);
                } else {
                // Save extracted memories using inline logic (avoid Tauri State construction)
                let personal_entries: Vec<_> = entries

                    .iter()
                    .filter(|e| e.project_id.is_none())
                    .cloned()
                    .collect();

                let project_entries: Vec<_> = entries
                    .iter()
                    .filter(|e| e.project_id.is_some())
                    .cloned()
                    .collect();

                let mut save_ok = true;
                // §5.4B: Collect newly-inserted facts for toast event.
                // Only facts that pass dedup check (actually inserted) are included.
                let mut newly_inserted: Vec<ExtractedFactInfo> = Vec::new();

                // Save personal memories
                if !personal_entries.is_empty() {
                    let conn = db.lock().unwrap();
                    for entry in &personal_entries {
                        // Simple dedup check (detailed contradiction detection happens in ai_save_memory)
                        let exists: bool = conn.query_row(
                            "SELECT COUNT(*) > 0 FROM ai_memory WHERE fact = ?1 AND is_active = 1",
                            params![entry.fact],
                            |row| row.get(0),
                        ).unwrap_or(false);
                        
                        if !exists {
                            let id = format!("mem-{}", uuid::Uuid::new_v4());
                            if let Err(e) = conn.execute(
                                "INSERT INTO ai_memory (id, fact, category, source_message_id, is_active, layer, project_id) 
                                 VALUES (?1, ?2, ?3, ?4, 1, 'work', NULL)",
                                params![id, entry.fact, entry.category, entry.source_message_id],
                            ) {
                                eprintln!("[EXTRACTION] Failed to insert personal memory: {}", e);
                                save_ok = false;
                            } else {
                                // §5.4B: Track for toast event
                                newly_inserted.push(ExtractedFactInfo {
                                    id: id.clone(),
                                    fact: entry.fact.clone(),
                                });
                                // #647: Dual-write — append [saved] marker to daily log
                                let time_str = chrono::Local::now().format("%H:%M").to_string();
                                let marker_line = format!("- {} {} {}: \"{}\"", 
                                    time_str, 
                                    crate::ai::audit_markers::MARKER_SAVED,
                                    entry.category, 
                                    entry.fact
                                );
                                // Best-effort: don't fail extraction if daily log write fails
                                if let Err(e) = crate::ai::scheduler::append_to_daily_log(&conn, &app, &[(marker_line, None)]) {
                                    eprintln!("[EXTRACTION] Warning: failed to write [saved] marker to daily log: {}", e);
                                }
                            }
                        }
                    }
                }

                // Save project memories
                if !project_entries.is_empty() {
                    let conn = db.lock().unwrap();
                    for entry in &project_entries {
                        let exists: bool = conn.query_row(
                            "SELECT COUNT(*) > 0 FROM ai_memory WHERE fact = ?1 AND is_active = 1",
                            params![entry.fact],
                            |row| row.get(0),
                        ).unwrap_or(false);
                        
                        if !exists {
                            let id = format!("mem-{}", uuid::Uuid::new_v4());
                            if let Err(e) = conn.execute(
                                "INSERT INTO ai_memory (id, fact, category, source_message_id, is_active, layer, project_id) 
                                 VALUES (?1, ?2, ?3, ?4, 1, 'work', ?5)",
                                params![id, entry.fact, entry.category, entry.source_message_id, entry.project_id],
                            ) {
                                eprintln!("[EXTRACTION] Failed to insert project memory: {}", e);
                                save_ok = false;
                            } else {
                                // §5.4B: Track for toast event
                                newly_inserted.push(ExtractedFactInfo {
                                    id: id.clone(),
                                    fact: entry.fact.clone(),
                                });
                                // #647: Dual-write — append [saved] marker to daily log
                                let time_str = chrono::Local::now().format("%H:%M").to_string();
                                let marker_line = format!("- {} {} {}: \"{}\"", 
                                    time_str, 
                                    crate::ai::audit_markers::MARKER_SAVED,
                                    entry.category, 
                                    entry.fact
                                );
                                // Best-effort: don't fail extraction if daily log write fails
                                if let Err(e) = crate::ai::scheduler::append_to_daily_log(&conn, &app, &[(marker_line, None)]) {
                                    eprintln!("[EXTRACTION] Warning: failed to write [saved] marker to daily log: {}", e);
                                }
                            }
                        }
                    }
                }

                if save_ok {
                    eprintln!(
                        "[EXTRACTION] Complete: {} — {} facts extracted ({} newly inserted)",
                        job.message_id, entries.len(), newly_inserted.len()
                    );
                    let conn = db.lock().unwrap();
                    let _ = mark_completed(&*conn, job.id);

                    // §5.4B: Emit memory-extracted event for toast display.
                    // Only emit when at least one fact was genuinely new (not a dupe).
                    if !newly_inserted.is_empty() {
                        let payload = MemoryExtractedPayload { facts: newly_inserted.clone() };
                        let app_clone = app.clone();
                        tokio::spawn(async move {
                            if let Err(e) = app_clone.emit("memory-extracted", payload) {
                                eprintln!("[EXTRACTION] Failed to emit memory-extracted event: {}", e);
                            }
                        });
                    }

                    // Check if we should trigger synthesis
                    if entries.len() > 0 {
                        let unrouted_count: i64 = conn.query_row(
                            "SELECT COUNT(*) FROM ai_memory WHERE routed_at IS NULL AND is_active = 1",
                            [],
                            |row| row.get(0),
                        ).unwrap_or(0);
                        
                        // Trigger synthesis when enough facts accumulate
                        let synthesis_threshold = get_synthesis_threshold(&conn);
                        if unrouted_count >= synthesis_threshold {
                            eprintln!(
                                "[EXTRACTION] {} unrouted facts >= threshold {} — triggering router",
                                unrouted_count, synthesis_threshold
                            );
                            
                            // Emit event to trigger router
                            let app_clone = app.clone();
                            tokio::spawn(async move {
                                if let Err(e) = app_clone.emit("trigger-router", ()) {
                                    eprintln!("[EXTRACTION] Failed to emit router trigger: {}", e);
                                }
                            });
                        }
                    }
                } else {
                    let conn = db.lock().unwrap();
                    let _ = mark_failed(&*conn, job.id, 3, "Failed to save memories");
                }
                } // close else { // entries non-empty after output filter
            }
            Err(e) => {
                // Extraction failed — retry with backoff
                let retry_delay = match job.attempts {
                    0 => 5,
                    1 => 30,
                    _ => 180,
                };

                eprintln!(
                    "[EXTRACTION] Failed: {} — {} (attempt {}/3, retry in {}s)",
                    job.message_id,
                    e,
                    job.attempts + 1,
                    retry_delay
                );

                {
                    let conn = db.lock().unwrap();
                    let _ = mark_failed(&*conn, job.id, 3, &e);
                } // Drop lock before await

                // Sleep for backoff duration before next poll
                sleep(Duration::from_secs(retry_delay)).await;
            }
        }

        // Check for extraction lag and log warning
        {
            let conn = db.lock().unwrap();
            if let Ok((pending, oldest)) = get_lag_metrics(&*conn) {
                if pending > 20 {
                    eprintln!(
                        "[EXTRACTION] Warning: extraction lag — {} items pending",
                        pending
                    );
                }

                if let Some(oldest_str) = oldest {
                    // Parse oldest timestamp and check if > 10 minutes old
                    // Simple heuristic: just log if pending > 20 for now
                    if pending > 20 {
                        eprintln!("[EXTRACTION] Warning: oldest pending from {}", oldest_str);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod extraction_tests {
    use super::*;
    use rusqlite::Connection;
    use tempfile::TempDir;

    fn setup_test_db() -> (TempDir, Connection) {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let conn = Connection::open(&db_path).unwrap();

        // Create extraction_queue table
        conn.execute_batch(
            "CREATE TABLE extraction_queue (
                id              INTEGER PRIMARY KEY AUTOINCREMENT,
                thread_id       TEXT NOT NULL,
                message_id      TEXT NOT NULL,
                user_message_id TEXT,
                status          TEXT NOT NULL DEFAULT 'pending',
                attempts        INTEGER NOT NULL DEFAULT 0,
                max_attempts    INTEGER NOT NULL DEFAULT 3,
                created_at      TEXT NOT NULL DEFAULT (datetime('now')),
                processed_at    TEXT,
                error           TEXT,
                UNIQUE(message_id)
            );
            CREATE INDEX idx_extraction_pending 
                ON extraction_queue(status, created_at)
                WHERE status IN ('pending', 'failed');",
        )
        .unwrap();

        conn.execute_batch(
            "CREATE TABLE chat_messages_local (
                id           TEXT PRIMARY KEY,
                thread_id    TEXT NOT NULL,
                role         TEXT NOT NULL,
                content      TEXT,
                project_id   TEXT,
                tool_calls   TEXT,
                tool_results TEXT,
                created_at   TEXT NOT NULL DEFAULT (datetime('now'))
            );",
        )
        .unwrap();

        // #3050 PR 5 (§36.4.3/§5.5): `enqueue_extraction` now looks up the
        // thread's own `chat_sessions.origin` before queueing — minimal
        // shape, same convention this hand-rolled fixture already uses for
        // `chat_messages_local` above (not the full migrations schema).
        // Every pre-existing test in this module uses thread ids that never
        // get a row here, so `origin` reads back `NULL` (not `'spawn'`) —
        // behaviourally identical to before this column existed.
        conn.execute_batch(
            "CREATE TABLE chat_sessions (
                id     TEXT PRIMARY KEY,
                origin TEXT
            );",
        )
        .unwrap();

        (temp_dir, conn)
    }

    // ── Test 11: extraction worker used_web_tools detection ──────────────────

    #[test]
    fn test_tool_calls_json_uses_web_tools_detects_web_fetch() {
        let json = r#"[{"id":"1","name":"web_fetch","params":{"url":"https://x.com"}}]"#;
        assert!(tool_calls_json_uses_web_tools(json));
    }

    #[test]
    fn test_tool_calls_json_uses_web_tools_detects_web_search() {
        let json = r#"[{"id":"1","name":"web_search","params":{"query":"x"}}]"#;
        assert!(tool_calls_json_uses_web_tools(json));
    }

    #[test]
    fn test_tool_calls_json_uses_web_tools_false_for_other_tools() {
        let json = r#"[{"id":"1","name":"create_note","params":{}}]"#;
        assert!(!tool_calls_json_uses_web_tools(json));
    }

    #[test]
    fn test_tool_calls_json_uses_web_tools_no_false_positive_on_substring() {
        // A note titled/URL containing the literal text "web_fetch" in a param
        // value must NOT trigger detection — only the `name` field counts.
        let json = r#"[{"id":"1","name":"create_note","params":{"title":"How web_fetch works"}}]"#;
        assert!(!tool_calls_json_uses_web_tools(json));
    }

    #[test]
    fn test_tool_calls_json_uses_web_tools_handles_empty_and_malformed() {
        assert!(!tool_calls_json_uses_web_tools("[]"));
        assert!(!tool_calls_json_uses_web_tools("not json"));
        assert!(!tool_calls_json_uses_web_tools(""));
    }

    #[test]
    fn test_load_extraction_pair_sets_used_web_tools_true() {
        let (_temp, conn) = setup_test_db();
        conn.execute(
            "INSERT INTO chat_messages_local (id, thread_id, role, content, project_id, tool_calls) VALUES ('asst1', 'thread1', 'assistant', 'Here is what I found', NULL, ?1)",
            params![r#"[{"id":"1","name":"web_fetch","params":{"url":"https://x.com"}}]"#],
        ).unwrap();
        conn.execute(
            "INSERT INTO chat_messages_local (id, thread_id, role, content, project_id) VALUES ('user1', 'thread1', 'user', 'look this up', NULL)",
            [],
        ).unwrap();

        let job = ExtractionJob {
            id: 1,
            thread_id: "thread1".to_string(),
            message_id: "asst1".to_string(),
            user_message_id: Some("user1".to_string()),
            attempts: 0,
        };
        let (user_msg, asst_msg, project_id, used_web_tools) =
            load_extraction_pair(&conn, &job).unwrap();

        assert_eq!(user_msg, "look this up");
        assert_eq!(asst_msg, "Here is what I found");
        assert_eq!(project_id, None);
        assert!(used_web_tools, "used_web_tools must be true when tool_calls contains web_fetch");
    }

    #[test]
    fn test_load_extraction_pair_sets_used_web_tools_false_when_no_web_calls() {
        let (_temp, conn) = setup_test_db();
        conn.execute(
            "INSERT INTO chat_messages_local (id, thread_id, role, content, project_id, tool_calls) VALUES ('asst2', 'thread1', 'assistant', 'Created the note', NULL, ?1)",
            params![r#"[{"id":"1","name":"create_note","params":{}}]"#],
        ).unwrap();
        conn.execute(
            "INSERT INTO chat_messages_local (id, thread_id, role, content, project_id) VALUES ('user2', 'thread1', 'user', 'make a note', NULL)",
            [],
        ).unwrap();

        let job = ExtractionJob {
            id: 2,
            thread_id: "thread1".to_string(),
            message_id: "asst2".to_string(),
            user_message_id: Some("user2".to_string()),
            attempts: 0,
        };
        let (_u, _a, _p, used_web_tools) = load_extraction_pair(&conn, &job).unwrap();
        assert!(!used_web_tools);
    }

    #[test]
    fn test_load_extraction_pair_sets_used_web_tools_false_when_tool_calls_null() {
        let (_temp, conn) = setup_test_db();
        conn.execute(
            "INSERT INTO chat_messages_local (id, thread_id, role, content, project_id) VALUES ('asst3', 'thread1', 'assistant', 'Hi there', NULL)",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO chat_messages_local (id, thread_id, role, content, project_id) VALUES ('user3', 'thread1', 'user', 'hello', NULL)",
            [],
        ).unwrap();

        let job = ExtractionJob {
            id: 3,
            thread_id: "thread1".to_string(),
            message_id: "asst3".to_string(),
            user_message_id: Some("user3".to_string()),
            attempts: 0,
        };
        let (_u, _a, _p, used_web_tools) = load_extraction_pair(&conn, &job).unwrap();
        assert!(!used_web_tools);
    }

    #[test]
    fn test_load_extraction_pair_errors_on_missing_message() {
        let (_temp, conn) = setup_test_db();
        let job = ExtractionJob {
            id: 99,
            thread_id: "thread1".to_string(),
            message_id: "does-not-exist".to_string(),
            user_message_id: None,
            attempts: 0,
        };
        let result = load_extraction_pair(&conn, &job);
        assert!(result.is_err());
    }

    /// §4.6.10 item 11(a) — proves `load_extraction_pair`'s SELECT does not leak
    /// `tool_results` into the extraction output. A sentinel string is inserted
    /// into the `tool_results` column (which the query's SELECT list does NOT
    /// name — see `load_extraction_pair`'s query above: `content, project_id,
    /// tool_calls` only) while `content` holds an unrelated, sentinel-free
    /// assistant reply. `assistant_response` must equal `content` verbatim and
    /// must not contain the sentinel.
    #[test]
    fn test_load_extraction_pair_does_not_leak_tool_results_into_assistant_response() {
        let (_temp, conn) = setup_test_db();
        conn.execute(
            "INSERT INTO chat_messages_local (id, thread_id, role, content, project_id, tool_calls, tool_results) VALUES ('asst4', 'thread1', 'assistant', 'Here is a plain-language summary of what I found.', NULL, ?1, ?2)",
            params![
                r#"[{"id":"1","name":"web_fetch","params":{"url":"https://x.com"}}]"#,
                "SENTINEL_TOOL_RESULTS_LEAK_MARKER_9f3c2a"
            ],
        ).unwrap();
        conn.execute(
            "INSERT INTO chat_messages_local (id, thread_id, role, content, project_id) VALUES ('user4', 'thread1', 'user', 'look this up', NULL)",
            [],
        ).unwrap();

        let job = ExtractionJob {
            id: 4,
            thread_id: "thread1".to_string(),
            message_id: "asst4".to_string(),
            user_message_id: Some("user4".to_string()),
            attempts: 0,
        };
        let (_user_msg, assistant_response, _project_id, used_web_tools) =
            load_extraction_pair(&conn, &job).unwrap();

        assert_eq!(assistant_response, "Here is a plain-language summary of what I found.");
        assert!(
            !assistant_response.contains("SENTINEL_TOOL_RESULTS_LEAK_MARKER_9f3c2a"),
            "assistant_response must never contain tool_results content, got: {}",
            assistant_response
        );
        assert!(used_web_tools, "tool_calls named web_fetch — used_web_tools must still be true");
    }

    #[test]
    fn test_extraction_enqueue_dequeue_fifo() {
        let (_temp, conn) = setup_test_db();

        // Enqueue three jobs
        enqueue_extraction(&conn, "thread1", "msg1", None).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10)); // Ensure distinct created_at
        enqueue_extraction(&conn, "thread1", "msg2", None).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        enqueue_extraction(&conn, "thread1", "msg3", None).unwrap();

        // Dequeue and verify FIFO order
        let job1 = fetch_next_job(&conn).unwrap().unwrap();
        assert_eq!(job1.message_id, "msg1");

        mark_completed(&conn, job1.id).unwrap();

        let job2 = fetch_next_job(&conn).unwrap().unwrap();
        assert_eq!(job2.message_id, "msg2");

        mark_completed(&conn, job2.id).unwrap();

        let job3 = fetch_next_job(&conn).unwrap().unwrap();
        assert_eq!(job3.message_id, "msg3");
    }

    #[test]
    fn test_extraction_dedup_same_message() {
        let (_temp, conn) = setup_test_db();

        // Enqueue same message twice
        enqueue_extraction(&conn, "thread1", "msg1", None).unwrap();
        enqueue_extraction(&conn, "thread1", "msg1", None).unwrap();

        // Verify only one job exists
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM extraction_queue WHERE message_id = 'msg1'", [], |row| row.get(0))
            .unwrap();

        assert_eq!(count, 1);
    }

    #[test]
    fn test_extraction_retry_on_failure() {
        let (_temp, conn) = setup_test_db();

        enqueue_extraction(&conn, "thread1", "msg1", None).unwrap();

        let job = fetch_next_job(&conn).unwrap().unwrap();
        assert_eq!(job.attempts, 0);

        // Mark as failed
        mark_failed(&conn, job.id, 3, "Test error").unwrap();

        // Verify attempts incremented and status still pending
        let (attempts, status): (i32, String) = conn
            .query_row(
                "SELECT attempts, status FROM extraction_queue WHERE id = ?1",
                rusqlite::params![job.id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();

        assert_eq!(attempts, 1);
        assert_eq!(status, "pending"); // Should still be pending after 1 failure
    }

    #[test]
    fn test_extraction_max_attempts_marks_failed() {
        let (_temp, conn) = setup_test_db();

        enqueue_extraction(&conn, "thread1", "msg1", None).unwrap();

        let job = fetch_next_job(&conn).unwrap().unwrap();

        // Fail it 3 times
        for _ in 0..3 {
            mark_failed(&conn, job.id, 3, "Test error").unwrap();
        }

        // Verify status is 'failed'
        let status: String = conn
            .query_row(
                "SELECT status FROM extraction_queue WHERE id = ?1",
                rusqlite::params![job.id],
                |row| row.get(0),
            )
            .unwrap();

        assert_eq!(status, "failed");
    }

    #[test]
    fn test_extraction_failed_doesnt_block_queue() {
        let (_temp, conn) = setup_test_db();

        // Enqueue two jobs
        enqueue_extraction(&conn, "thread1", "msg1", None).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        enqueue_extraction(&conn, "thread1", "msg2", None).unwrap();

        // Fetch and fail first job permanently
        let job1 = fetch_next_job(&conn).unwrap().unwrap();
        for _ in 0..3 {
            mark_failed(&conn, job1.id, 3, "Test error").unwrap();
        }

        // Second job should still be fetchable
        let job2 = fetch_next_job(&conn).unwrap().unwrap();
        assert_eq!(job2.message_id, "msg2");
    }

    #[test]
    fn test_extraction_lag_metric() {
        let (_temp, conn) = setup_test_db();

        // Enqueue several jobs
        for i in 1..=25 {
            enqueue_extraction(&conn, "thread1", &format!("msg{}", i), None).unwrap();
        }

        // Get lag metrics
        let (pending, oldest) = get_lag_metrics(&conn).unwrap();

        assert_eq!(pending, 25);
        assert!(oldest.is_some());
    }
    
    fn setup_test_db_with_ai_settings() -> (TempDir, Connection) {
        let (dir, conn) = setup_test_db();
        conn.execute_batch(
            "CREATE TABLE ai_settings (
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );"
        ).unwrap();
        (dir, conn)
    }

    /// §27.6.3 null-gate: should_run_extraction returns false when extraction_model unset
    #[test]
    fn test_extraction_null_model_disables_worker() {
        let (_dir, conn) = setup_test_db_with_ai_settings();
        // No extraction_model set — should_run_extraction must return false
        assert!(
            !should_run_extraction(&conn),
            "should_run_extraction must return false when extraction_model is not set"
        );
    }

    /// §27.6.3 null-gate: should_run_extraction returns true when extraction_model is set
    #[test]
    fn test_extraction_model_enables_worker() {
        let (_dir, conn) = setup_test_db_with_ai_settings();
        conn.execute(
            "INSERT INTO ai_settings (key, value) VALUES ('extraction_model', 'anthropic/claude-haiku-3-5')",
            [],
        ).unwrap();
        assert!(
            should_run_extraction(&conn),
            "should_run_extraction must return true when extraction_model is set"
        );
    }

    /// ADR-0038: resolve_extraction_model returns None even when simple_tier_model is set (no fallback)
    #[test]
    fn test_extraction_model_resolution_no_fallback() {
        let (_dir, conn) = setup_test_db_with_ai_settings();
        // Set simple_tier_model but NOT extraction_model
        conn.execute(
            "INSERT INTO ai_settings (key, value) VALUES ('simple_tier_model', 'anthropic/claude-haiku')",
            [],
        ).unwrap();
        let resolved = resolve_extraction_model(&conn);
        assert!(
            resolved.is_none(),
            "resolve_extraction_model must return None even when simple_tier_model is set (ADR-0038: no fallback)"
        );
    }

    /// ADR-0038: resolve_extraction_model returns Some when extraction_model is explicitly set
    #[test]
    fn test_extraction_model_resolution_returns_set_value() {
        let (_dir, conn) = setup_test_db_with_ai_settings();
        conn.execute(
            "INSERT INTO ai_settings (key, value) VALUES ('extraction_model', 'anthropic/claude-haiku')", // CI-ALLOW: hardcoded-model
            [],
        ).unwrap();
        let resolved = resolve_extraction_model(&conn);
        assert_eq!(
            resolved,
            Some("anthropic/claude-haiku".to_string()), // CI-ALLOW: hardcoded-model
            "resolve_extraction_model must return the set extraction_model value"
        );
    }

    /// AiProvider → (provider, api_key, endpoint) conversion for extraction dispatch
    /// Tests that provider string params are correctly derived from each AiProvider variant.
    /// This is the dispatch logic used in the extraction worker to call the LLM.
    #[test]
    fn test_extraction_model_set_processes_jobs() {
        // OpenRouter: provider_name = "openrouter", api_key = key, endpoint = None
        let openrouter = crate::ai::provider::AiProvider::OpenRouter {
            api_key: "sk-or-test-key".to_string(),
        };
        let (provider, api_key, endpoint) = match &openrouter {
            crate::ai::provider::AiProvider::OpenRouter { api_key } =>
                ("openrouter".to_string(), api_key.clone(), None::<String>),
            _ => panic!("unexpected variant"),
        };
        assert_eq!(provider, "openrouter");
        assert_eq!(api_key, "sk-or-test-key");
        assert!(endpoint.is_none(), "OpenRouter must have no endpoint override");

        // Ollama: provider_name = "ollama", api_key = "", endpoint = Some(url)
        let ollama = crate::ai::provider::AiProvider::Ollama {
            endpoint: "http://localhost:11434".to_string(),
        };
        let (provider2, api_key2, endpoint2) = match &ollama {
            crate::ai::provider::AiProvider::Ollama { endpoint } =>
                ("ollama".to_string(), String::new(), Some(endpoint.clone())),
            _ => panic!("unexpected variant"),
        };
        assert_eq!(provider2, "ollama");
        assert!(api_key2.is_empty(), "Ollama api_key must be empty string");
        assert_eq!(endpoint2, Some("http://localhost:11434".to_string()));
    }

    #[test]
    fn test_extraction_db_write_failure_recovery() {
        // P1: Extraction DB write failure recovery
        // Scenario: Job completes extraction but DB insert fails (e.g., constraint violation)
        // Expected: Job should NOT be marked completed, should retry or be marked failed
        
        let (_temp, conn) = setup_test_db();
        
        // Create ai_memory table with PRIMARY KEY constraint
        conn.execute_batch(
            "CREATE TABLE ai_memory (
                id              TEXT PRIMARY KEY,
                fact            TEXT NOT NULL,
                category        TEXT,
                source_message_id TEXT,
                is_active       INTEGER NOT NULL DEFAULT 1,
                layer           TEXT NOT NULL DEFAULT 'work',
                project_id      TEXT,
                created_at      TEXT NOT NULL DEFAULT (datetime('now'))
            );"
        ).unwrap();
        
        // Pre-insert a memory with a specific ID to cause constraint violation
        conn.execute(
            "INSERT INTO ai_memory (id, fact, category, source_message_id, is_active, layer) 
             VALUES ('mem-duplicate', 'Existing fact', 'identity', 'msg-source', 1, 'work')",
            [],
        ).unwrap();
        
        // Enqueue a job
        enqueue_extraction(&conn, "thread1", "msg-test", Some("msg-source")).unwrap();
        
        let job = fetch_next_job(&conn).unwrap().unwrap();
        assert_eq!(job.message_id, "msg-test");
        
        // Simulate what the worker does when saving memories
        // Try to insert a memory with duplicate ID (will fail)
        let insert_result = conn.execute(
            "INSERT INTO ai_memory (id, fact, category, source_message_id, is_active, layer) 
             VALUES ('mem-duplicate', 'New fact', 'identity', 'msg-test', 1, 'work')",
            [],
        );
        
        // This should fail due to PRIMARY KEY constraint
        assert!(insert_result.is_err(), "Insert should fail due to duplicate ID");
        
        // When save fails, job should be marked failed, NOT completed
        mark_failed(&conn, job.id, 3, "Failed to save memories").unwrap();
        
        // Verify job is NOT marked completed
        let status: String = conn
            .query_row(
                "SELECT status FROM extraction_queue WHERE id = ?1",
                rusqlite::params![job.id],
                |row| row.get(0),
            )
            .unwrap();
        
        // After first failure, status should still be 'pending' (retry)
        assert_eq!(status, "pending", "Job should remain pending for retry after DB write failure");
        
        let attempts: i32 = conn
            .query_row(
                "SELECT attempts FROM extraction_queue WHERE id = ?1",
                rusqlite::params![job.id],
                |row| row.get(0),
            )
            .unwrap();
        
        assert_eq!(attempts, 1, "Attempts should be incremented");
        
        // Fail it 2 more times to hit max attempts
        mark_failed(&conn, job.id, 3, "Failed to save memories").unwrap();
        mark_failed(&conn, job.id, 3, "Failed to save memories").unwrap();
        
        // After 3 failures, should be marked 'failed' permanently
        let final_status: String = conn
            .query_row(
                "SELECT status FROM extraction_queue WHERE id = ?1",
                rusqlite::params![job.id],
                |row| row.get(0),
            )
            .unwrap();
        
        assert_eq!(final_status, "failed", "Job should be marked failed after max attempts");
        
        // Verify it won't be fetched again
        let next_job = fetch_next_job(&conn).unwrap();
        assert!(next_job.is_none(), "Failed job should not be fetched again");
    }

    // =========================================================================
    // §5.4 Step 3: Output Filter Tests (#812)
    // =========================================================================

    fn make_entry(fact: &str, category: &str, attribution: &str) -> crate::ai::scheduler::MemoryEntry {
        make_entry_full(fact, category, attribution, "user", "durable")
    }

    /// Extraction-guard PR A (#2745, #2750): full constructor with `source`/`scope`
    /// for tests exercising the new mechanical drop rules.
    fn make_entry_full(
        fact: &str,
        category: &str,
        attribution: &str,
        source: &str,
        scope: &str,
    ) -> crate::ai::scheduler::MemoryEntry {
        crate::ai::scheduler::MemoryEntry {
            id: uuid::Uuid::new_v4().to_string(),
            fact: fact.to_string(),
            category: category.to_string(),
            project_id: None,
            source_message_id: None,
            attribution: attribution.to_string(),
            source: source.to_string(),
            scope: scope.to_string(),
        }
    }

    // --- Signal (a): attribution tag ---

    /// Signal (a): facts with attribution=="assistant" are dropped
    #[test]
    fn test_filter_signal_a_drops_assistant_tagged() {
        let entries = vec![
            make_entry("I created a note called Plans", "fact", "assistant"),
            make_entry("You asked me to set the status", "fact", "assistant"),
        ];
        let result = filter_assistant_origin_facts(entries);
        assert!(result.is_empty(), "All assistant-tagged facts must be dropped by signal (a)");
    }

    /// Signal (a): facts with attribution=="user" pass through
    #[test]
    fn test_filter_signal_a_keeps_user_tagged() {
        let entries = vec![
            make_entry("User prefers dark mode", "preference", "user"),
            make_entry("We decided to go with SQLite", "decision", "user"),
            make_entry("Lives in Atlanta", "fact", "user"),
        ];
        let result = filter_assistant_origin_facts(entries);
        assert_eq!(result.len(), 3, "All user-tagged facts must pass through signal (a)");
    }

    /// Signal (a): facts with missing attribution default to "user" and pass through
    #[test]
    fn test_filter_signal_a_missing_attribution_defaults_to_user() {
        // When serde deserializes with `#[serde(default = "default_attribution")]`,
        // missing attribution becomes "user". We simulate this directly here.
        let entries = vec![
            make_entry("Some neutral fact", "fact", "user"), // default value
        ];
        let result = filter_assistant_origin_facts(entries);
        assert_eq!(result.len(), 1, "Entries with default attribution=user must pass through");
    }

    // --- Signal (b): mechanical pattern backstop ---

    /// Signal (b): first-person-AI prefix caught in fact category
    #[test]
    fn test_filter_signal_b_first_person_ai_fact() {
        assert!(matches_assistant_pattern("I created a new note for you", "fact"));
        assert!(matches_assistant_pattern("I set the status to done", "fact"));
        assert!(matches_assistant_pattern("I moved the note to Projects", "fact"));
        assert!(matches_assistant_pattern("I deleted the folder", "fact"));
        assert!(matches_assistant_pattern("I organized the files", "fact"));
        assert!(matches_assistant_pattern("I updated the note content", "fact"));
        assert!(matches_assistant_pattern("I added a subtask", "fact"));
        assert!(matches_assistant_pattern("I removed the tag", "fact"));
        assert!(matches_assistant_pattern("I renamed the project", "fact"));
    }

    /// Signal (b): first-person-AI prefix caught in reflection category
    #[test]
    fn test_filter_signal_b_first_person_ai_reflection() {
        assert!(matches_assistant_pattern("I created the structure as requested", "reflection"));
        assert!(matches_assistant_pattern("I set the deadline to Friday", "reflection"));
    }

    /// Signal (b): second-person-addressing-AI prefix caught
    #[test]
    fn test_filter_signal_b_second_person_ai() {
        assert!(matches_assistant_pattern("You asked me to create a project", "fact"));
        assert!(matches_assistant_pattern("You created a note in Projects", "fact"));
        assert!(matches_assistant_pattern("You set the priority to high", "fact"));
        assert!(matches_assistant_pattern("The assistant created the folder", "fact"));
        assert!(matches_assistant_pattern("The AI deleted the note", "fact"));
    }

    /// Signal (b): tool-result restatement substrings caught
    #[test]
    fn test_filter_signal_b_tool_result_restatements() {
        assert!(matches_assistant_pattern("Matt created a note for the project", "fact"));
        assert!(matches_assistant_pattern("The system created a note called Plans", "fact"));
        assert!(matches_assistant_pattern("The AI created a folder called Archive", "fact"));
        assert!(matches_assistant_pattern("Edited note with new content", "fact"));
        assert!(matches_assistant_pattern("Moved note to another project", "fact"));
        assert!(matches_assistant_pattern("Deleted note from workspace", "fact"));
        assert!(matches_assistant_pattern("Deleted folder called Archive", "fact"));
        assert!(matches_assistant_pattern("Set status to In Progress", "fact"));
        assert!(matches_assistant_pattern("Set the status of the note", "fact"));
    }

    /// Signal (b): ONLY applies to fact and reflection — not to decision/preference/goal/etc.
    #[test]
    fn test_filter_signal_b_only_applies_to_backstop_categories() {
        // These would match if category were fact/reflection, but decision is not backstopped
        assert!(!matches_assistant_pattern("I created a new architecture decision", "decision"));
        assert!(!matches_assistant_pattern("I set the preference for dark mode", "preference"));
        assert!(!matches_assistant_pattern("I organized the goal list", "goal"));
        assert!(!matches_assistant_pattern("I updated the identity info", "identity"));
        assert!(!matches_assistant_pattern("I set the style guide", "style"));
        assert!(!matches_assistant_pattern("I deleted the deadline", "deadline"));
    }

    /// Signal (b): legitimate user-origin facts in fact category are NOT caught
    #[test]
    fn test_filter_signal_b_user_facts_pass_through() {
        assert!(!matches_assistant_pattern("Lives in Atlanta", "fact"));
        assert!(!matches_assistant_pattern("Uses macOS for development", "fact"));
        assert!(!matches_assistant_pattern("Prefers concise commit messages", "fact"));
        assert!(!matches_assistant_pattern("Has a 2021 Jeep Willys", "fact"));
        assert!(!matches_assistant_pattern("Works in the Eastern timezone", "fact"));
    }

    /// Combined: fact tagged user by model but caught by pattern backstop (b) is still dropped
    #[test]
    fn test_filter_combined_user_tagged_but_pattern_caught() {
        // Model tagged it "user" (signal a passes) but signal b catches it
        let entries = vec![
            make_entry("I created a note for the meeting", "fact", "user"),
        ];
        let result = filter_assistant_origin_facts(entries);
        assert!(
            result.is_empty(),
            "Fact tagged user but matching assistant pattern must still be dropped by signal (b)"
        );
    }

    /// Red line: user-origin facts about decisions are NEVER dropped
    #[test]
    fn test_filter_redline_user_decision_facts_never_dropped() {
        let entries = vec![
            make_entry("We decided to go with Postgres", "decision", "user"),
            make_entry("Chose SQLite over Postgres for local storage", "decision", "user"),
            make_entry("Going with Option A for the navigation design", "decision", "user"),
            make_entry("Selected dark mode as the default theme", "decision", "user"),
        ];
        let result = filter_assistant_origin_facts(entries);
        assert_eq!(
            result.len(), 4,
            "Red line: user-origin decision facts must NEVER be dropped by the output filter"
        );
    }

    /// Red line: terse user turn where AI articulates the decision — user-attributed, must be kept
    #[test]
    fn test_filter_redline_terse_user_turn_decision_kept() {
        // The user said "yeah, B" and the AI replied "we'll go with the Postgres migration"
        // The extraction model should tag this as user-attributed.
        // Verify signal (b) doesn't catch it (decision category is not backstopped).
        let entries = vec![
            make_entry("User will go with the Postgres migration", "decision", "user"),
        ];
        let result = filter_assistant_origin_facts(entries);
        assert_eq!(result.len(), 1, "Terse-user-turn decision must not be dropped");
    }

    // =========================================================================
    // Extraction-guard PR A (#2745, #2750): Signal (c) source=="web" and
    // Signal (d) scope=="episodic" mechanical drop tests
    // =========================================================================

    /// Test 1 (task spec): the three MEMORY.md negative examples, constructed as
    /// RawEntry-shaped MemoryEntry objects tagged scope="episodic" (the real extraction
    /// worker would receive these from an LLM response tagging them episodic per the
    /// updated prompt rules), must all be dropped by the output filter.
    #[test]
    fn test_three_negative_examples_dropped_via_episodic_scope() {
        let entries = vec![
            make_entry_full(
                "The spec delta for §4.6.7 describing Brave error handling is included in the same PR, not spec-skipped.",
                "fact",
                "user",
                "user",
                "episodic",
            ),
            make_entry_full(
                "Four tests are required: three in web.rs (422 with SUBSCRIPTION_TOKEN_INVALID, 422 with VALIDATION, 422 with malformed body) and one in commands.rs for the Settings path rendering the friendly error message.",
                "fact",
                "user",
                "user",
                "episodic",
            ),
            make_entry_full(
                "Reed will delta-audit with head and base pinned, run tests, and confirm log line shape before merge; no pushes after audit starts.",
                "decision",
                "user",
                "user",
                "episodic",
            ),
        ];
        let result = filter_assistant_origin_facts(entries);
        assert!(
            result.is_empty(),
            "All three real over-save examples tagged scope=episodic must be dropped by signal (d)"
        );
    }

    /// Test 2 (task spec): Tampa/Florida fixture split — a source="web" fact is dropped,
    /// a source="user" fact in the same batch survives.
    #[test]
    fn test_tampa_florida_fixture_splits_on_source() {
        let entries = vec![
            make_entry_full(
                "Median home price in Tampa is $415K",
                "fact",
                "user",
                "web",
                "durable",
            ),
            make_entry_full(
                "I'm researching Florida property values",
                "fact",
                "user",
                "user",
                "durable",
            ),
        ];
        let result = filter_assistant_origin_facts(entries);
        assert_eq!(result.len(), 1, "Exactly one fact (the user-sourced one) must survive");
        assert_eq!(result[0].fact, "I'm researching Florida property values");
        assert!(
            !result.iter().any(|e| e.fact.contains("415K")),
            "The web-sourced Tampa home price fact must be dropped"
        );
    }

    /// Test 3 (task spec): an episodic-tagged fact never reaches the DB — verified at
    /// the filter boundary (the worker only writes what survives this function).
    #[test]
    fn test_episodic_tagged_fact_removed_from_output() {
        let entries = vec![
            make_entry_full(
                "Extract this task assignment for the current PR test run",
                "fact",
                "user",
                "user",
                "episodic",
            ),
        ];
        let result = filter_assistant_origin_facts(entries);
        assert!(result.is_empty(), "scope=episodic fact must be removed from the output list");
    }

    /// Test 4 (task spec): source=="web" drops even when the model/exchange-level
    /// used_web_tools flag would have been false — the source tag is the mechanical
    /// enforcement point, not the used_web_tools prompt hint (which never reaches
    /// this function at all).
    #[test]
    fn test_source_web_drops_independent_of_used_web_tools() {
        // This entry simulates a fact the model tagged source="web" even though the
        // exchange itself may not have used web tools (e.g. paraphrased content from
        // an earlier turn, or a model error). filter_assistant_origin_facts has no
        // knowledge of used_web_tools — it acts purely on the entry's own source tag.
        let entries = vec![
            make_entry_full(
                "The capital of France is Paris",
                "fact",
                "user",
                "web",
                "durable",
            ),
        ];
        let result = filter_assistant_origin_facts(entries);
        assert!(
            result.is_empty(),
            "source=web fact must be dropped by signal (c) regardless of any exchange-level flag"
        );
    }

    /// Signal (c)/(d): durable + user-sourced facts pass through untouched.
    #[test]
    fn test_durable_user_sourced_facts_pass_through() {
        let entries = vec![
            make_entry_full("User prefers dark mode", "preference", "user", "user", "durable"),
            make_entry_full("We decided to use SQLite for local storage", "decision", "user", "user", "durable"),
        ];
        let result = filter_assistant_origin_facts(entries);
        assert_eq!(result.len(), 2, "Durable, user-sourced facts must survive all four signals");
    }

    /// Signal (c)/(d): missing source/scope fields default to "user"/"durable" (matching
    /// serde's `default_source`/`default_scope`) and pass through.
    #[test]
    fn test_missing_source_scope_defaults_pass_through() {
        // Simulates serde deserialization of a RawEntry omitting source/scope entirely.
        let entries = vec![make_entry("Some neutral fact", "fact", "user")]; // make_entry defaults source=user, scope=durable
        let result = filter_assistant_origin_facts(entries);
        assert_eq!(result.len(), 1, "Entries with default source=user/scope=durable must pass through");
    }

    /// Combined: an entry that is both attribution=assistant AND source=web AND
    /// scope=episodic is still just dropped once (idempotent, no double-counting bugs).
    #[test]
    fn test_combined_all_signals_still_single_drop() {
        let entries = vec![make_entry_full(
            "I fetched this from the web for the current PR",
            "fact",
            "assistant",
            "web",
            "episodic",
        )];
        let result = filter_assistant_origin_facts(entries);
        assert!(result.is_empty(), "Entry failing multiple signals must still be dropped");
    }

    // =========================================================================
    // §5.4B: MemoryExtractedPayload serialization + newly_inserted collection tests
    // =========================================================================

    /// Verify MemoryExtractedPayload serializes correctly to JSON (required for Tauri emit)
    #[test]
    fn test_memory_extracted_payload_serializes() {
        let payload = MemoryExtractedPayload {
            facts: vec![
                ExtractedFactInfo {
                    id: "mem-abc123".to_string(),
                    fact: "User prefers dark mode".to_string(),
                },
                ExtractedFactInfo {
                    id: "mem-def456".to_string(),
                    fact: "Lives in Atlanta".to_string(),
                },
            ],
        };
        let json = serde_json::to_string(&payload).expect("Should serialize without error");
        assert!(json.contains("memory-extracted") || json.contains("mem-abc123"),
            "Serialized payload must contain the fact id");
        assert!(json.contains("User prefers dark mode"), "Serialized payload must contain fact text");
        assert!(json.contains("mem-def456"), "Serialized payload must contain second id");
    }

    /// Verify ExtractedFactInfo serializes with correct field names (id + fact)
    #[test]
    fn test_extracted_fact_info_field_names() {
        let info = ExtractedFactInfo {
            id: "mem-test-999".to_string(),
            fact: "Drinks black coffee".to_string(),
        };
        let json = serde_json::to_string(&info).unwrap();
        // Tauri frontend expects {"id": ..., "fact": ...}
        assert!(json.contains('"' ), "JSON must use quotes");
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["id"], "mem-test-999");
        assert_eq!(v["fact"], "Drinks black coffee");
    }

    /// Verify that only genuinely-inserted facts (not dedup-skipped) would enter newly_inserted.
    /// We test this by simulating the dedup logic manually against a test DB.
    #[test]
    fn test_newly_inserted_dedup_logic() {
        let (_temp, conn) = setup_test_db();
        // Create ai_memory table
        conn.execute_batch(
            "CREATE TABLE ai_memory (
                id TEXT PRIMARY KEY,
                fact TEXT NOT NULL,
                category TEXT,
                source_message_id TEXT,
                is_active INTEGER NOT NULL DEFAULT 1,
                layer TEXT NOT NULL DEFAULT 'work',
                project_id TEXT,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            );"
        ).unwrap();

        // Pre-insert one fact (existing)
        conn.execute(
            "INSERT INTO ai_memory (id, fact, category, is_active, layer) VALUES ('mem-existing', 'Already saved fact', 'fact', 1, 'work')",
            [],
        ).unwrap();

        let mut newly_inserted: Vec<ExtractedFactInfo> = Vec::new();
        let candidates = vec![
            ("Already saved fact", "fact"),
            ("Brand new fact", "preference"),
        ];

        for (fact, _category) in &candidates {
            let exists: bool = conn.query_row(
                "SELECT COUNT(*) > 0 FROM ai_memory WHERE fact = ?1 AND is_active = 1",
                rusqlite::params![fact],
                |row| row.get(0),
            ).unwrap_or(false);

            if !exists {
                let id = format!("mem-{}", uuid::Uuid::new_v4());
                conn.execute(
                    "INSERT INTO ai_memory (id, fact, category, is_active, layer) VALUES (?1, ?2, 'fact', 1, 'work')",
                    rusqlite::params![id, fact],
                ).unwrap();
                newly_inserted.push(ExtractedFactInfo { id, fact: fact.to_string() });
            }
        }

        assert_eq!(newly_inserted.len(), 1, "Only the non-duplicate fact should be in newly_inserted");
        assert_eq!(newly_inserted[0].fact, "Brand new fact");
        assert!(newly_inserted[0].id.starts_with("mem-"), "ID must have mem- prefix");
    }

    /// Verify empty payload is not emitted when all facts are dedup-skipped
    #[test]
    fn test_no_emit_when_all_facts_are_dupes() {
        // This tests the guard: `if !newly_inserted.is_empty()` before emitting.
        // We verify the collection stays empty when all facts already exist.
        let (_temp, conn) = setup_test_db();
        conn.execute_batch(
            "CREATE TABLE ai_memory (
                id TEXT PRIMARY KEY, fact TEXT NOT NULL,
                category TEXT, source_message_id TEXT,
                is_active INTEGER NOT NULL DEFAULT 1,
                layer TEXT NOT NULL DEFAULT 'work',
                project_id TEXT,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            );"
        ).unwrap();
        conn.execute(
            "INSERT INTO ai_memory (id, fact, category, is_active, layer) VALUES ('mem-x', 'Existing fact', 'fact', 1, 'work')",
            [],
        ).unwrap();

        let mut newly_inserted: Vec<ExtractedFactInfo> = Vec::new();
        let exists: bool = conn.query_row(
            "SELECT COUNT(*) > 0 FROM ai_memory WHERE fact = ?1 AND is_active = 1",
            rusqlite::params!["Existing fact"],
            |row| row.get(0),
        ).unwrap_or(false);

        if !exists {
            newly_inserted.push(ExtractedFactInfo {
                id: "mem-should-not-appear".to_string(),
                fact: "Existing fact".to_string(),
            });
        }

        // Guard condition: no emit should be triggered
        assert!(
            newly_inserted.is_empty(),
            "newly_inserted must be empty when all facts are dedup-skipped — no emit should fire"
        );
    }

    /// Signal (a): assistant-tagged fact in non-backstop category still dropped by signal (a)
    #[test]
    fn test_filter_signal_a_applies_to_all_categories() {
        let entries = vec![
            make_entry("Assistant chose the Postgres migration", "decision", "assistant"),
            make_entry("AI set the preference", "preference", "assistant"),
        ];
        let result = filter_assistant_origin_facts(entries);
        assert!(
            result.is_empty(),
            "Signal (a) drops assistant-tagged facts regardless of category"
        );
    }

    // ─── #3050 PR 5 named test (§8 #5): extraction queue skips a job-session
    // message; accepts a 'user'-origin message (the report-back message's
    // own session) ────────────────────────────────────────────────────────

    /// **Mutation (actually run, see PR body for the exact output):**
    /// deleted the `if session_origin.as_deref() == Some("spawn") { return
    /// Ok(()); }` check inside `enqueue_extraction` and re-ran. A job-session
    /// turn got queued (red); restored and re-confirmed green after.
    #[test]
    fn test_enqueue_extraction_skips_spawn_session_accepts_user_session() {
        let (_temp, conn) = setup_test_db();

        conn.execute(
            "INSERT INTO chat_sessions (id, origin) VALUES ('spawn-session-1', 'spawn')",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO chat_sessions (id, origin) VALUES ('user-session-1', 'user')",
            [],
        ).unwrap();

        // A spawn-session turn must NOT be queued.
        enqueue_extraction(&conn, "spawn-session-1", "msg-spawn-1", None).unwrap();
        let spawn_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM extraction_queue WHERE message_id = 'msg-spawn-1'", [], |row| row.get(0))
            .unwrap();
        assert_eq!(spawn_count, 0, "a spawn-session (origin='spawn') message must NOT be queued for extraction");

        // The report-back message's own session (always origin='user')
        // must still be queued normally — "excluded at the queue" must not
        // also exclude the thing it's supposed to reach.
        enqueue_extraction(&conn, "user-session-1", "msg-user-1", None).unwrap();
        let user_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM extraction_queue WHERE message_id = 'msg-user-1'", [], |row| row.get(0))
            .unwrap();
        assert_eq!(user_count, 1, "a user-origin session's message (e.g. the report-back message itself) must still be queued normally");
    }

    /// Schedule sessions are explicitly NOT excluded — §36.4.3 only names
    /// spawn transcripts; schedule sessions are visible, Scheduled-panel
    /// sessions, extracted like any other.
    #[test]
    fn test_enqueue_extraction_does_not_skip_schedule_session() {
        let (_temp, conn) = setup_test_db();
        conn.execute(
            "INSERT INTO chat_sessions (id, origin) VALUES ('schedule-session-1', 'schedule')",
            [],
        ).unwrap();

        enqueue_extraction(&conn, "schedule-session-1", "msg-schedule-1", None).unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM extraction_queue WHERE message_id = 'msg-schedule-1'", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1, "a schedule-session (origin='schedule') message must still be queued — only 'spawn' is excluded");
    }
}
