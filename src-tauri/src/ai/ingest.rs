//! Ingest: pulling the OpenClaw transcript into Pane's own chat history,
//! external-message insertion and tombstoning, agent sync/import from the
//! gateway, and the agent-roster integrity check. Split out of
//! `ai/commands.rs` (#3097).

use rusqlite::params;
use rusqlite::OptionalExtension;
use super::tools::{policy_error, atomic_write};
use super::connections::{fetch_oc_agent_model, ai_configure_and_save};
use crate::database::DbState;
use crate::filesystem_policy::{ExistingKind, PaneFs, PaneRoot};
use serde::{Deserialize, Serialize};
use tauri::State;


// === External Message Ingestion & Proactive Delivery (§30.11, #2410) ===
//
// spec-covered-by: #2687 (§30.11 proactive message delivery spec, merged 2026-08-23
// — touches docs/spec/sessions.md)
//
// Provider-neutral ingestion command (§30.11.1) + the OpenClaw transcript pull
// adapter (§30.11.3), the first (and for this slice, only) source adapter.
//
// INVARIANT (session-scoped, §30.11.1): an external-message insert never triggers
// dispatch. This module performs `chat_messages_local` writes only — it must never
// call `ai_chat_stream` or any streaming code path.

/// A single externally-sourced message to ingest into a Pane session's transcript.
/// Provider-neutral by construction — nothing here references OpenClaw, `chat.history`,
/// or any transport (§30.11.1). The OC transcript pull adapter is the first caller that
/// builds these; future source adapters (Hermes, a scheduler feed, ...) build the same
/// struct from their own transport.
#[derive(Debug, Clone, Deserialize)]
pub struct ExternalMessage {
    /// Pane `chat_sessions.id` this message lands in.
    pub target_session_id: String,
    /// Dedup key (§30.11.2) — OC transcript's `__openclaw.id` for the OC adapter;
    /// other adapters supply their own. Stored as `chat_messages_local.oc_message_id`.
    pub source_message_id: String,
    /// Matches `chat_messages_local.role`. Proactive messages are always "assistant"
    /// in practice this slice.
    pub role: String,
    pub content: String,
    /// RFC3339. #2690 (§30.11 ordering fix): this IS written to `created_at` on
    /// insert — normalized to SQLite's space-separated format and floor-clamped
    /// against the thread's max `created_at` first (see `stamp_origination_timestamp`
    /// / `insert_external_message`). Never bound raw.
    pub timestamp: String,
    /// Pane `ai_agents.id` — binding-check input (§30.11.5 / §F.1 of the build shape).
    pub source_agent_id: String,
    pub attachment_ids: Option<String>,
    pub tool_calls: Option<String>,
    pub tool_results: Option<String>,
    /// §7.2 of Reed's PR 5 scope audit: populated by report-back
    /// (`agent_jobs.id`) so the chip-vs-normal-bubble discriminator in
    /// `ChatPanel.svelte` can tell "Pane-templated chip" (model = NULL)
    /// apart from "report-back, render as a normal attributed bubble"
    /// (model = Some). The OC transcript pull adapter — the only other
    /// production caller — keeps its existing behavior unaffected by this
    /// new field.
    pub model: Option<String>,
    pub token_count: Option<i64>,
    /// §5.4/§6 (#3050 PR 5): set on report-back rows only (`agent_jobs.id`).
    /// `None` for every other caller — the OC transcript pull adapter
    /// passes `None` explicitly (compiler-forced: this is NOT `#[serde(default)]`,
    /// so a caller that omits it is a compile error, not a silent default).
    pub job_id: Option<String>,
}


/// Result of an `ingest_external_messages` batch call. Skip counts are split
/// (dedup vs. binding-mismatch) per Reed's recommendation — a single opaque
/// `skipped_count` makes the eventual telemetry/debugging story much harder.
#[derive(Debug, Clone, Serialize, Default, PartialEq)]
pub struct IngestResult {
    pub inserted_count: usize,
    /// Same `(thread_id, oc_message_id)` pair already present — `INSERT OR IGNORE`
    /// against `idx_chat_local_oc_dedup` was a no-op.
    pub dedup_skipped_count: usize,
    /// `chat_sessions.agent_id` for the target session did not match the message's
    /// claimed `source_agent_id` (§30.11.5 defense-in-depth check).
    pub binding_mismatch_count: usize,
    /// Per-message failure reasons (missing session, DB error, ...). Does not abort
    /// the batch — per-message skip-not-abort (§30.11.1).
    pub errors: Vec<String>,
}


/// Insert one externally-sourced message via the greeting pattern's `INSERT OR
/// IGNORE` variant (§B.3 Option 1 of the build shape doc) — direct DB write, no
/// dispatch, no stream, no context assembly. Deliberately does NOT reuse
/// `save_chat_message`'s `INSERT OR REPLACE` (that's keyed on the local `id`, which
/// this path mints fresh per call — REPLACE would never catch a same-content
/// replay; see §B.3). Dedup instead relies on `idx_chat_local_oc_dedup`, a partial
/// unique index on `(thread_id, oc_message_id)`.
///
/// Binding check (§30.11.5 / §F.1): looks up the target session's bound
/// `agent_id` and compares it to `msg.source_agent_id` BEFORE inserting. A
/// mismatch skips this message (caller increments `binding_mismatch_count`) —
/// this is defense-in-depth for future multi-source adapters; in this slice's
/// design the poll only ever runs against the one already-resolved session it
/// was scoped to, so a mismatch should be structurally unreachable, but the
/// check still runs because a future adapter *could* violate that invariant.
///
/// The inserted row's `agent_id` column is stamped from the session's own
/// binding (matches `save_chat_message`'s structural-attribution convention,
/// ADR-0062 D4) — not blindly copied from `msg.source_agent_id`, though by the
/// time this line runs the binding check already proved the two are equal.
///
/// Returns `Ok(true)` if a row was inserted, `Ok(false)` if the dedup index
/// silently absorbed a replay (0 rows affected), and `Err` only for actual
/// binding mismatches or DB errors — callers translate these into the three
/// `IngestResult` counters.
enum InsertOutcome {
    Inserted,
    DedupSkipped,
    BindingMismatch,
}


/// SQLite's own `datetime('now')` format (`YYYY-MM-DD HH:MM:SS`, space-separated,
/// no offset, whole-second precision) — matches every other writer of
/// `chat_messages_local.created_at` in this file. **Never** store raw RFC3339
/// directly into `created_at`: `T` (0x54) sorts after every digit character and
/// after the space (0x20) separator SQLite's own format uses at the same byte
/// position, so a raw-RFC3339 row would silently corrupt every plain-string
/// inequality comparison against this column (Rule 2's containment window,
/// compaction's cutoff) — confirmed empirically in
/// `docs/audits/2026-08-24-ordering-scope-2690.md` §2. This is the exact hazard
/// the test suite's own `sqlite_ts()` helper (several near-identical copies across
/// this file's test modules) already documents.
/// Sub-second precision is intentionally dropped — the `rowid` tiebreak
/// (`load_chat_messages`'s `ORDER BY created_at DESC, rowid DESC`) handles
/// same-second collisions.
fn sql_format_timestamp(dt: chrono::DateTime<chrono::Utc>) -> String {
    dt.format("%Y-%m-%d %H:%M:%S").to_string()
}


/// Origination-timestamp stamping + clock-skew floor clamp (#2690, Amendment 1 +
/// clock-skew guard, `docs/audits/2026-08-24-ordering-scope-2690.md` §2/§3).
///
/// Parses `msg.timestamp` (RFC3339, per `map_oc_message_to_external`'s derivation)
/// and normalizes to SQLite's own space-separated format via `sql_format_timestamp`
/// — never writes raw RFC3339 into `created_at`. A malformed/unparseable timestamp
/// falls back to `Utc::now()`, same degrade-gracefully posture as
/// `map_oc_message_to_external`'s own fallback.
///
/// Then clamps the result to never sort earlier than the thread's current max
/// `created_at` (`SELECT MAX(created_at) FROM chat_messages_local WHERE thread_id
/// = ?1`, including any just-saved Pane dispatch row) — this is what makes it
/// structurally impossible for OC clock skew to invert a user-message →
/// assistant-reply turn pair (§3 of the audit: without this clamp, a fast OC clock
/// could stamp a reply's origination time before the question that triggered it).
/// An empty thread (no prior rows) uses the origination timestamp as-is — no floor
/// to clamp against yet.
///
/// String comparison is safe here because both sides are already normalized to the
/// same SQL format — lexicographic order matches chronological order for that
/// format (confirmed by `sqlite_ts()`'s own doc comment and this audit's §2 repro).
fn stamp_origination_timestamp(conn: &rusqlite::Connection, thread_id: &str, raw_timestamp: &str) -> Result<String, String> {
    let origination_sql = chrono::DateTime::parse_from_rfc3339(raw_timestamp)
        .map(|dt| dt.with_timezone(&chrono::Utc))
        .unwrap_or_else(|e| {
            eprintln!(
                "[INGEST] stamp_origination_timestamp: could not parse timestamp '{}' ({}) — falling back to now()",
                raw_timestamp, e
            );
            chrono::Utc::now()
        });
    let origination_sql = sql_format_timestamp(origination_sql);

    let thread_max: Option<String> = conn
        .query_row(
            "SELECT MAX(created_at) FROM chat_messages_local WHERE thread_id = ?1",
            rusqlite::params![thread_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()
        .map_err(|e| format!("Failed to query thread max created_at for {}: {}", thread_id, e))?
        .flatten();

    match thread_max {
        Some(max) if max > origination_sql => {
            eprintln!(
                "[INGEST] Clock-skew clamp: thread {} origination timestamp '{}' is before thread max '{}' — clamping to thread max",
                thread_id, origination_sql, max
            );
            Ok(max)
        }
        _ => Ok(origination_sql),
    }
}


fn insert_external_message(
    conn: &rusqlite::Connection,
    msg: &ExternalMessage,
) -> Result<InsertOutcome, String> {
    // Binding check (§30.11.5): the session's bound agent_id must match the
    // message's claimed source_agent_id. A session with no row at all, or a NULL
    // agent_id (not yet self-healed), also counts as a mismatch — there is no
    // "unbound, accept anything" case for external ingestion.
    let session_agent_id: Option<String> = conn
        .query_row(
            "SELECT agent_id FROM chat_sessions WHERE id = ?1",
            rusqlite::params![&msg.target_session_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()
        .map_err(|e| format!("Failed to look up session {}: {}", msg.target_session_id, e))?
        .flatten();

    let Some(bound_agent_id) = session_agent_id else {
        eprintln!(
            "[INGEST] Binding mismatch: session {} has no bound agent (source_agent_id={})",
            msg.target_session_id, msg.source_agent_id
        );
        return Ok(InsertOutcome::BindingMismatch);
    };

    if bound_agent_id != msg.source_agent_id {
        eprintln!(
            "[INGEST] Binding mismatch: session {} is bound to agent {}, message claims source_agent_id={}",
            msg.target_session_id, bound_agent_id, msg.source_agent_id
        );
        return Ok(InsertOutcome::BindingMismatch);
    }

    let local_id = format!("ext-{}", uuid::Uuid::new_v4());

    // #2690 (§30.11 ordering fix, Amendment 1 + clock-skew guard): stamp
    // created_at from the message's own origination timestamp instead of
    // datetime('now') — this is what lets same-poll-tick batches render in true
    // send order instead of insert order. Normalized to SQL format + floor-
    // clamped against the thread's current max created_at BEFORE binding (see
    // stamp_origination_timestamp doc comment for both invariants).
    let created_at = stamp_origination_timestamp(conn, &msg.target_session_id, &msg.timestamp)?;

    // §30.11.2: INSERT OR IGNORE against idx_chat_local_oc_dedup
    // (thread_id, oc_message_id) WHERE oc_message_id IS NOT NULL. A re-ingested
    // (thread_id, source_message_id) pair is silently a no-op — rows_affected == 0
    // tells the caller it was a dedup hit, not a DB error.
    //
    // §30.11 does NOT bump chat_sessions.last_active_at (§E.2 of the build shape
    // — user messages don't bump it today either; matching that keeps the
    // column's semantics consistent). This is a single-table write.
    let rows = conn
        .execute(
            "INSERT OR IGNORE INTO chat_messages_local \
             (id, project_id, thread_id, role, content, created_at, attachment_ids, tool_calls, tool_results, model, token_count, agent_id, oc_message_id, job_id) \
             VALUES (?1, '', ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            rusqlite::params![
                local_id,
                msg.target_session_id,
                msg.role,
                msg.content,
                created_at,
                msg.attachment_ids,
                msg.tool_calls,
                msg.tool_results,
                msg.model,
                msg.token_count,
                bound_agent_id,
                msg.source_message_id,
                msg.job_id,
            ],
        )
        .map_err(|e| format!("Failed to insert external message: {}", e))?;

    if rows == 0 {
        eprintln!(
            "[INGEST] Dedup skip: thread_id={}, source_message_id={}",
            msg.target_session_id, msg.source_message_id
        );
        Ok(InsertOutcome::DedupSkipped)
    } else {
        eprintln!(
            "[INGEST] Inserted: id={}, thread_id={}, source_message_id={}, created_at={}",
            local_id, msg.target_session_id, msg.source_message_id, created_at
        );
        Ok(InsertOutcome::Inserted)
    }
}


/// Provider-neutral ingestion command (§30.11.1): `ingest_external_messages(messages)
/// → IngestResult`. Nothing in this signature references OpenClaw, `chat.history`, or
/// any transport — a future Hermes adapter or scheduler feed calls the identical
/// command with messages sourced differently.
///
/// Batch semantics are per-message skip-not-abort (§30.11.1): one message's failure
/// (missing session, dedup hit, binding mismatch, DB error) never blocks the rest of
/// the batch.
///
/// INVARIANT: this command is structurally incapable of triggering dispatch — it
/// only ever calls `insert_external_message`'s direct `INSERT OR IGNORE`. No
/// `ai_chat_stream`, no streaming code path, anywhere in this function.
///
/// Lock-free core — takes an already-held `Connection` so both the `#[tauri::command]`
/// wrapper (which owns the `State<DbState>` lock) and `pull_oc_transcript_for_session`
/// (which pre-reads gateway config, releases the lock across the HTTP call per the
/// #1180 convention, then re-locks) can share one implementation without either
/// re-acquiring a lock the other already holds.
pub(crate) fn ingest_external_messages_impl(
    messages: &[ExternalMessage],
    conn: &rusqlite::Connection,
) -> IngestResult {
    let mut result = IngestResult::default();

    // #2690 (§30.11 ordering fix): process the batch in origination-timestamp
    // order, not raw array order. `insert_external_message`'s floor clamp reads the
    // THREAD's live max created_at (including rows already inserted earlier in this
    // same batch) — that clamp exists to guard against genuine clock skew (a reply
    // stamped before its trigger message), not to interfere with same-batch
    // reordering. If the batch were processed in raw array order and that array
    // happened to be out of chronological order (the exact same-poll-tick scenario
    // this fix targets), an earlier-in-time message processed after a later-in-time
    // one would get its floor bumped up to the later message's timestamp —
    // collapsing the very ordering distinction this fix is meant to preserve.
    // Sorting first makes each subsequent floor clamp monotonically consistent with
    // true send order within the batch; `sort_by` is stable, so array order is
    // preserved as the tiebreak for identical/unparseable timestamps.
    let mut ordered: Vec<&ExternalMessage> = messages.iter().collect();
    ordered.sort_by_key(|msg| {
        chrono::DateTime::parse_from_rfc3339(&msg.timestamp)
            .map(|dt| dt.with_timezone(&chrono::Utc))
            .unwrap_or_else(|_| chrono::Utc::now())
    });

    for msg in ordered {
        match insert_external_message(conn, msg) {
            Ok(InsertOutcome::Inserted) => result.inserted_count += 1,
            Ok(InsertOutcome::DedupSkipped) => result.dedup_skipped_count += 1,
            Ok(InsertOutcome::BindingMismatch) => result.binding_mismatch_count += 1,
            Err(e) => {
                eprintln!(
                    "[INGEST] Per-message failure (target_session_id={}, source_message_id={}): {}",
                    msg.target_session_id, msg.source_message_id, e
                );
                result.errors.push(format!(
                    "{} (target_session_id={}, source_message_id={})",
                    e, msg.target_session_id, msg.source_message_id
                ));
            }
        }
    }

    eprintln!(
        "[INGEST] Done: inserted={}, dedup_skipped={}, binding_mismatch={}, errors={}",
        result.inserted_count, result.dedup_skipped_count, result.binding_mismatch_count, result.errors.len()
    );

    result
}


#[tauri::command]
pub fn ingest_external_messages(
    messages: Vec<ExternalMessage>,
    db: State<DbState>,
) -> Result<IngestResult, String> {
    eprintln!(
        "[INGEST] ingest_external_messages called with {} message(s)",
        messages.len()
    );

    let conn = db.0.lock().map_err(|e| format!("Failed to lock database: {}", e))?;
    Ok(ingest_external_messages_impl(&messages, &conn))
}


/// #2738 — shared, read-only gateway-context resolution for the own-turn
/// tombstoning helpers below (`fetch_oc_latest_seq`, `tombstone_own_turn`).
/// Deliberately a SEPARATE, smaller helper rather than a refactor of
/// `pull_oc_transcript_for_session`'s inline resolution block: that function is
/// the primary, heavily-tested ingest adapter (§30.11.3 and every test in
/// `ingest_external_messages_tests`), and duplicating this ~20-line lookup here
/// keeps this PR's diff isolated to the own-turn-tombstoning concern without
/// touching a code path #2410/#2725/#2730 already depend on line-for-line.
///
/// Mirrors (read-only, never mutates) that function's own resolution order:
/// runtime-type eligibility gate → `runtime_ref` lookup → gateway URL → gateway
/// token → optional TLS cert. Returns `Ok(None)` for every ineligible/unconfigured
/// case — never `Err` for these — because own-turn tombstoning is inherently
/// best-effort defense-in-depth layered on top of the existing content-based echo
/// checks (`CONTENT_LENGTH_FLOOR`, `is_recent_local_assistant_echo`): a failure to
/// resolve gateway context here must never abort or alter `ai_chat_stream`'s own
/// response to its caller, only skip the extra tombstone pass for that dispatch.
// §36.3.4 (#3050 PR 2): widened from module-private to `pub(crate)` so the
// scheduler's OC-trigger-only send (`ai::scheduler::scheduler_tick`) can reuse
// this exact resolution instead of duplicating it — same runtime-type gate →
// runtime_ref → gateway URL → token → cert order, same "Ok(None), never Err,
// for every ineligible/unconfigured case" contract the doc comment above
// already describes. No behavior change to either existing caller
// (`fetch_oc_latest_seq`/`tombstone_own_turn`), which still see the identical
// private-in-effect type from within this module.
pub(crate) struct OcGatewayContext {
    pub(crate) runtime_ref: String,
    pub(crate) gateway_url: String,
    pub(crate) token: String,
    pub(crate) cert_pem: Option<String>,
}


pub(crate) fn resolve_oc_gateway_context(
    conn: &rusqlite::Connection,
    session_id: &str,
) -> Result<Option<OcGatewayContext>, String> {
    if !session_uses_openclaw_runtime(conn, session_id)? {
        return Ok(None);
    }

    let runtime_ref: Option<String> = conn
        .query_row(
            "SELECT ai_agents.runtime_ref FROM chat_sessions \
             JOIN ai_agents ON ai_agents.id = chat_sessions.agent_id \
             WHERE chat_sessions.id = ?1",
            rusqlite::params![session_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()
        .map_err(|e| format!("Failed to look up runtime_ref for session {}: {}", session_id, e))?
        .flatten();
    let Some(runtime_ref) = runtime_ref else {
        return Ok(None);
    };

    let Some(gateway_url) = crate::sync_client::get_gateway_url(conn) else {
        return Ok(None);
    };

    let Some(token) = crate::sync_client::get_gateway_token(conn) else {
        return Ok(None);
    };

    let cert_pem = crate::database::get_ai_setting(conn, "gateway_tls_cert_pem");

    Ok(Some(OcGatewayContext {
        runtime_ref,
        gateway_url,
        token,
        cert_pem,
    }))
}


/// #2738 — one raw OC transcript message's identity + ordering fields, pulled
/// straight from `__openclaw` metadata without the full `ExternalMessage` mapping
/// (`map_oc_message_to_external` also filters non-assistant/empty-content rows,
/// which the tombstone pass below needs to do with its OWN role check so it can
/// log skip counts distinctly — see `tombstone_own_turn`).
fn extract_oc_seq(raw: &serde_json::Value) -> Option<(String, i64)> {
    let oc_meta = raw.get("__openclaw")?;
    let id = oc_meta.get("id").and_then(|v| v.as_str())?.to_string();
    let seq = oc_meta.get("seq").and_then(|v| v.as_i64())?;
    Some((id, seq))
}


/// #2738 — records the pre-dispatch `seq` baseline for a session's OC transcript.
/// Called once, immediately before `ai_chat_stream`'s spawned task issues its
/// first outbound OC call. Issues a single lightweight `chat.history?limit=1`
/// fetch (via the gateway's `GET /v1/sessions/:id/messages` route — the exact
/// same endpoint `pull_oc_transcript_for_session` uses, just without any of that
/// function's ingestion/mapping machinery) and returns the newest message's
/// `__openclaw.seq`, or `None` if gateway context can't be resolved, the request
/// fails, or the session has no transcript history yet (fresh session, seq
/// effectively starts below anything OC will ever assign — `None` is handled by
/// the caller as "skip the own-turn tombstone pass for this dispatch," which is
/// safe: the existing content-based echo checks remain the fallback).
///
/// Rotation (#2738 Task 1, exception #3 of Reed's audit): the OC session **key**
/// (`agent:{runtime_ref}:openai-user:pane-{session_id}`) is derived fresh from
/// `runtime_ref`/`session_id` on every call, exactly like
/// `pull_oc_transcript_for_session` — this function never observes or caches OC's
/// own internal, rotatable `sessionId` at all (only the gateway's
/// `session_mappings` table does, server-side). Because the SAME re-derivation
/// happens again, independently, when `tombstone_own_turn` runs post-`done:true`,
/// a rotation that occurs mid-turn is transparent to both calls: each resolves
/// the gateway's CURRENT session state for the stable key at the moment it runs,
/// never carrying forward a stale snapshot.
pub(crate) async fn fetch_oc_latest_seq(session_id: &str, db: &DbState) -> Option<i64> {
    let ctx = {
        let conn = db.0.lock().ok()?;
        resolve_oc_gateway_context(&conn, session_id).ok().flatten()
    }?;

    let client = crate::gateway_client::build_gateway_client_with_cert(ctx.cert_pem).ok()?;
    let oc_session_key = format!("agent:{}:openai-user:pane-{}", ctx.runtime_ref, session_id);
    let base = ctx.gateway_url.trim_end_matches('/').trim_end_matches("/v1");
    let url = format!("{}/v1/sessions/{}/messages?limit=1", base, urlencoding::encode(&oc_session_key));

    let response = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", ctx.token))
        .send()
        .await
        .ok()?;

    if !response.status().is_success() {
        eprintln!(
            "[OC-OWN-TURN] Baseline fetch for session {} returned HTTP {} — skipping own-turn tombstone pass for this dispatch",
            session_id, response.status()
        );
        return None;
    }

    let body: serde_json::Value = response.json().await.ok()?;
    let raw_messages = body.get("messages").and_then(|v| v.as_array()).cloned().unwrap_or_default();

    let baseline = raw_messages.iter().filter_map(extract_oc_seq).map(|(_, seq)| seq).max();

    eprintln!(
        "[OC-OWN-TURN] Recorded baseline seq={:?} for session {} (key={})",
        baseline, session_id, oc_session_key
    );

    baseline
}


/// #2738 — post-`done:true` own-turn tombstone pass. Called from all three
/// `ai_chat_stream` terminal emit sites (success, reconnect-budget-exhausted
/// error, iteration-cap/timeout fallback — Reed's audit, Task 3) with the
/// baseline `fetch_oc_latest_seq` recorded at dispatch start. Re-fetches
/// `chat.history` (same `limit=20` page `pull_oc_transcript_for_session` polls)
/// and, for every assistant-role message whose `__openclaw.seq` is STRICTLY
/// greater than `baseline_seq`, writes a `chat_oc_tombstones` row via the
/// existing `write_tombstone` helper with `reason="own_turn"` — before the
/// frontend's next poll tick can re-ingest this dispatch's own short replies as
/// if they were duplicate proactive messages (the #2738 bug: a 4-char reply like
/// "Gold" is too short to be caught by `CONTENT_LENGTH_FLOOR`).
///
/// Best-effort by design (matches `pull_oc_transcript_for_session`'s own
/// "graceful skip, never panic" contract for OC-adjacent code): any failure to
/// resolve gateway context, reach the gateway, or parse its response is logged
/// and swallowed — this pass NEVER returns an `Err` that could propagate into
/// `ai_chat_stream`'s own terminal emit path. A dispatch whose tombstone pass
/// fails simply falls back on the pre-existing content-based echo checks for
/// that one turn's own messages, same as before this fix shipped. Returns the
/// count of rows tombstoned (0 on any failure or no qualifying messages).
pub(crate) async fn tombstone_own_turn(session_id: &str, db: &DbState, baseline_seq: i64) -> usize {
    let ctx = {
        let conn = match db.0.lock() {
            Ok(c) => c,
            Err(e) => {
                eprintln!("[OC-OWN-TURN] Failed to lock database for tombstone pass on session {}: {}", session_id, e);
                return 0;
            }
        };
        match resolve_oc_gateway_context(&conn, session_id) {
            Ok(Some(ctx)) => ctx,
            Ok(None) => return 0,
            Err(e) => {
                eprintln!("[OC-OWN-TURN] Failed to resolve gateway context for session {}: {}", session_id, e);
                return 0;
            }
        }
    };

    let client = match crate::gateway_client::build_gateway_client_with_cert(ctx.cert_pem) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[OC-OWN-TURN] Failed to build gateway client for session {}: {} — skipping tombstone pass", session_id, e);
            return 0;
        }
    };
    let oc_session_key = format!("agent:{}:openai-user:pane-{}", ctx.runtime_ref, session_id);
    let base = ctx.gateway_url.trim_end_matches('/').trim_end_matches("/v1");
    let url = format!("{}/v1/sessions/{}/messages?limit=20", base, urlencoding::encode(&oc_session_key));

    let response = match client
        .get(&url)
        .header("Authorization", format!("Bearer {}", ctx.token))
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[OC-OWN-TURN] Transport error fetching tombstone-pass history for session {}: {} — skipping", session_id, e);
            return 0;
        }
    };

    if !response.status().is_success() {
        eprintln!(
            "[OC-OWN-TURN] Tombstone-pass fetch for session {} returned HTTP {} — skipping",
            session_id, response.status()
        );
        return 0;
    }

    let body: serde_json::Value = match response.json().await {
        Ok(b) => b,
        Err(e) => {
            eprintln!("[OC-OWN-TURN] Failed to parse tombstone-pass response for session {}: {} — skipping", session_id, e);
            return 0;
        }
    };
    let raw_messages = body.get("messages").and_then(|v| v.as_array()).cloned().unwrap_or_default();

    let conn = match db.0.lock() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[OC-OWN-TURN] Failed to lock database to write tombstones for session {}: {}", session_id, e);
            return 0;
        }
    };

    let mut tombstoned = 0usize;
    for raw in &raw_messages {
        let role = raw.get("role").and_then(|v| v.as_str()).unwrap_or("");
        if role != "assistant" {
            continue;
        }
        let Some((oc_message_id, seq)) = extract_oc_seq(raw) else {
            continue;
        };
        if seq <= baseline_seq {
            continue;
        }
        let content_preview = raw
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap_or("<non-string content>");
        if let Err(e) = write_tombstone(&conn, session_id, &oc_message_id, "own_turn", content_preview, None, None) {
            eprintln!("[OC-OWN-TURN] Failed to write own_turn tombstone for thread {} oc_msg_id={}: {}", session_id, oc_message_id, e);
            continue;
        }
        tombstoned += 1;
    }

    eprintln!(
        "[OC-OWN-TURN] Tombstone pass for session {}: baseline_seq={} → {} own-turn message(s) tombstoned",
        session_id, baseline_seq, tombstoned
    );

    tombstoned
}


/// §30.11.5 / §C.3 of the build shape: read-only helper — poll eligibility is
/// "session bound to a `runtime_type = 'openclaw'` agent", nothing more. Deliberately
/// does NOT call `check_bound_agent_active` (agents.rs) — that guards *dispatch*
/// (`ai_chat_stream`) and errors on a locked/inactive bound agent. Per ADR-0062
/// Amendment 2026-08-23 ("append-in ≠ dispatch-out"), a locked agent's session must
/// still poll and still receive proactive messages; only dispatch stays refused.
/// This helper has no active-agent gate by design.
pub fn session_uses_openclaw_runtime(
    conn: &rusqlite::Connection,
    session_id: &str,
) -> Result<bool, String> {
    let runtime_type: Option<String> = conn
        .query_row(
            "SELECT ai_agents.runtime_type FROM chat_sessions \
             JOIN ai_agents ON ai_agents.id = chat_sessions.agent_id \
             WHERE chat_sessions.id = ?1",
            rusqlite::params![session_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()
        .map_err(|e| format!("Failed to look up runtime_type for session {}: {}", session_id, e))?
        .flatten();

    Ok(runtime_type.as_deref() == Some("openclaw"))
}


/// Result of a single OC transcript pull cycle — how many messages were fetched
/// from the gateway vs. handed to `ingest_external_messages`, plus the ingest
/// outcome itself. `None` for `ingest` means the pull cycle was skipped cleanly
/// (not OC runtime, gateway not configured, or a graceful 502/transport-error
/// skip — §30.11.3) — never a panic, never an error that would crash the caller.
#[derive(Debug, Clone, Serialize, Default)]
pub struct OcPullResult {
    pub fetched_count: usize,
    pub ingest: Option<IngestResult>,
    /// Human-readable reason the cycle was skipped (absent gateway config, non-OC
    /// runtime, 502/transport error, ...). `None` when messages were fetched.
    pub skip_reason: Option<String>,
    /// Echo suppression backstop (#2410, §30.11.4 PR 4, Candidate 3) — **log-only as
    /// of #3053 item F.** Count of mapped OC messages whose content matched (was a
    /// substring of) this thread's most recent local assistant row within the
    /// content-match window — see `is_recent_local_assistant_echo`. Prior to #3053
    /// item F, a match here DROPPED the message before `ingest_external_messages` ever
    /// saw it, and wrote a permanent `chat_oc_tombstones` row (`reason='backstop_echo'`).
    /// Text-containment silently destroyed real, user-visible content this way (a
    /// 2026-10-20 incident: a genuine cron result was dropped because an earlier
    /// assistant reply happened to quote it) — dedup is now id (`is_already_classified`)
    /// and seq (`tombstone_own_turn`) ONLY. A match is still counted here and logged
    /// (`[OC-PULL]` diagnostic line) purely for observability; it no longer removes the
    /// message from the batch, and no tombstone is written for it. See §30.11.10 of
    /// `docs/spec/sessions.md` for the full rationale and the migration that clears
    /// pre-existing `backstop_echo`/`stale_echo` tombstones written before this fix.
    pub content_match_skipped_count: usize,
    /// Stateful echo suppression — tombstoning (#2410, Rule 1 §2c of
    /// `docs/audits/2026-08-24-tombstone-scope.md`): count of mapped OC messages
    /// dropped by the pre-filter BEFORE the backstop or stale-echo check ever ran,
    /// because `(thread_id, oc_message_id)` was already permanently settled — either
    /// a real row already exists in `chat_messages_local`, or it was already
    /// tombstoned on a prior tick. Runs UNCONDITIONALLY, including on
    /// `suppress_ingest=true` ticks (§3d — only ever removes already-settled
    /// candidates, never a message that would otherwise have been legitimately
    /// deferred).
    pub already_classified_count: usize,
    /// Stateful echo suppression (#2410, refined Rule 2 §6 of the tombstone-scope
    /// audit addendum; dead band closed by #2725) — **log-only as of #3053 item F**,
    /// same change and same rationale as `content_match_skipped_count` above. Count of
    /// first-sight messages, of ANY age, that had content long enough to trust (≥ the
    /// length floor) AND matched an existing local assistant row within ±5min of the
    /// message's own claimed timestamp. Prior to #3053 item F this TOMBSTONED the
    /// message (`reason='stale_echo'`) and dropped it before backstop/ingest; it now
    /// only logs the match (`[OC-PULL]` diagnostic line) and lets the message proceed—
    /// dedup is id/seq only. Runs UNCONDITIONALLY, same rationale as
    /// `already_classified_count`.
    pub stale_echo_tombstoned_count: usize,
}


/// §30.11.3: the OpenClaw transcript pull adapter — the first (and for this slice,
/// only) `ingest_external_messages` source adapter. Not a Tauri command itself (per
/// §H.2 of the build shape, this is a plain Rust function; a thin command wrapper for
/// the frontend poll ships in PR 3).
///
/// 1. Resolves the OC session **key** fresh on every call — composite form
///    `agent:{runtime_ref}:openai-user:pane-{session_id}` (§30.11.3, verified
///    2026-08-23). Never caches or reuses a previously-observed OC `sessionId`;
///    rotation is a live possibility on any out-of-turn poll (§A.4 of the build
///    shape) and a cached id is exactly the value that goes stale.
/// 2. Calls the gateway's `GET /v1/sessions/:pane_session_id/messages?limit=20`
///    route (fixed by the prerequisite PR, #2686) with NO `since`/`before` cursor
///    — over-fetches a small recent page and lets §30.11.2 dedup discard the
///    already-seen (§H.2: OC has no native since-by-message-id cursor).
/// 3. Maps each returned OC message into an `ExternalMessage`, pulling
///    `__openclaw.id` for `source_message_id`. If absent on a given message,
///    inserts with `source_message_id` left as a locally-synthesized value that
///    intentionally cannot collide with a real OC id, so dedup protection is
///    effectively skipped for that one message (§B.4) rather than fabricating a
///    composite key that could itself collide.
/// 4. Calls `ingest_external_messages` with the mapped batch.
///
/// Graceful 502 / transport-error skip (§30.11.3): any network error or non-2xx
/// gateway response logs and returns `Ok(OcPullResult { skip_reason: Some(..), .. })`
/// — never a panic, never an `Err` that would propagate and crash the caller. The
/// next poll tick simply tries again.
///
/// `suppress_ingest` (#2410, §30.11.4 PR 4, Candidate 2 — echo suppression, dispatch
/// grace window): when `true`, the gateway GET call and message mapping still run in
/// full (so `fetched_count` stays accurate and the caller gets real telemetry), but
/// the mapped batch is NEVER handed to `ingest_external_messages_impl` — `ingest`
/// comes back `None` with a `skip_reason` explaining the suppression. This is the
/// "defer, don't skip" contract the frontend poll relies on (`ChatSidebar.svelte`'s
/// grace-window check): because this adapter has no `since`/cursor state (§30.11.3),
/// suppressing ingestion on one tick costs nothing — the exact same messages are
/// simply re-fetched and normally ingested on the next eligible (non-suppressed)
/// tick. A genuine cron/proactive message that lands during another dispatch's grace
/// window is therefore delayed, never dropped.
pub async fn pull_oc_transcript_for_session(
    session_id: String,
    db: &DbState,
    suppress_ingest: bool,
) -> Result<OcPullResult, String> {
    eprintln!("[OC-PULL] pull_oc_transcript_for_session called: session_id={}", session_id);

    // Resolve runtime_ref + gateway config + eligibility, all read-only, while
    // holding the DB lock only as long as needed (never across the HTTP call —
    // matches sync_client.rs's "pre-read then release" convention, #1180).
    let (runtime_ref, gateway_url, token, cert_pem) = {
        let conn = db.0.lock().map_err(|e| format!("Failed to lock database: {}", e))?;

        if !session_uses_openclaw_runtime(&conn, &session_id)? {
            eprintln!("[OC-PULL] Skipping: session {} is not bound to an openclaw-runtime agent", session_id);
            return Ok(OcPullResult {
                skip_reason: Some("session not bound to an openclaw-runtime agent".to_string()),
                ..Default::default()
            });
        }

        let runtime_ref: Option<String> = conn
            .query_row(
                "SELECT ai_agents.runtime_ref FROM chat_sessions \
                 JOIN ai_agents ON ai_agents.id = chat_sessions.agent_id \
                 WHERE chat_sessions.id = ?1",
                rusqlite::params![&session_id],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()
            .map_err(|e| format!("Failed to look up runtime_ref for session {}: {}", session_id, e))?
            .flatten();

        let Some(runtime_ref) = runtime_ref else {
            eprintln!("[OC-PULL] Skipping: session {} has no runtime_ref on its bound agent", session_id);
            return Ok(OcPullResult {
                skip_reason: Some("bound agent has no runtime_ref".to_string()),
                ..Default::default()
            });
        };

        let Some(gateway_url) = crate::sync_client::get_gateway_url(&conn) else {
            eprintln!("[OC-PULL] Skipping: no active gateway URL configured");
            return Ok(OcPullResult {
                skip_reason: Some("no active gateway URL configured".to_string()),
                ..Default::default()
            });
        };

        let Some(token) = crate::sync_client::get_gateway_token(&conn) else {
            eprintln!("[OC-PULL] Skipping: no gateway token configured");
            return Ok(OcPullResult {
                skip_reason: Some("no gateway token configured".to_string()),
                ..Default::default()
            });
        };

        let cert_pem = crate::database::get_ai_setting(&conn, "gateway_tls_cert_pem");

        (runtime_ref, gateway_url, token, cert_pem)
    };

    // §30.11.3: the composite key, re-derived fresh every call — never
    // cached. #3129 (C28(b)) retired the job-session special case this
    // used to branch on (a lookup against `agent_jobs.oc_session_key`,
    // the bootstrap-returned key `ensure_oc_session_bootstrapped` used to
    // write) — a job session is never an OC-side session at all once it
    // runs through `run_agent_loop`; `select_oc_pickup_eligible_sessions`
    // (`ai::scheduler`) now excludes `origin IN ('spawn', 'schedule')`
    // sessions from this function's caller entirely, so a job session
    // never reaches this formula in production — this is simply the one
    // formula every OC-bound chat session (the only kind that still
    // reaches here) has always used.
    let oc_session_key = format!("agent:{}:openai-user:pane-{}", runtime_ref, session_id);

    let client = match crate::gateway_client::build_gateway_client_with_cert(cert_pem) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[OC-PULL] Failed to build gateway client: {} — skipping cycle", e);
            return Ok(OcPullResult {
                skip_reason: Some(format!("gateway client build failed: {}", e)),
                ..Default::default()
            });
        }
    };

    let base = gateway_url.trim_end_matches('/').trim_end_matches("/v1");
    let url = format!("{}/v1/sessions/{}/messages?limit=20", base, urlencoding::encode(&oc_session_key));

    let response = match client
        .get(&url)
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            // Transport error (connection refused, timeout, DNS, ...) — graceful skip.
            eprintln!("[OC-PULL] Transport error calling {}: {} — skipping cycle", url, e);
            return Ok(OcPullResult {
                skip_reason: Some(format!("transport error: {}", e)),
                ..Default::default()
            });
        }
    };

    if response.status() == reqwest::StatusCode::BAD_GATEWAY {
        eprintln!("[OC-PULL] Gateway returned 502 for session {} — skipping cycle gracefully", session_id);
        return Ok(OcPullResult {
            skip_reason: Some("gateway returned 502".to_string()),
            ..Default::default()
        });
    }

    if !response.status().is_success() {
        let status = response.status();
        eprintln!("[OC-PULL] Gateway returned HTTP {} for session {} — skipping cycle gracefully", status, session_id);
        return Ok(OcPullResult {
            skip_reason: Some(format!("gateway returned HTTP {}", status)),
            ..Default::default()
        });
    }

    let body: serde_json::Value = match response.json().await {
        Ok(b) => b,
        Err(e) => {
            eprintln!("[OC-PULL] Failed to parse gateway response as JSON: {} — skipping cycle", e);
            return Ok(OcPullResult {
                skip_reason: Some(format!("invalid JSON response: {}", e)),
                ..Default::default()
            });
        }
    };

    let raw_messages = body
        .get("messages")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    eprintln!("[OC-PULL] Fetched {} raw message(s) for session {} (key={})", raw_messages.len(), session_id, oc_session_key);

    // §2410 ingestion filters: first-ever-pull baseline check. Read-only, pre-read
    // under the DB lock, matches the pre-read-then-release convention already used
    // above and below in this function (#1180). `oc_baseline_done_at IS NULL` means
    // this session has never been polled before — every message OC just returned is
    // pre-existing history Pane already has (the session's first 20-message page),
    // not new proactive output, so it must be marked "seen" without being inserted.
    let baseline_done_at: Option<String> = {
        let conn = db.0.lock().map_err(|e| format!("Failed to lock database: {}", e))?;
        conn.query_row(
            "SELECT oc_baseline_done_at FROM chat_sessions WHERE id = ?1",
            rusqlite::params![&session_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()
        .map_err(|e| format!("Failed to look up oc_baseline_done_at for session {}: {}", session_id, e))?
        .flatten()
    };

    if baseline_done_at.is_none() {
        let conn = db.0.lock().map_err(|e| format!("Failed to lock database: {}", e))?;
        conn.execute(
            "UPDATE chat_sessions SET oc_baseline_done_at = datetime('now') WHERE id = ?1",
            rusqlite::params![&session_id],
        )
        .map_err(|e| format!("Failed to stamp oc_baseline_done_at for session {}: {}", session_id, e))?;
        eprintln!(
            "[OC-PULL] First-ever pull for session {} — baselining {} message(s), none ingested",
            session_id, raw_messages.len()
        );
        return Ok(OcPullResult {
            fetched_count: raw_messages.len(),
            ingest: None,
            skip_reason: Some(format!(
                "first pull: baselined {} message(s), none ingested",
                raw_messages.len()
            )),
            content_match_skipped_count: 0,
            already_classified_count: 0,
            stale_echo_tombstoned_count: 0,
        });
    }

    // Resolve source_agent_id once — the agent this poll was made against IS the
    // session's bound agent (this adapter runs for one already-resolved session;
    // §F.1 of the build shape: "no ambiguity, consistent with the structural-invariant
    // argument").
    let source_agent_id: Option<String> = {
        let conn = db.0.lock().map_err(|e| format!("Failed to lock database: {}", e))?;
        conn.query_row(
            "SELECT agent_id FROM chat_sessions WHERE id = ?1",
            rusqlite::params![&session_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()
        .map_err(|e| format!("Failed to look up session agent_id: {}", e))?
        .flatten()
    };

    let Some(source_agent_id) = source_agent_id else {
        eprintln!("[OC-PULL] Skipping: session {} has no bound agent_id (cannot stamp source_agent_id)", session_id);
        return Ok(OcPullResult {
            fetched_count: raw_messages.len(),
            skip_reason: Some("session has no bound agent_id".to_string()),
            ..Default::default()
        });
    };

    let mapped: Vec<ExternalMessage> = raw_messages
        .iter()
        .filter_map(|m| map_oc_message_to_external(m, &session_id, &source_agent_id))
        .collect();

    eprintln!("[OC-PULL] Mapped {} of {} raw message(s) into ExternalMessage batch", mapped.len(), raw_messages.len());

    // Stateful echo suppression — tombstoning (#2410, Rule 1 §2c of
    // docs/audits/2026-08-24-tombstone-scope.md): permanence pre-filter. Runs
    // UNCONDITIONALLY — before suppress_ingest and before any content-comparison
    // logic — on every tick, including `suppress_ingest=true` ticks (§3d: this only
    // ever removes candidates already permanently settled one way or the other, so
    // it never violates "defer, don't skip"). This is what makes classification
    // permanent: once a message is tombstoned (or already a real row), no future
    // tick's drifted "most recent local row" reference can give it a different
    // answer, because it never reaches that comparison again.
    let mut already_classified_count = 0usize;
    let after_prefilter: Vec<ExternalMessage> = {
        let conn = db.0.lock().map_err(|e| format!("Failed to lock database: {}", e))?;
        let mut out = Vec::with_capacity(mapped.len());
        for msg in mapped {
            match is_already_classified(&conn, &msg.target_session_id, &msg.source_message_id) {
                Ok(true) => {
                    eprintln!(
                        "[OC-PULL] Tombstone pre-filter: already classified — dropping thread={} oc_msg_id={}",
                        msg.target_session_id, msg.source_message_id
                    );
                    already_classified_count += 1;
                }
                Ok(false) => out.push(msg),
                Err(e) => {
                    // Defensive: a DB error on the pre-filter's own lookup must never block
                    // the message — fail open, log, let it proceed to the remaining checks
                    // (each of which has its own independent protection).
                    eprintln!("[OC-PULL] Tombstone pre-filter check failed (non-fatal, message proceeds): {}", e);
                    out.push(msg);
                }
            }
        }
        out
    };

    // Stateful echo suppression — refined Rule 2 (#2410, §6 addendum of the
    // tombstone-scope audit) — **log-only as of #3053 item F.** A content-containment
    // match used to tombstone (`reason='stale_echo'`) and drop the message here,
    // unconditionally. That silently destroyed real content: a later, genuinely new
    // message whose text happens to be contained in an existing local assistant row
    // (a reply that quotes/restates it is ordinary behavior, not an edge case) was
    // indistinguishable from an actual echo by text alone. Dedup is now id
    // (`is_already_classified`, above) and seq (`tombstone_own_turn`) ONLY — the
    // containment check still runs and is still logged (useful diagnostic signal for
    // tracing suspected duplicates) but NEVER removes a message from the batch and
    // NEVER writes a tombstone for it. `stale_echo` is no longer written by any code
    // path; see §30.11.10 of docs/spec/sessions.md for the migration that clears
    // pre-existing rows with this reason.
    let mut stale_echo_tombstoned_count = 0usize;
    let after_stale_check: Vec<ExternalMessage> = {
        let conn = db.0.lock().map_err(|e| format!("Failed to lock database: {}", e))?;
        let mut out = Vec::with_capacity(after_prefilter.len());
        for msg in after_prefilter {
            match check_stale_echo(&conn, &msg.target_session_id, &msg.content, &msg.timestamp) {
                Ok(Some((matched_local_created_at, age_secs))) => {
                    eprintln!(
                        "[OC-PULL] check_stale_echo: text-containment match observed (log-only, #3053 item F — NOT tombstoned, message proceeds) thread={} oc_msg_id={} matched_local_created_at={} age_secs={:.1}",
                        msg.target_session_id, msg.source_message_id, matched_local_created_at, age_secs
                    );
                    stale_echo_tombstoned_count += 1;
                    out.push(msg);
                }
                Ok(None) => out.push(msg),
                Err(e) => {
                    // Defensive: a DB error on the stale-echo check must never block the
                    // message — fail toward inserting (§5 of the audit: "fails toward
                    // showing a genuine message over hiding one").
                    eprintln!("[OC-PULL] Stale-echo check failed (non-fatal, message proceeds): {}", e);
                    out.push(msg);
                }
            }
        }
        out
    };

    // Echo suppression (#2410, §30.11.4 PR 4, Candidate 2 — primary mechanism): a poll
    // tick landing inside the frontend's post-dispatch grace window sets
    // `suppress_ingest=true`. "Defer, don't skip" (task constraint: must NOT drop
    // genuine cron/proactive messages): the gateway fetch above still ran in full
    // (`fetched_count` stays accurate), but the surviving batch (post pre-filter and
    // stale-echo check, both of which already ran unconditionally above) is withheld
    // from ingestion entirely rather than being ingested now. Because this adapter
    // has NO `since`/cursor state (§30.11.3 — always re-fetches the same recent
    // page), deferring costs nothing: the identical messages are simply re-fetched
    // and normally ingested on the next eligible (non-suppressed) tick.
    if suppress_ingest {
        eprintln!(
            "[OC-PULL] Suppressing ingest for session {} — {} mapped message(s) deferred to next eligible tick (dispatch grace window)",
            session_id, after_stale_check.len()
        );
        return Ok(OcPullResult {
            fetched_count: raw_messages.len(),
            ingest: None,
            skip_reason: Some(format!(
                "ingest suppressed: dispatch grace window active, {} message(s) deferred to next tick",
                after_stale_check.len()
            )),
            content_match_skipped_count: 0,
            already_classified_count,
            stale_echo_tombstoned_count,
        });
    }

    // Content-match backstop (#2410, §30.11.4 PR 4, Candidate 3) — **log-only as of
    // #3053 item F**, same change and same rationale as the stale-echo check above.
    // A hit used to drop the message here and write a permanent `backstop_echo`
    // tombstone (#2410 tombstoning addendum, §2b of the tombstone-scope audit) so the
    // same `(thread_id, oc_message_id)` could never be re-evaluated by a future tick's
    // drifted "most recent local row" reference. That protected against a real but
    // narrower problem (OC's own tool loop re-wrapping segments of a turn Pane
    // already saved) at the cost of a broader, worse one: ANY genuinely new message
    // whose text happens to be a substring of a recent local assistant row — a quote,
    // a restated summary, a repeated boilerplate line — got silently destroyed with
    // only a log line as evidence (the 2026-10-20 seq-154 incident). The containment
    // check still runs and is still logged for diagnosis, but it no longer removes a
    // message from the batch or writes a tombstone. `backstop_echo` is no longer
    // written by any code path; §30.11.10 of docs/spec/sessions.md documents the
    // migration that clears pre-existing rows with this reason.
    let mut content_match_skipped_count = 0usize;
    let filtered_mapped: Vec<ExternalMessage> = {
        let conn = db.0.lock().map_err(|e| format!("Failed to lock database: {}", e))?;
        let mut out = Vec::with_capacity(after_stale_check.len());
        for msg in after_stale_check {
            match is_recent_local_assistant_echo(&conn, &msg.target_session_id, &msg.content) {
                Ok(true) => {
                    eprintln!(
                        "[OC-PULL] Content-match backstop: text-containment match observed (log-only, #3053 item F — NOT tombstoned, message proceeds) thread={} oc_msg_id={}",
                        msg.target_session_id, msg.source_message_id
                    );
                    content_match_skipped_count += 1;
                    out.push(msg);
                }
                Ok(false) => out.push(msg),
                Err(e) => {
                    // Defensive: a DB error on the backstop's own lookup must never block
                    // the message from reaching ingestion (which has its own independent
                    // dedup protection) — fail open, log, and let it through.
                    eprintln!("[OC-PULL] Content-match check failed (non-fatal, message proceeds to ingest): {}", e);
                    out.push(msg);
                }
            }
        }
        out
    };

    let ingest_result = {
        let conn = db.0.lock().map_err(|e| format!("Failed to lock database: {}", e))?;
        ingest_external_messages_impl(&filtered_mapped, &conn)
    };

    Ok(OcPullResult {
        fetched_count: raw_messages.len(),
        ingest: Some(ingest_result),
        skip_reason: None,
        content_match_skipped_count,
        already_classified_count,
        stale_echo_tombstoned_count,
    })
}


/// Echo suppression backstop (#2410, §30.11.4 PR 4, Candidate 3) — content-match check
/// against this thread's most recent LOCAL assistant row, scoped to a short trailing
/// time window.
///
/// **#3053 item F (2026-10-21) — this function's return value is now a LOG-ONLY
/// diagnostic signal, never a drop/tombstone decision.** The caller
/// (`pull_oc_transcript_for_session`) logs a `true` result and increments a counter
/// for observability, but no longer removes the message from the batch and no longer
/// writes a `backstop_echo` tombstone for it — dedup is id (`is_already_classified`)
/// and seq (`tombstone_own_turn`) only. This function's own containment logic is
/// UNCHANGED (kept exactly as documented below, since it's still useful for tracing
/// suspected duplicates in logs) — only the caller's REACTION to a `true` result
/// changed. Rationale: a substring match here is not proof of an echo, only
/// correlation — ordinary behavior (a reply that quotes or restates prior content)
/// produces the identical signal, and treating it as a deletion trigger silently
/// destroyed real, user-visible messages (2026-10-20 incident: a genuine cron result
/// dropped because an earlier assistant reply happened to quote it verbatim). See
/// §30.11.10 of `docs/spec/sessions.md`.
///
/// **Pre-step finding (2026-08-24, required before implementation — correlating the
/// `f16e623a` transcript's 7-segment turn against OC's server-side request/tool-loop
/// shape):** confirmed via `tools.rs::get_pane_tools()` that Pane's own client-side
/// tool registry does NOT include the `exec` tool OC's internal agent used for that
/// turn. That means the 7 transcript segments were produced by a SINGLE Pane
/// `ai_chat_stream` HTTP request/SSE connection, whose completion OC's own internal
/// (server-side) agent tool loop expanded into multiple transcript entries before
/// returning — NOT by 7 separate Pane-side loop iterations each making their own HTTP
/// call. (Pane's own iteration loop, `run_agent_loop`, only fires for
/// Pane-DECLARED tools requiring CLIENT-side execution; `exec` is not one of them, so
/// it can never have driven that loop.) Because `stream_openrouter_inner`'s SSE
/// parser (`openrouter.rs:693-815`) accumulates every `delta.content` chunk from that
/// one connection into a single `full_text`/`accumulated_text` buffer
/// (`run_agent_loop`'s `accumulated_text.push_str(&final_text)`), Pane's ONE
/// locally-saved row ends up as a CONCATENATION of all of OC's intermediate + final
/// text segments for the turn — not an exact copy of any single segment. **This is
/// why the match strategy below is SUBSTRING (`local_row.content.contains(incoming)`),
/// not exact-equality:** an intermediate segment's text is a proper substring of the
/// local row's full concatenated content, and the check only needs to hold in that
/// one direction (incoming segment ⊆ local row), never the reverse.
///
/// Scope (tight, per the task's rotation-safety / must-not-drop-genuine-messages
/// constraints):
/// - Only the SINGLE most recent local assistant row for `thread_id` is considered
///   (`ORDER BY created_at DESC LIMIT 1`) — no unbounded lookback.
/// - That row must have been written within the last `CONTENT_MATCH_WINDOW_SECS`
///   (150s — covers the frontend's 90s post-`done` grace period, §30.11.4 PR 4, plus
///   margin for a poll tick landing right at the window's edge). A local assistant
///   row older than that can never suppress a new incoming message, no matter how
///   similar the content — this is what keeps a short/canned genuine proactive reply
///   from being wrongly suppressed by unrelated older history.
fn is_recent_local_assistant_echo(
    conn: &rusqlite::Connection,
    thread_id: &str,
    incoming_content: &str,
) -> Result<bool, String> {
    const CONTENT_MATCH_WINDOW_SECS: i64 = 150;
    // Mirrors `check_stale_echo`'s (Rule 2) identically-named/identically-valued
    // constant (commands.rs, §6.4 mitigation 1) — same rationale applies here: short
    // content (e.g. a 4-char cron delivery like "Blue") is too easily a spurious
    // substring match of ANY sufficiently long unrelated local row, so it's too
    // short to trust the backstop's containment check at all. #2730.
    const CONTENT_LENGTH_FLOOR: usize = 20;

    // Empty incoming content would trivially be a substring of anything —
    // map_oc_message_to_external's empty-content guard already filters these out
    // before this function is ever called, but guard here too for defense-in-depth
    // against any future direct caller.
    if incoming_content.trim().is_empty() {
        return Ok(false);
    }

    // #2730: content below the length floor is too short for the substring-match
    // backstop to trust — fail toward inserting (§5's stated posture) rather than
    // risk tombstoning a genuine short message as a false-positive substring hit.
    if incoming_content.trim().len() < CONTENT_LENGTH_FLOOR {
        eprintln!(
            "[OC-PULL] is_recent_local_assistant_echo: content below length floor ({} chars) for thread {} — skipping backstop content-match check, not suppressing",
            incoming_content.trim().len(), thread_id
        );
        return Ok(false);
    }

    let recent_local_content: Option<String> = conn
        .query_row(
            "SELECT content FROM chat_messages_local \
             WHERE thread_id = ?1 AND role = 'assistant' \
               AND created_at >= datetime('now', ?2) \
             ORDER BY created_at DESC LIMIT 1",
            rusqlite::params![thread_id, format!("-{} seconds", CONTENT_MATCH_WINDOW_SECS)],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|e| format!("Failed to look up recent local assistant row for thread {}: {}", thread_id, e))?;

    let Some(local_content) = recent_local_content else {
        return Ok(false);
    };

    Ok(local_content.contains(incoming_content))
}


/// Stateful echo suppression — tombstoning (#2410, Rule 1 §2c of
/// `docs/audits/2026-08-24-tombstone-scope.md`): the permanence pre-filter. Decides
/// whether `(thread_id, oc_message_id)` has ALREADY been classified, one way or the
/// other — either it's already a real row in `chat_messages_local` (a prior
/// ingest), or it was already tombstoned on a prior tick (a prior echo/stale-echo
/// verdict). A `true` result means the caller must drop this message from the batch
/// immediately, before it ever reaches the stale-echo check or the backstop's
/// content-match comparison — this is what makes classification permanent: once a
/// message is settled, no future tick's drifted "most recent local row" reference
/// can ever re-evaluate it and give a different answer.
///
/// Mirrors the exact query shape from §2c of the audit (a single `EXISTS OR EXISTS`
/// round-trip) rather than two separate queries — cheaper, and the two clauses are
/// logically inseparable from the caller's point of view (either one means "already
/// settled").
fn is_already_classified(
    conn: &rusqlite::Connection,
    thread_id: &str,
    oc_message_id: &str,
) -> Result<bool, String> {
    conn.query_row(
        "SELECT EXISTS(\
             SELECT 1 FROM chat_messages_local \
             WHERE thread_id = ?1 AND oc_message_id = ?2\
         ) OR EXISTS(\
             SELECT 1 FROM chat_oc_tombstones \
             WHERE thread_id = ?1 AND oc_message_id = ?2\
         )",
        rusqlite::params![thread_id, oc_message_id],
        |row| row.get::<_, bool>(0),
    )
    .map_err(|e| format!("Failed to check already-classified status for thread {} / oc_message_id {}: {}", thread_id, oc_message_id, e))
}


/// `STALE_THRESHOLD_SECS` (15 minutes / 900s, §3c of the tombstone-scope audit) —
/// no longer gates whether `check_stale_echo`'s containment check RUNS (#2725 closed
/// that dead band, see the function's own doc comment), but is retained as the
/// dividing line used to label a tombstone's catch-class in observability: a
/// `stale_echo` hit with `age_secs < STALE_THRESHOLD_SECS` is a "dead-band catch"
/// (the 150s–15min gap this hotfix closes); `age_secs >= STALE_THRESHOLD_SECS` is a
/// "resurrection catch" (the >15min case Rule 2 always covered). Also still used by
/// `is_recent_local_assistant_echo`'s doc comment as the historical reference point
/// for why the backstop's own 150s window was originally sized the way it was.
const STALE_THRESHOLD_SECS: i64 = 900;


/// Stateful echo suppression — refined Rule 2 (#2410, §6 addendum of the
/// tombstone-scope audit; dead band closed by #2725, see
/// `docs/audits/2026-08-26-echo-dead-band-scope.md`): content-containment check run
/// on EVERY first-sight message, regardless of age.
///
/// **#3053 item F (2026-10-21) — a `Some(..)` result is now a LOG-ONLY diagnostic
/// signal, never a drop/tombstone decision.** This function's own containment logic
/// below is UNCHANGED — only the CALLER's reaction changed: `pull_oc_transcript_
/// for_session` logs a match and counts it for observability, but no longer removes
/// the message from the batch and no longer writes a `stale_echo` tombstone. Dedup is
/// id (`is_already_classified`) and seq (`tombstone_own_turn`) only now. A text match
/// is correlation, not proof of an echo — a later message that genuinely quotes or
/// restates earlier content (ordinary behavior) produces the identical signal, and
/// using it as a deletion trigger silently destroyed real content (the seq-154
/// incident this fix responds to). See §30.11.10 of `docs/spec/sessions.md` for the
/// full rationale and the migration clearing pre-existing `stale_echo`/`backstop_echo`
/// tombstone rows.
///
/// Sequence (§6.1, amended by #2725):
/// - **Step A — age computation (observability only, no longer a gate):** computes
///   `msg_timestamp`'s age in seconds relative to wall-clock "now" for use in the
///   tombstone reason string (`STALE_THRESHOLD_SECS` still marks the dividing line
///   between a "dead-band catch" — age < 900s, the 150s–15min gap this hotfix closes
///   — and a "resurrection catch" — age >= 900s, the case Rule 2 always covered).
///   Prior to #2725, this step ALSO gated whether Steps B/C ran at all (`if age_secs
///   < STALE_THRESHOLD_SECS { return Ok(None); }`) — per the #2725 scope audit's (a)
///   subsumption analysis, that early-return was the sole cause of the dead band: a
///   first-sight message aged strictly between 150s (the backstop's own window,
///   `is_recent_local_assistant_echo`) and 900s satisfied neither gate's entry
///   condition and inserted unconditionally, bypassing all content comparison. The
///   gate is removed; Steps B/C now always run. A malformed/unparseable
///   `msg_timestamp` still fails toward inserting (Rule 2 skipped for that message),
///   matching this file's existing defensive-coding convention.
/// - **Step B — length floor:** is `incoming_content.trim().len() < 20`? If YES,
///   the content is too short to trust containment (§6.4 mitigation 1: short
///   boilerplate like "Done." false-matches too easily at unbounded lookback) —
///   returns `Ok(None)` (fail toward inserting, §5's stated posture). Unaffected by
///   #2725 — this step's logic never depended on the message being stale.
/// - **Step C — content-containment check, ±5min anchor:** scans local assistant
///   rows within ±5 minutes of the incoming message's OWN claimed `timestamp` (not
///   wall-clock "now" — §6.4 mitigation 2, avoids reopening the original
///   stateless-drift bug since the anchor never moves once the message is fetched).
///   A match returns `Ok(Some(matched_row_created_at))` (tombstone as `stale_echo`).
///   No match returns `Ok(None)` — genuine content (of any age) proceeds to
///   backstop/ingest (§6.5, §30.11.4 compliance). Unaffected by #2725.
///
/// The 150s backstop (`is_recent_local_assistant_echo`) is kept exactly as-is
/// alongside this check per the #2725 scope audit's (a) recommendation (ratified):
/// the anchor check subsumes the backstop for all practically observed cases but is
/// not a strict mathematical superset (now-anchored + trailing-only vs.
/// message-timestamp-anchored + symmetric), so the backstop remains belt-and-
/// suspenders at negligible added cost (one cheap indexed single-row query).
///
/// Returns `Err` only for a DB error on the lookups themselves — callers fail open
/// (message proceeds) per this file's existing defensive-coding convention.
fn check_stale_echo(
    conn: &rusqlite::Connection,
    thread_id: &str,
    incoming_content: &str,
    msg_timestamp: &str,
) -> Result<Option<(String, f64)>, String> {
    const CONTAINMENT_ANCHOR_WINDOW_MINUTES: i64 = 5; // ±5min, §6.4 mitigation 2
    const CONTENT_LENGTH_FLOOR: usize = 20; // §6.4 mitigation 1

    // Step A: age computation, observability-only (#2725 — no longer a gate on
    // whether Steps B/C run). `msg_timestamp` is RFC3339
    // (map_oc_message_to_external's derivation, this file) — SQLite's datetime() functions parse RFC3339
    // natively, no manual parsing needed.
    let age_secs: Option<f64> = conn
        .query_row(
            "SELECT (julianday('now') - julianday(?1)) * 86400.0",
            rusqlite::params![msg_timestamp],
            |row| row.get::<_, Option<f64>>(0),
        )
        .map_err(|e| format!("Failed to compute message age for thread {}: {}", thread_id, e))?;

    let Some(age_secs) = age_secs else {
        // Malformed timestamp — SQLite's julianday() returns NULL rather than erroring.
        // Fail toward inserting: treat as fresh (Rule 2 skipped), same posture as any
        // other malformed-input case in this file.
        eprintln!("[OC-PULL] check_stale_echo: could not parse msg_timestamp '{}' — treating as fresh, Rule 2 skipped", msg_timestamp);
        return Ok(None);
    };

    // Step B: length floor.
    if incoming_content.trim().len() < CONTENT_LENGTH_FLOOR {
        eprintln!(
            "[OC-PULL] check_stale_echo: content below length floor ({} chars) for message in thread {} (age={:.1}s) — skipping containment check, Rule 2 skipped",
            incoming_content.trim().len(), thread_id, age_secs
        );
        return Ok(None);
    }

    // Step C: content-containment check, ±5min anchor on the message's OWN timestamp
    // (not wall-clock "now" — §6.4 mitigation 2). Fetch candidate rows ordered
    // newest-first (matches the backstop's own convention) and check containment
    // Rust-side, mirroring the existing backstop's `local_content.contains(incoming)`
    // direction (`check_stale_echo`, this file).
    let mut stmt = conn
        .prepare(
            "SELECT content, created_at FROM chat_messages_local \
             WHERE thread_id = ?1 \
               AND role = 'assistant' \
               AND created_at >= datetime(?2, ?3) \
               AND created_at <= datetime(?2, ?4) \
             ORDER BY created_at DESC",
        )
        .map_err(|e| format!("Failed to prepare stale-echo containment query for thread {}: {}", thread_id, e))?;

    let rows: Vec<(String, String)> = stmt
        .query_map(
            rusqlite::params![
                thread_id,
                msg_timestamp,
                format!("-{} minutes", CONTAINMENT_ANCHOR_WINDOW_MINUTES),
                format!("+{} minutes", CONTAINMENT_ANCHOR_WINDOW_MINUTES),
            ],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .map_err(|e| format!("Failed to run stale-echo containment query for thread {}: {}", thread_id, e))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("Failed to collect stale-echo containment rows for thread {}: {}", thread_id, e))?;

    for (local_content, local_created_at) in rows {
        if local_content.contains(incoming_content) {
            return Ok(Some((local_created_at, age_secs)));
        }
    }

    // No match — genuine content of any age proceeds (§6.5).
    Ok(None)
}


/// Stateful echo suppression — tombstoning (#2410, §2b of the tombstone-scope
/// audit): writes a permanent classification row. `INSERT OR IGNORE` against the
/// table's `(thread_id, oc_message_id)` primary key — idempotent by construction, a
/// re-write for an already-tombstoned id (which shouldn't normally happen, since the
/// pre-filter would have caught it first) is a silent no-op, not an error.
///
/// **#3053 item F (2026-10-21):** this helper remains generic and still backs the
/// seq-based `own_turn` reason (`tombstone_own_turn`, correct and unaffected by this
/// change). As of this fix, no live code path calls it with `reason="stale_echo"` or
/// `reason="backstop_echo"` any more — both text-containment checks (`check_stale_
/// echo`, `is_recent_local_assistant_echo`) are log-only now (see their own doc
/// comments). Pre-existing rows with those two reasons, written before this fix
/// shipped, are cleared by migration v92 (`migrate_v92`, `migrations.rs`) — see
/// §30.11.10 of `docs/spec/sessions.md`.
///
/// Observability (task requirement): every tombstone write is logged at a visible
/// level (`eprintln!`), matching the existing `[OC-PULL...]` logging style used
/// throughout this file. For `reason='stale_echo'`, the matched local row's
/// `created_at` is also logged (`matched_local_created_at`) — makes any false
/// positive immediately traceable from terminal output (§6.4 mitigation 3).
///
/// `age_secs` (#2725): when the caller has a message age available (the stale-check
/// loop's `stale_echo` arm always does, via `check_stale_echo`'s returned age), it's
/// logged alongside `matched_local_created_at` so a dead-band catch (age < 900s /
/// `STALE_THRESHOLD_SECS`) is distinguishable from a resurrection catch (age >= 900s)
/// directly from terminal output, without needing to cross-reference timestamps by
/// hand. `None` (the backstop's `backstop_echo` call site, which never computes an
/// age) omits the field entirely rather than fabricating one.
fn write_tombstone(
    conn: &rusqlite::Connection,
    thread_id: &str,
    oc_message_id: &str,
    reason: &str,
    content_preview_source: &str,
    matched_local_created_at: Option<&str>,
    age_secs: Option<f64>,
) -> Result<(), String> {
    conn.execute(
        "INSERT OR IGNORE INTO chat_oc_tombstones (thread_id, oc_message_id, reason) VALUES (?1, ?2, ?3)",
        rusqlite::params![thread_id, oc_message_id, reason],
    )
    .map_err(|e| format!("Failed to write tombstone for thread {} / oc_message_id {} (reason={}): {}", thread_id, oc_message_id, reason, e))?;

    let preview: String = content_preview_source.chars().take(80).collect();
    let age_suffix = match age_secs {
        Some(secs) => {
            let catch_class = if secs < STALE_THRESHOLD_SECS as f64 { "dead_band" } else { "resurrection" };
            format!(" age_secs={:.1} catch_class={}", secs, catch_class)
        }
        None => String::new(),
    };
    match matched_local_created_at {
        Some(created_at) => eprintln!(
            "[OC-PULL-TOMBSTONE] thread={} oc_msg_id={} reason={} content_preview=\"{}\" matched_local_created_at={}{}",
            thread_id, oc_message_id, reason, preview, created_at, age_suffix
        ),
        None => eprintln!(
            "[OC-PULL-TOMBSTONE] thread={} oc_msg_id={} reason={} content_preview=\"{}\"{}",
            thread_id, oc_message_id, reason, preview, age_suffix
        ),
    }

    Ok(())
}


/// Thin Tauri command wrapper (§30.11.4/PR3, #2410) — the frontend's 60s poll
/// (`ChatSidebar.svelte`) invokes this directly. All real logic lives in
/// `pull_oc_transcript_for_session`, which already handles key derivation,
/// eligibility gating, the gateway call, ingestion, dedup, and graceful
/// 502/transport-error skip. This wrapper exists only because
/// `pull_oc_transcript_for_session` takes `&DbState` (shared with future non-Tauri
/// callers, per its own doc comment) while `#[tauri::command]` needs a
/// `State<'_, DbState>` parameter — same thin-wrapper shape as every other
/// `db: &DbState`-taking helper in this file (e.g. `fetch_internal_provider_models`
/// wrapping `get_internal_provider`).
///
/// `suppress_ingest` (#2410, §30.11.4 PR 4): threaded straight through from the
/// frontend's per-thread grace-window check (`ChatSidebar.svelte`, via
/// `streamManager.isStreamingThread`/`getLastDoneAt`). `Option<bool>` (rather than a
/// bare `bool`) so any existing caller/test invoking this command without the new
/// arg still deserializes cleanly — `None` is treated identically to `Some(false)`,
/// the pre-#2410-PR4 steady-state (never suppressed) behavior.
#[tauri::command]
pub async fn pull_oc_transcript_for_session_cmd(
    session_id: String,
    suppress_ingest: Option<bool>,
    db: State<'_, DbState>,
) -> Result<OcPullResult, String> {
    let suppress_ingest = suppress_ingest.unwrap_or(false);
    eprintln!(
        "[OC-PULL-CMD] pull_oc_transcript_for_session_cmd called: session_id={}, suppress_ingest={}",
        session_id, suppress_ingest
    );
    pull_oc_transcript_for_session(session_id, &db, suppress_ingest).await
}


/// Thin Tauri command wrapper (§30.11.4/PR3, #2410) around `session_uses_openclaw_runtime`
/// — the frontend checks this once per session-open/session-switch (not every poll
/// tick, per ADR-0062 D1: the binding is immutable for the life of a session) to gate
/// whether the 60s poll should run at all.
#[tauri::command]
pub fn session_uses_openclaw_runtime_cmd(
    session_id: String,
    db: State<'_, DbState>,
) -> Result<bool, String> {
    eprintln!("[OC-PULL-CMD] session_uses_openclaw_runtime_cmd called: session_id={}", session_id);
    let conn = db.0.lock().map_err(|e| format!("Failed to lock database: {}", e))?;
    session_uses_openclaw_runtime(&conn, &session_id)
}


/// Maps a single raw OC `chat.history` message (as returned by the gateway's
/// `GET /v1/sessions/:id/messages` route) into an `ExternalMessage`. Returns `None`
/// if the raw value is missing required fields (`role`/`content`) — defensive
/// per SOUL.md §5 ("don't assume gateway responses are well-formed"); that one
/// message is dropped rather than the whole batch failing.
///
/// `__openclaw.id` (§B.1 of the build shape — internal, unversioned OC transcript
/// metadata, not part of the documented `chat.history` response contract) supplies
/// `source_message_id` when present. When absent, a locally-synthesized id
/// (`no-oc-id-{uuid}`) is used instead — guaranteed never to collide with a real
/// OC id or a replay of this same message, which means dedup protection is
/// effectively skipped for that message (§B.4: "accepting the small risk of a
/// duplicate on retry" is explicitly acceptable for this slice).
fn map_oc_message_to_external(
    raw: &serde_json::Value,
    session_id: &str,
    source_agent_id: &str,
) -> Option<ExternalMessage> {
    let role = raw.get("role").and_then(|v| v.as_str())?.to_string();

    // §2410 ingestion filters (role guard): only assistant-role turns are new AI
    // output worth ingesting. `user`-role messages from OC's chat.history are
    // often cron/system-payload echoes rather than anything Matt typed in Pane's
    // UI, and `toolResult` messages are raw tool-output blobs — neither belongs
    // in chat_messages_local as if it were a real conversational turn.
    if role != "assistant" {
        return None;
    }

    // OC's chat.history content is an array of content blocks (§22C.6.5.6); this
    // slice only needs a flattened text representation — tool-call-bearing
    // proactive messages are an explicit follow-up (§F.2 of the build shape).
    let content = match raw.get("content") {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(serde_json::Value::Array(blocks)) => blocks
            .iter()
            .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => return None,
    };

    // §2410 ingestion filters (empty-content guard): a pure-toolCall assistant turn
    // (content blocks are all `toolCall`/`thinking`, no `text` block) flattens to an
    // empty string above rather than hitting the `_ => return None` arm — without this
    // guard it would insert as a blank row.
    if content.trim().is_empty() {
        return None;
    }

    let source_message_id = raw
        .get("__openclaw")
        .and_then(|m| m.get("id"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("no-oc-id-{}", uuid::Uuid::new_v4()));

    // OC's timestamp is a numeric epoch (ms); fall back to "now" (ISO 8601) if
    // absent/malformed rather than dropping the message.
    let timestamp = raw
        .get("timestamp")
        .and_then(|v| v.as_i64())
        .and_then(|ms| chrono::DateTime::from_timestamp_millis(ms))
        .map(|dt| dt.to_rfc3339())
        .unwrap_or_else(|| chrono::Utc::now().to_rfc3339());

    Some(ExternalMessage {
        target_session_id: session_id.to_string(),
        source_message_id,
        role,
        content,
        timestamp,
        source_agent_id: source_agent_id.to_string(),
        attachment_ids: None,
        tool_calls: None,
        tool_results: None,
        model: None,
        token_count: None,
        // §5.4 (#3050 PR 5): the OC transcript pull adapter never carries a
        // Pane job id — report-back (`ai::scheduler`) is the only producer.
        job_id: None,
    })
}


#[cfg(test)]
mod ingest_external_messages_tests {
    use super::*;
    use crate::keychain::test_support::KeychainGuard;
    use serial_test::serial;

    /// Real production schema via `run_migrations` — not a hand-rolled fixture. SOUL.md
    /// §3's "production preconditions" requirement for state-mutating functions.
    fn make_conn() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().expect("in-memory db");
        crate::migrations::run_migrations(&conn).expect("fresh-install migrations should succeed");
        conn
    }

    fn seed_agent(conn: &rusqlite::Connection, id: &str, runtime_type: Option<&str>, runtime_ref: Option<&str>) {
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO ai_agents (id, name, slug, is_default, is_active, workspace_path, runtime_type, runtime_ref, model, created_at, updated_at)
             VALUES (?1, ?1, ?1, 0, 1, ?1, ?2, ?3, 'test-model', ?4, ?4)",
            rusqlite::params![id, runtime_type, runtime_ref, now],
        )
        .expect("seed agent");
    }

    fn seed_session(conn: &rusqlite::Connection, id: &str, agent_id: &str) {
        conn.execute(
            "INSERT INTO chat_sessions (id, project_id, agent_id) VALUES (?1, 'workspace', ?2)",
            rusqlite::params![id, agent_id],
        )
        .expect("seed session");
    }

    fn make_external_message(session_id: &str, source_message_id: &str, source_agent_id: &str) -> ExternalMessage {
        ExternalMessage {
            target_session_id: session_id.to_string(),
            source_message_id: source_message_id.to_string(),
            role: "assistant".to_string(),
            content: "proactive hello".to_string(),
            timestamp: chrono::Utc::now().to_rfc3339(),
            source_agent_id: source_agent_id.to_string(),
            attachment_ids: None,
            tool_calls: None,
            tool_results: None,
            model: None,
            token_count: None,
            job_id: None,
        }
    }

    // ── 1. Dedup idempotency ──────────────────────────────────────────────────

    #[test]
    fn test_dedup_idempotency_reingesting_same_batch_inserts_zero_second_time() {
        let conn = make_conn();
        seed_agent(&conn, "agent-1", Some("openclaw"), Some("main"));
        seed_session(&conn, "session-1", "agent-1");

        let batch = vec![
            make_external_message("session-1", "oc-msg-1", "agent-1"),
            make_external_message("session-1", "oc-msg-2", "agent-1"),
            make_external_message("session-1", "oc-msg-3", "agent-1"),
        ];

        let first = ingest_external_messages_impl(&batch, &conn);
        assert_eq!(first.inserted_count, 3, "first ingest should insert all 3 messages: {:?}", first);
        assert_eq!(first.dedup_skipped_count, 0);
        assert_eq!(first.binding_mismatch_count, 0);
        assert!(first.errors.is_empty(), "no errors expected: {:?}", first.errors);

        let second = ingest_external_messages_impl(&batch, &conn);
        assert_eq!(second.inserted_count, 0, "re-ingesting the same batch must insert zero rows: {:?}", second);
        assert_eq!(second.dedup_skipped_count, 3, "all 3 must be reported as dedup hits: {:?}", second);
        assert_eq!(second.binding_mismatch_count, 0);

        let row_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM chat_messages_local WHERE thread_id = 'session-1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(row_count, 3, "exactly 3 rows must exist after both ingests — no duplicates");
    }

    // ── 2. Binding-mismatch rejection ───────────────────────────────────────────

    #[test]
    fn test_binding_mismatch_rejects_and_does_not_insert() {
        let conn = make_conn();
        seed_agent(&conn, "agent-bound", Some("openclaw"), Some("main"));
        seed_agent(&conn, "agent-other", Some("openclaw"), Some("other"));
        seed_session(&conn, "session-1", "agent-bound");

        // Message claims source_agent_id = agent-other, but session-1 is bound to agent-bound.
        let batch = vec![make_external_message("session-1", "oc-msg-1", "agent-other")];

        let result = ingest_external_messages_impl(&batch, &conn);
        assert_eq!(result.binding_mismatch_count, 1, "mismatched source_agent_id must be counted: {:?}", result);
        assert_eq!(result.inserted_count, 0, "mismatched message must NOT insert: {:?}", result);
        assert_eq!(result.dedup_skipped_count, 0);

        let row_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM chat_messages_local WHERE thread_id = 'session-1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(row_count, 0, "no row should have been written for a binding mismatch");
    }

    // ── 3. Per-message skip-not-abort ───────────────────────────────────────────

    #[test]
    fn test_per_message_skip_not_abort_binding_mismatch_does_not_block_batch() {
        let conn = make_conn();
        seed_agent(&conn, "agent-bound", Some("openclaw"), Some("main"));
        seed_agent(&conn, "agent-other", Some("openclaw"), Some("other"));
        seed_session(&conn, "session-1", "agent-bound");

        let batch = vec![
            make_external_message("session-1", "oc-msg-1", "agent-bound"),
            make_external_message("session-1", "oc-msg-2", "agent-other"), // mismatch
            make_external_message("session-1", "oc-msg-3", "agent-bound"),
        ];

        let result = ingest_external_messages_impl(&batch, &conn);
        assert_eq!(result.inserted_count, 2, "messages 1 and 3 must still insert despite message 2's mismatch: {:?}", result);
        assert_eq!(result.binding_mismatch_count, 1, "message 2 must be counted as a mismatch: {:?}", result);

        let ids: Vec<String> = {
            let mut stmt = conn
                .prepare("SELECT oc_message_id FROM chat_messages_local WHERE thread_id = 'session-1' ORDER BY oc_message_id")
                .unwrap();
            stmt.query_map([], |r| r.get::<_, String>(0)).unwrap().map(|r| r.unwrap()).collect()
        };
        assert_eq!(ids, vec!["oc-msg-1".to_string(), "oc-msg-3".to_string()], "only the non-mismatched messages should be present");
    }

    // ── 4. session_uses_openclaw_runtime ────────────────────────────────────────

    #[test]
    fn test_session_uses_openclaw_runtime_true_for_openclaw_agent() {
        let conn = make_conn();
        seed_agent(&conn, "agent-oc", Some("openclaw"), Some("main"));
        seed_session(&conn, "session-oc", "agent-oc");

        let result = session_uses_openclaw_runtime(&conn, "session-oc").expect("query should succeed");
        assert!(result, "session bound to an openclaw-runtime agent must report true");
    }

    #[test]
    fn test_session_uses_openclaw_runtime_false_for_openrouter_agent() {
        let conn = make_conn();
        seed_agent(&conn, "agent-or", Some("openrouter"), None);
        seed_session(&conn, "session-or", "agent-or");

        let result = session_uses_openclaw_runtime(&conn, "session-or").expect("query should succeed");
        assert!(!result, "session bound to an openrouter-runtime agent must report false");
    }

    #[test]
    fn test_session_uses_openclaw_runtime_false_for_missing_session() {
        let conn = make_conn();
        let result = session_uses_openclaw_runtime(&conn, "does-not-exist").expect("query should succeed");
        assert!(!result, "a session with no matching row must report false, not error");
    }

    /// ADR-0062 Amendment 2026-08-23 ("append-in ≠ dispatch-out"): a LOCKED (inactive)
    /// openclaw agent's session must still report true — this helper has no
    /// active-agent gate, unlike `check_bound_agent_active`.
    #[test]
    fn test_session_uses_openclaw_runtime_true_even_when_agent_inactive() {
        let conn = make_conn();
        seed_agent(&conn, "agent-locked", Some("openclaw"), Some("main"));
        conn.execute("UPDATE ai_agents SET is_active = 0 WHERE id = 'agent-locked'", [])
            .expect("deactivate agent");
        seed_session(&conn, "session-locked", "agent-locked");

        let result = session_uses_openclaw_runtime(&conn, "session-locked").expect("query should succeed");
        assert!(result, "a locked/inactive openclaw agent's session must still poll-eligible — append-in ≠ dispatch-out");
    }

    // ── 5. Migration v84 idempotency ────────────────────────────────────────────

    #[test]
    #[serial(pane_test_root)]
    fn test_migration_v84_idempotent_on_double_run() {
        // The v83→v84 dispatch arm calls create_migration_backup(get_db_path(), ..),
        // which (absent an explicit test override) resolves to the REAL production
        // DB path and would WAL-checkpoint + snapshot it — and get_backups_dir() (via
        // get_pane_root()) would write into the real ~/.pane/backups. Redirect BOTH
        // via PANE_TEST_MODE/PANE_TEST_DB (get_db_path) and PANE_TEST_ROOT
        // (get_pane_root/get_backups_dir) to a scratch tempdir for the duration of
        // this test. The backup call itself will fail fast ("file does not exist" —
        // no real DB at the scratch path), which the migration arm catches and logs
        // as a non-fatal WARNING rather than propagating.
        use tempfile::TempDir;
        let temp_dir = TempDir::new().expect("tempdir");
        let test_db_path = temp_dir.path().join("scratch-pane.db");
        unsafe {
            std::env::set_var("PANE_TEST_MODE", "1");
            std::env::set_var("PANE_TEST_DB", test_db_path.to_str().unwrap());
            std::env::set_var("PANE_TEST_ROOT", temp_dir.path());
        }

        let conn = rusqlite::Connection::open_in_memory().expect("in-memory db");
        crate::migrations::run_migrations(&conn).expect("first run should succeed");

        let col_exists_first: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM pragma_table_info('chat_messages_local') WHERE name='oc_message_id'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(col_exists_first, "oc_message_id column must exist after first migration run");

        // Re-running migrations at the current (already-matching) SCHEMA_VERSION must
        // be a clean no-op — exercises the verify_crr_tables path, not migrate_v84
        // directly, but proves the end state survives a second run without error.
        let second_run = crate::migrations::run_migrations(&conn);
        assert!(second_run.is_ok(), "second run at current SCHEMA_VERSION must succeed: {:?}", second_run);

        let col_exists_second: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM pragma_table_info('chat_messages_local') WHERE name='oc_message_id'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(col_exists_second, "oc_message_id column must still exist after second run");

        // Directly exercise the v83→v84 migration arm's idempotency by stamping the
        // schema_version back to "83" (simulating an install that already has the
        // column/index from a first v83→v84 run) and re-running — the ALTER TABLE
        // pragma-check guard must make this a clean no-op, not a "duplicate column" error.
        conn.execute("UPDATE ai_settings SET value = '83' WHERE key = 'schema_version'", [])
            .expect("stamp back to v83 for idempotency re-test");
        let rerun_from_v83 = crate::migrations::run_migrations(&conn);
        assert!(rerun_from_v83.is_ok(), "v83→v84 migration arm must be safely re-runnable when the column/index already exist: {:?}", rerun_from_v83);

        let col_exists_third: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM pragma_table_info('chat_messages_local') WHERE name='oc_message_id'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(col_exists_third, "oc_message_id column must still exist after the simulated v83→v84 re-run");

        unsafe {
            std::env::remove_var("PANE_TEST_MODE");
            std::env::remove_var("PANE_TEST_DB");
        }
    }

    // ── 6. 502 graceful skip ────────────────────────────────────────────────────

    #[tokio::test]
    #[serial(keychain_override)]
    async fn test_oc_pull_502_graceful_skip_no_panic() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", mockito::Matcher::Regex(r"^/v1/sessions/.*/messages.*".to_string()))
            .with_status(502)
            .with_body("Bad Gateway")
            .create_async()
            .await;

        let conn = make_conn();
        seed_agent(&conn, "agent-oc", Some("openclaw"), Some("main"));
        seed_session(&conn, "session-1", "agent-oc");
        crate::database::set_ai_setting(&conn, "endpoint", &server.url()).unwrap();
        // has_active_openclaw_agent (get_gateway_url's gate) requires is_active=1,
        // already satisfied by seed_agent's default.

        let _kc = KeychainGuard::set();
        crate::ai::credential_migration::write_gateway_token(&conn, "test-token-abc").unwrap();

        let db = DbState::new(conn);

        let result = pull_oc_transcript_for_session("session-1".to_string(), &db, false).await;

        assert!(result.is_ok(), "a 502 must be handled gracefully, never propagate as Err: {:?}", result);
        let pull = result.unwrap();
        assert_eq!(pull.fetched_count, 0, "no messages should be reported as fetched on a 502");
        assert!(pull.ingest.is_none(), "ingest must not run when the gateway call failed");
        assert!(pull.skip_reason.is_some(), "skip_reason must explain why the cycle was skipped");
        assert!(
            pull.skip_reason.as_ref().unwrap().contains("502"),
            "skip_reason should mention the 502: {:?}", pull.skip_reason
        );

        // No row should have been written — confirms the skip really was clean, not a
        // partial/silent write before the error was noticed.
        let conn2 = db.0.lock().unwrap();
        let row_count: i64 = conn2
            .query_row("SELECT COUNT(*) FROM chat_messages_local WHERE thread_id = 'session-1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(row_count, 0, "a 502 skip must not write any transcript rows");
    }

    /// Confirms the adapter also skips cleanly (not just on 502) for a session that
    /// isn't openclaw-runtime — the eligibility gate runs before any HTTP call.
    #[tokio::test]
    async fn test_oc_pull_skips_non_openclaw_session_without_network_call() {
        let conn = make_conn();
        seed_agent(&conn, "agent-or", Some("openrouter"), None);
        seed_session(&conn, "session-or", "agent-or");

        let db = DbState::new(conn);
        let result = pull_oc_transcript_for_session("session-or".to_string(), &db, false).await;

        assert!(result.is_ok());
        let pull = result.unwrap();
        assert!(pull.ingest.is_none());
        assert!(pull.skip_reason.is_some());
        assert!(
            pull.skip_reason.as_ref().unwrap().contains("openclaw"),
            "skip_reason should explain the non-openclaw gate: {:?}", pull.skip_reason
        );
    }

    // ── map_oc_message_to_external ──────────────────────────────────────────────

    #[test]
    fn test_map_oc_message_extracts_openclaw_id() {
        let raw = serde_json::json!({
            "role": "assistant",
            "content": "hello from OC",
            "timestamp": 1_756_000_000_000i64,
            "__openclaw": { "id": "real-oc-uuid-123" }
        });

        let mapped = map_oc_message_to_external(&raw, "session-1", "agent-1").expect("should map successfully");
        assert_eq!(mapped.source_message_id, "real-oc-uuid-123");
        assert_eq!(mapped.role, "assistant");
        assert_eq!(mapped.content, "hello from OC");
        assert_eq!(mapped.target_session_id, "session-1");
        assert_eq!(mapped.source_agent_id, "agent-1");
    }

    /// §B.4: absent __openclaw.id must not drop the message — it gets a
    /// locally-synthesized id that skips dedup protection for that one message.
    #[test]
    fn test_map_oc_message_synthesizes_id_when_openclaw_id_absent() {
        let raw = serde_json::json!({
            "role": "assistant",
            "content": "no id here",
        });

        let mapped = map_oc_message_to_external(&raw, "session-1", "agent-1").expect("should map successfully");
        assert!(mapped.source_message_id.starts_with("no-oc-id-"), "got: {}", mapped.source_message_id);
    }

    #[test]
    fn test_map_oc_message_returns_none_when_role_missing() {
        let raw = serde_json::json!({ "content": "no role field" });
        assert!(map_oc_message_to_external(&raw, "session-1", "agent-1").is_none());
    }

    #[test]
    fn test_map_oc_message_flattens_content_block_array() {
        let raw = serde_json::json!({
            "role": "assistant",
            "content": [{"type": "text", "text": "block one"}, {"type": "text", "text": "block two"}],
            "__openclaw": { "id": "oc-id-1" }
        });
        let mapped = map_oc_message_to_external(&raw, "session-1", "agent-1").expect("should map");
        assert_eq!(mapped.content, "block one\nblock two");
    }

    // ── 7. Happy-path 200 integration test (PR 3, WARN 2 from the PR 2688 delta audit) ──
    //
    // Reed's audit (docs/audits/2026-08-23-pr2688-delta-audit.md §5) flagged that PR 2
    // only covered the 502-skip and non-openclaw-skip paths for
    // `pull_oc_transcript_for_session` itself — no test exercised a successful 200
    // response end to end. This closes that gap: mocked gateway returns 3 real
    // messages with `__openclaw` metadata, asserts they land in
    // `chat_messages_local` with the correct `oc_message_id`/`role`/`content`, then
    // re-calls and asserts dedup (`inserted_count == 0` on replay).

    // NOTE: this session is pre-baselined below (`oc_baseline_done_at` stamped
    // directly via SQL before the first `pull_oc_transcript_for_session` call) so
    // this test exercises steady-state ingest/dedup behavior, not the first-pull
    // baseline gate — that gate has its own dedicated tests (§2410 ingestion
    // filters, see `oc_pull_baseline_and_role_filter_tests` below).
    #[tokio::test]
    #[serial(keychain_override)]
    async fn test_oc_pull_happy_path_200_ingests_messages_end_to_end() {
        let mut server = mockito::Server::new_async().await;
        let body = serde_json::json!({
            "messages": [
                {
                    "role": "assistant",
                    "content": "first proactive message",
                    "timestamp": 1_756_000_000_000i64,
                    "__openclaw": { "id": "oc-happy-1" }
                },
                {
                    "role": "assistant",
                    "content": "second proactive message",
                    "timestamp": 1_756_000_001_000i64,
                    "__openclaw": { "id": "oc-happy-2" }
                },
                {
                    "role": "user",
                    "content": "an out-of-turn user-authored transcript entry",
                    "timestamp": 1_756_000_002_000i64,
                    "__openclaw": { "id": "oc-happy-3" }
                }
            ]
        });
        // §30.11.3: the gateway path must carry the derived composite OC session
        // key (`agent:<runtime_ref>:openai-user:pane-<session_id>`), percent-encoded,
        // not the bare session id — this is the contract PR 2410 fixes and locks.
        let _m = server
            .mock(
                "GET",
                "/v1/sessions/agent%3Amain%3Aopenai-user%3Apane-session-happy/messages?limit=20",
            )
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(body.to_string())
            .create_async()
            .await;

        let conn = make_conn();
        seed_agent(&conn, "agent-oc", Some("openclaw"), Some("main"));
        seed_session(&conn, "session-happy", "agent-oc");
        crate::database::set_ai_setting(&conn, "endpoint", &server.url()).unwrap();
        // §2410 ingestion filters: pre-baseline this session so the pull below exercises
        // the steady-state ingest path, not the first-pull baseline gate.
        conn.execute(
            "UPDATE chat_sessions SET oc_baseline_done_at = datetime('now') WHERE id = 'session-happy'",
            [],
        )
        .unwrap();

        let _kc = KeychainGuard::set();
        crate::ai::credential_migration::write_gateway_token(&conn, "test-token-happy").unwrap();

        let db = DbState::new(conn);

        // First call (post-baseline): 3 raw messages fetched, but the role filter
        // (§2410) drops the `user`-role message — only the 2 assistant messages map
        // and insert.
        let result = pull_oc_transcript_for_session("session-happy".to_string(), &db, false).await;

        assert!(result.is_ok(), "a 200 with valid messages must not error: {:?}", result);
        let pull = result.unwrap();
        assert_eq!(pull.fetched_count, 3, "all 3 raw messages must be counted as fetched: {:?}", pull);
        assert!(pull.skip_reason.is_none(), "a successful pull must not carry a skip_reason: {:?}", pull);
        let ingest = pull.ingest.expect("ingest result must be present on a successful pull");
        assert_eq!(ingest.inserted_count, 2, "only the 2 assistant-role messages must insert (§2410 role filter drops the user-role message): {:?}", ingest);
        assert_eq!(ingest.dedup_skipped_count, 0);
        assert_eq!(ingest.binding_mismatch_count, 0);
        assert!(ingest.errors.is_empty(), "no errors expected: {:?}", ingest.errors);

        // Assert the rows actually exist in chat_messages_local with correct fields —
        // and that the user-role message never landed at all.
        {
            let conn2 = db.0.lock().unwrap();
            let mut stmt = conn2
                .prepare(
                    "SELECT oc_message_id, role, content FROM chat_messages_local \
                     WHERE thread_id = 'session-happy' ORDER BY oc_message_id",
                )
                .unwrap();
            let rows: Vec<(String, String, String)> = stmt
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                .unwrap()
                .map(|r| r.unwrap())
                .collect();
            assert_eq!(
                rows,
                vec![
                    ("oc-happy-1".to_string(), "assistant".to_string(), "first proactive message".to_string()),
                    ("oc-happy-2".to_string(), "assistant".to_string(), "second proactive message".to_string()),
                ],
                "only the 2 assistant messages must land — the user-role message must be filtered out entirely"
            );
        }

        // Second call (dedup): the same 3 messages come back from the mocked gateway
        // again — must insert zero, proving end-to-end dedup through the real adapter,
        // not just through ingest_external_messages_impl in isolation.
        //
        // #2410 tombstoning note: with the Rule 1 permanence pre-filter (§2c of the
        // tombstone-scope audit) now running unconditionally BEFORE the dedup index or
        // the content-match backstop ever see this batch, both oc-happy-1 and
        // oc-happy-2 are caught immediately by `is_already_classified` (they're already
        // real rows in `chat_messages_local` from the first pull, matched by
        // `(thread_id, oc_message_id)`) — neither one reaches the dedup index or the
        // backstop at all on this re-pull. Either way, zero new rows insert and nothing
        // is lost — this asserts the SUM across all three possible catch points rather
        // than assuming a specific one.
        let second = pull_oc_transcript_for_session("session-happy".to_string(), &db, false).await;

        assert!(second.is_ok(), "a re-pull must not error: {:?}", second);
        let second_pull = second.unwrap();
        let second_ingest = second_pull.ingest.expect("ingest result must be present on the re-pull too");
        assert_eq!(second_ingest.inserted_count, 0, "re-pulling the same batch must insert zero rows: {:?}", second_ingest);
        assert_eq!(
            second_ingest.dedup_skipped_count + second_pull.content_match_skipped_count + second_pull.already_classified_count,
            2,
            "both previously-inserted assistant messages must be caught — by the tombstone pre-filter, the dedup \
             index, or the content-match backstop: {:?} / content_match_skipped_count={} / already_classified_count={}",
            second_ingest, second_pull.content_match_skipped_count, second_pull.already_classified_count
        );

        let row_count: i64 = {
            let conn3 = db.0.lock().unwrap();
            conn3
                .query_row("SELECT COUNT(*) FROM chat_messages_local WHERE thread_id = 'session-happy'", [], |r| r.get(0))
                .unwrap()
        };
        assert_eq!(row_count, 2, "exactly 2 rows must exist after both pulls — no duplicates, no user-role row");
    }

    // #2690 (§30.11 ordering fix): origination-timestamp stamping, clock-skew floor
    // clamp, and the rowid display tiebreak. Nested (not sibling) module so these
    // tests can reuse the parent's private `make_conn`/`seed_agent`/`seed_session`
    // helpers via `super::*` — Rust privacy only extends to descendant modules,
    // not siblings.
    mod ordering_2690_tests {
    use super::*;

    fn external_message_at(session_id: &str, source_message_id: &str, source_agent_id: &str, rfc3339_timestamp: &str, content: &str) -> ExternalMessage {
        ExternalMessage {
            target_session_id: session_id.to_string(),
            source_message_id: source_message_id.to_string(),
            role: "assistant".to_string(),
            content: content.to_string(),
            timestamp: rfc3339_timestamp.to_string(),
            source_agent_id: source_agent_id.to_string(),
            attachment_ids: None,
            tool_calls: None,
            tool_results: None,
            model: None,
            token_count: None,
            job_id: None,
        }
    }

    // ── 1. Same-tick batch ordering (Blue/Green fixture) ───────────────────────
    //
    // Two messages ingested in the same `ingest_external_messages_impl` call (i.e.
    // the same poll tick) but with distinct origination timestamps must render in
    // send order — origination time wins over any incidental insertion-order
    // artifact within the batch.

    #[test]
    #[serial]
    fn test_same_tick_batch_renders_in_origination_send_order() {
        let conn = make_conn();
        seed_agent(&conn, "agent-1", Some("openclaw"), Some("main"));
        seed_session(&conn, "session-1", "agent-1");

        // Blue sent first (10:00:00), Green sent second (10:00:05) — but the batch
        // itself lists Green before Blue, simulating OC handing back an out-of-order
        // page within one poll tick. If ordering only depended on insertion order,
        // Green would render first; the origination stamp must override that.
        let batch = vec![
            external_message_at("session-1", "oc-green", "agent-1", "2026-08-24T10:00:05Z", "Green"),
            external_message_at("session-1", "oc-blue", "agent-1", "2026-08-24T10:00:00Z", "Blue"),
        ];

        let result = ingest_external_messages_impl(&batch, &conn);
        assert_eq!(result.inserted_count, 2, "both messages must insert: {:?}", result);

        let mut stmt = conn
            .prepare("SELECT content, rowid FROM chat_messages_local WHERE thread_id = 'session-1' ORDER BY created_at DESC, rowid DESC")
            .unwrap();
        let rows: Vec<(String, i64)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();

        assert_eq!(
            rows.iter().map(|(c, _)| c.as_str()).collect::<Vec<_>>(),
            vec!["Green", "Blue"],
            "display order (created_at DESC) must be Green-then-Blue (send order, newest first) \
             regardless of insertion order within the batch: {:?}",
            rows
        );
    }

    // ── 2. RFC3339-vs-SQLite format regression ──────────────────────────────────

    #[test]
    #[serial]
    fn test_created_at_stored_as_sqlite_format_not_raw_rfc3339() {
        let conn = make_conn();
        seed_agent(&conn, "agent-1", Some("openclaw"), Some("main"));
        seed_session(&conn, "session-1", "agent-1");

        let batch = vec![external_message_at("session-1", "oc-msg-1", "agent-1", "2026-08-24T10:00:05Z", "hi")];
        let result = ingest_external_messages_impl(&batch, &conn);
        assert_eq!(result.inserted_count, 1, "message must insert: {:?}", result);

        let created_at: String = conn
            .query_row(
                "SELECT created_at FROM chat_messages_local WHERE thread_id = 'session-1' AND oc_message_id = 'oc-msg-1'",
                [],
                |r| r.get(0),
            )
            .unwrap();

        assert_eq!(
            created_at, "2026-08-24 10:00:05",
            "created_at must be normalized to SQLite's space-separated format, never raw RFC3339 \
             ('T' separator, '+00:00'/'Z' offset suffix) — the exact hazard sqlite_ts() documents"
        );
        assert!(!created_at.contains('T'), "created_at must not contain the RFC3339 'T' separator: {:?}", created_at);
        assert!(!created_at.contains('Z'), "created_at must not contain the RFC3339 'Z' offset suffix: {:?}", created_at);
    }

    // ── 3. Clock-skew clamp ─────────────────────────────────────────────────────

    #[test]
    #[serial]
    fn test_clock_skew_clamps_reply_to_never_sort_above_its_trigger_message() {
        let conn = make_conn();
        seed_agent(&conn, "agent-1", Some("openclaw"), Some("main"));
        seed_session(&conn, "session-1", "agent-1");

        // Simulate a just-saved Pane dispatch row (the user's trigger message) at
        // 10:05:00 local Pane clock time — written the same way save_chat_message
        // writes it (datetime('now') semantics, but pinned here for determinism).
        conn.execute(
            "INSERT INTO chat_messages_local (id, project_id, thread_id, role, content, created_at) \
             VALUES ('local-trigger', '', 'session-1', 'user', 'what time is it', '2026-08-24 10:05:00')",
            [],
        )
        .unwrap();

        // OC's clock is skewed 3 minutes slow: the reply's origination timestamp
        // (10:02:00) claims to be BEFORE the trigger message it's replying to.
        let batch = vec![external_message_at("session-1", "oc-reply", "agent-1", "2026-08-24T10:02:00Z", "it's 10:05")];
        let result = ingest_external_messages_impl(&batch, &conn);
        assert_eq!(result.inserted_count, 1, "reply must insert: {:?}", result);

        let reply_created_at: String = conn
            .query_row(
                "SELECT created_at FROM chat_messages_local WHERE thread_id = 'session-1' AND oc_message_id = 'oc-reply'",
                [],
                |r| r.get(0),
            )
            .unwrap();

        assert_eq!(
            reply_created_at, "2026-08-24 10:05:00",
            "clock-skewed reply must be clamped to the thread's max created_at (the trigger message's \
             timestamp), never keep its earlier claimed origination time"
        );
        assert!(
            reply_created_at >= "2026-08-24 10:05:00".to_string(),
            "reply must never sort before (or, via string comparison, earlier than) its trigger message"
        );
    }

    #[test]
    #[serial]
    fn test_first_message_in_empty_thread_uses_origination_timestamp_as_is() {
        let conn = make_conn();
        seed_agent(&conn, "agent-1", Some("openclaw"), Some("main"));
        seed_session(&conn, "session-1", "agent-1");

        // No prior rows in this thread — nothing to clamp against.
        let batch = vec![external_message_at("session-1", "oc-first", "agent-1", "2026-08-24T09:00:00Z", "first ever message")];
        let result = ingest_external_messages_impl(&batch, &conn);
        assert_eq!(result.inserted_count, 1, "message must insert: {:?}", result);

        let created_at: String = conn
            .query_row(
                "SELECT created_at FROM chat_messages_local WHERE thread_id = 'session-1' AND oc_message_id = 'oc-first'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(created_at, "2026-08-24 09:00:00", "an empty thread's first message must use the origination timestamp unclamped");
    }

    // ── 4. Compaction boundary ───────────────────────────────────────────────────

    #[test]
    #[serial]
    fn test_origination_stamped_row_near_compaction_cutoff_never_silently_dropped() {
        let conn = make_conn();
        seed_agent(&conn, "agent-1", Some("openclaw"), Some("main"));
        seed_session(&conn, "session-1", "agent-1");

        // Seed 3 "old" local messages that will be marked compacted.
        for i in 1..=3 {
            conn.execute(
                "INSERT INTO chat_messages_local (id, project_id, thread_id, role, content, created_at) \
                 VALUES (?1, '', 'session-1', 'user', ?2, ?3)",
                rusqlite::params![
                    format!("local-old-{}", i),
                    format!("old message {}", i),
                    format!("2026-08-24 09:0{}:00", i)
                ],
            )
            .unwrap();
        }

        let last_old_rowid: i64 = conn
            .query_row("SELECT rowid FROM chat_messages_local WHERE id = 'local-old-3'", [], |r| r.get(0))
            .unwrap();

        // Record a compaction summary whose cutoff is the 3rd old message's rowid,
        // mirroring what maybe_compact_thread's write site now stores.
        conn.execute(
            "INSERT INTO compaction_summaries (id, thread_id, summary, message_count, compacted_before, compacted_before_rowid, created_at) \
             VALUES ('summary-1', 'session-1', 'summary of old messages', 3, '2026-08-24 09:03:00', ?1, datetime('now'))",
            rusqlite::params![last_old_rowid],
        )
        .unwrap();

        // Now ingest an OC-origination-stamped reply whose origination timestamp is
        // EARLIER than the compacted messages' created_at (e.g. OC clock skew or a
        // late-arriving batch) but whose insertion (rowid) happens AFTER the cutoff
        // was recorded. A created_at-keyed cutoff comparison would wrongly treat this
        // row as already-compacted (silently dropped from both the summary and the
        // live list) since its created_at <= the old cutoff's created_at string.
        let batch = vec![external_message_at("session-1", "oc-late", "agent-1", "2026-08-24T09:01:30Z", "late-arriving reply")];
        let result = ingest_external_messages_impl(&batch, &conn);
        assert_eq!(result.inserted_count, 1, "late-arriving reply must still insert: {:?}", result);

        let late_rowid: i64 = conn
            .query_row("SELECT rowid FROM chat_messages_local WHERE thread_id = 'session-1' AND oc_message_id = 'oc-late'", [], |r| r.get(0))
            .unwrap();
        assert!(late_rowid > last_old_rowid, "late message's rowid must be after the cutoff rowid (insertion order)");

        // The rowid-keyed cutoff query (mirroring maybe_compact_thread's non-compacted
        // load) must include this row — it was inserted after the cutoff rowid, so it
        // is NOT compacted, regardless of its (earlier, clock-skewed) created_at.
        let non_compacted_ids: Vec<String> = {
            let mut stmt = conn
                .prepare("SELECT id FROM chat_messages_local WHERE thread_id = 'session-1' AND rowid > ?1 ORDER BY created_at ASC, rowid ASC")
                .unwrap();
            stmt.query_map(rusqlite::params![last_old_rowid], |r| r.get(0))
                .unwrap()
                .map(|r| r.unwrap())
                .collect()
        };
        assert!(
            non_compacted_ids.iter().any(|id| id.contains("oc-late") || id.starts_with("ext-")),
            "the late-arriving row must appear in the non-compacted (live) set — rowid-keyed cutoff must never \
             silently drop a row whose created_at looks old but whose rowid says it was inserted after the cutoff: {:?}",
            non_compacted_ids
        );

        // And a created_at-keyed comparison (the pre-#2690 behavior) WOULD have
        // wrongly excluded it — asserting the old bug still reproduces on the old
        // query shape documents exactly what this fix closes.
        let old_query_would_have_included: bool = {
            let created_at: String = conn
                .query_row("SELECT created_at FROM chat_messages_local WHERE thread_id = 'session-1' AND oc_message_id = 'oc-late'", [], |r| r.get(0))
                .unwrap();
            created_at > "2026-08-24 09:03:00".to_string()
        };
        assert!(
            !old_query_would_have_included,
            "sanity check: the late message's created_at (09:01:30, clock-skewed) must indeed be BEFORE the old \
             compaction cutoff's created_at (09:03:00) — confirming this test actually exercises the bug the \
             rowid-keyed fix closes, not a scenario the old query already handled correctly"
        );
    }

    // ── 5. Tiebreak determinism ──────────────────────────────────────────────────

    #[test]
    #[serial]
    fn test_identical_created_at_tiebreak_is_stable_across_requeries() {
        let conn = make_conn();
        seed_agent(&conn, "agent-1", Some("openclaw"), Some("main"));
        seed_session(&conn, "session-1", "agent-1");

        // Two messages with distinct sub-second origination timestamps that both
        // truncate to the same whole second — a same-second collision.
        let batch = vec![
            external_message_at("session-1", "oc-a", "agent-1", "2026-08-24T10:00:00.100Z", "A"),
            external_message_at("session-1", "oc-b", "agent-1", "2026-08-24T10:00:00.900Z", "B"),
        ];
        let result = ingest_external_messages_impl(&batch, &conn);
        assert_eq!(result.inserted_count, 2, "both messages must insert: {:?}", result);

        let created_ats: Vec<String> = {
            let mut stmt = conn.prepare("SELECT created_at FROM chat_messages_local WHERE thread_id = 'session-1'").unwrap();
            stmt.query_map([], |r| r.get(0)).unwrap().map(|r| r.unwrap()).collect()
        };
        assert_eq!(
            created_ats[0], created_ats[1],
            "sub-second timestamps must truncate to an identical whole-second created_at, producing the collision \
             this test exercises: {:?}",
            created_ats
        );

        // Query with the ORDER BY created_at DESC, rowid DESC tiebreak repeatedly —
        // must produce the exact same order every time (deterministic, not
        // SQLite's undefined tie-order behavior).
        let query_order = || -> Vec<String> {
            let mut stmt = conn
                .prepare("SELECT oc_message_id FROM chat_messages_local WHERE thread_id = 'session-1' ORDER BY created_at DESC, rowid DESC")
                .unwrap();
            stmt.query_map([], |r| r.get(0)).unwrap().map(|r| r.unwrap()).collect()
        };

        let first_query = query_order();
        let second_query = query_order();
        let third_query = query_order();

        assert_eq!(first_query, second_query, "tiebreak order must be stable across repeated queries");
        assert_eq!(second_query, third_query, "tiebreak order must be stable across repeated queries");
        assert_eq!(
            first_query,
            vec!["oc-b".to_string(), "oc-a".to_string()],
            "rowid DESC tiebreak must resolve the collision deterministically — oc-b inserted after oc-a (higher \
             rowid) so it sorts first under DESC: {:?}",
            first_query
        );
    }
    } // mod ordering_2690_tests
}


/// §2410 ingestion filters: first-pull baseline gate + role/content mapper guards.
/// Separate module so the tests below can reuse `make_conn`/`seed_agent`/`seed_session`
/// from the parent `ingest_external_messages_tests` module via `super::*`, matching the
/// existing test-module nesting convention in this file.
#[cfg(test)]
mod oc_pull_baseline_and_role_filter_tests {
    use super::*;
    use crate::keychain::test_support::KeychainGuard;
    use serial_test::serial;

    /// Real production schema via `run_migrations` — matches the sibling
    /// `ingest_external_messages_tests::make_conn` (private to that module, so
    /// duplicated here per this file's existing test-module convention — see
    /// `oc_pull_command_wrapper_tests` below, which does the same for `seed_agent`/
    /// `seed_session`).
    fn make_conn() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().expect("in-memory db");
        crate::migrations::run_migrations(&conn).expect("fresh-install migrations should succeed");
        conn
    }

    fn seed_agent(conn: &rusqlite::Connection, id: &str, runtime_type: Option<&str>, runtime_ref: Option<&str>) {
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO ai_agents (id, name, slug, is_default, is_active, workspace_path, runtime_type, runtime_ref, model, created_at, updated_at)
             VALUES (?1, ?1, ?1, 0, 1, ?1, ?2, ?3, 'test-model', ?4, ?4)",
            rusqlite::params![id, runtime_type, runtime_ref, now],
        )
        .expect("seed agent");
    }

    fn seed_session(conn: &rusqlite::Connection, id: &str, agent_id: &str) {
        conn.execute(
            "INSERT INTO chat_sessions (id, project_id, agent_id) VALUES (?1, 'workspace', ?2)",
            rusqlite::params![id, agent_id],
        )
        .expect("seed session");
    }

    /// First-pull baseline (§2410): mocked OC returns 5 messages. The FIRST call to
    /// `pull_oc_transcript_for_session` for a never-before-polled session must insert
    /// ZERO rows and stamp `chat_sessions.oc_baseline_done_at` non-NULL — the session's
    /// pre-existing OC history is marked "seen", not backfilled as new output.
    #[tokio::test]
    #[serial(keychain_override)]
    async fn test_oc_pull_first_pull_baselines_without_inserting() {
        let mut server = mockito::Server::new_async().await;
        let body = serde_json::json!({
            "messages": [
                { "role": "assistant", "content": "history 1", "timestamp": 1_756_000_000_000i64, "__openclaw": { "id": "oc-hist-1" } },
                { "role": "user", "content": "history 2", "timestamp": 1_756_000_001_000i64, "__openclaw": { "id": "oc-hist-2" } },
                { "role": "assistant", "content": "history 3", "timestamp": 1_756_000_002_000i64, "__openclaw": { "id": "oc-hist-3" } },
                { "role": "toolResult", "content": "history 4", "timestamp": 1_756_000_003_000i64, "__openclaw": { "id": "oc-hist-4" } },
                { "role": "assistant", "content": "history 5", "timestamp": 1_756_000_004_000i64, "__openclaw": { "id": "oc-hist-5" } }
            ]
        });
        let _m = server
            .mock(
                "GET",
                "/v1/sessions/agent%3Amain%3Aopenai-user%3Apane-session-baseline/messages?limit=20",
            )
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(body.to_string())
            .create_async()
            .await;

        let conn = make_conn();
        seed_agent(&conn, "agent-oc", Some("openclaw"), Some("main"));
        seed_session(&conn, "session-baseline", "agent-oc");
        crate::database::set_ai_setting(&conn, "endpoint", &server.url()).unwrap();

        // Sanity check: oc_baseline_done_at is NULL for a freshly-seeded session.
        {
            let baseline: Option<String> = conn
                .query_row(
                    "SELECT oc_baseline_done_at FROM chat_sessions WHERE id = 'session-baseline'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert!(baseline.is_none(), "a freshly-seeded session must start with oc_baseline_done_at = NULL");
        }

        let _kc = KeychainGuard::set();
        crate::ai::credential_migration::write_gateway_token(&conn, "test-token-baseline").unwrap();

        let db = DbState::new(conn);

        let result = pull_oc_transcript_for_session("session-baseline".to_string(), &db, false).await;

        assert!(result.is_ok(), "first-ever pull must not error: {:?}", result);
        let pull = result.unwrap();
        assert_eq!(pull.fetched_count, 5, "all 5 raw messages must be counted as fetched: {:?}", pull);
        assert!(pull.ingest.is_none(), "a baseline cycle must not run ingestion at all: {:?}", pull);
        assert_eq!(
            pull.skip_reason.as_deref(),
            Some("first pull: baselined 5 message(s), none ingested"),
            "skip_reason must match the baseline convention exactly: {:?}", pull.skip_reason
        );

        // Zero rows written to chat_messages_local.
        let conn2 = db.0.lock().unwrap();
        let row_count: i64 = conn2
            .query_row("SELECT COUNT(*) FROM chat_messages_local WHERE thread_id = 'session-baseline'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(row_count, 0, "a first-ever pull must insert zero rows, even though 5 messages were fetched");

        // oc_baseline_done_at must now be non-NULL.
        let baseline_after: Option<String> = conn2
            .query_row("SELECT oc_baseline_done_at FROM chat_sessions WHERE id = 'session-baseline'", [], |r| r.get(0))
            .unwrap();
        assert!(baseline_after.is_some(), "oc_baseline_done_at must be stamped non-NULL after the first pull");
    }

    /// Second pull ingests novel assistant messages (§2410): once a session is
    /// baselined, a subsequent pull with NEW messages must ingest only the
    /// assistant-role, non-empty-content ones — proving the baseline gate only fires
    /// once, and steady-state ingestion + the role/content filters compose correctly.
    #[tokio::test]
    #[serial(keychain_override)]
    async fn test_oc_pull_second_pull_ingests_novel_assistant_messages() {
        let mut server = mockito::Server::new_async().await;

        let conn = make_conn();
        seed_agent(&conn, "agent-oc", Some("openclaw"), Some("main"));
        seed_session(&conn, "session-second", "agent-oc");
        crate::database::set_ai_setting(&conn, "endpoint", &server.url()).unwrap();

        let _kc = KeychainGuard::set();
        crate::ai::credential_migration::write_gateway_token(&conn, "test-token-second").unwrap();

        let db = DbState::new(conn);

        // First call: baseline cycle (an initial 2-message page). Consumes the
        // baseline gate; nothing ingested.
        let baseline_body = serde_json::json!({
            "messages": [
                { "role": "assistant", "content": "pre-existing 1", "timestamp": 1_756_000_000_000i64, "__openclaw": { "id": "oc-pre-1" } },
                { "role": "assistant", "content": "pre-existing 2", "timestamp": 1_756_000_001_000i64, "__openclaw": { "id": "oc-pre-2" } }
            ]
        });
        let m1 = server
            .mock(
                "GET",
                "/v1/sessions/agent%3Amain%3Aopenai-user%3Apane-session-second/messages?limit=20",
            )
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(baseline_body.to_string())
            .create_async()
            .await;

        let baseline_result = pull_oc_transcript_for_session("session-second".to_string(), &db, false).await;
        assert!(baseline_result.is_ok());
        let baseline_pull = baseline_result.unwrap();
        assert!(baseline_pull.ingest.is_none(), "the first call must be the baseline cycle: {:?}", baseline_pull);
        m1.assert_async().await;

        // Second call: mock now returns novel messages — new assistant text, plus a
        // user-role echo and a toolResult blob mixed in, matching the real transcript
        // shape the task describes. Only the novel assistant/text message must insert.
        let novel_body = serde_json::json!({
            "messages": [
                { "role": "assistant", "content": "new proactive output", "timestamp": 1_756_000_002_000i64, "__openclaw": { "id": "oc-novel-1" } },
                { "role": "user", "content": "cron payload echo", "timestamp": 1_756_000_003_000i64, "__openclaw": { "id": "oc-novel-2" } },
                { "role": "toolResult", "content": "raw tool output blob", "timestamp": 1_756_000_004_000i64, "__openclaw": { "id": "oc-novel-3" } }
            ]
        });
        let _m2 = server
            .mock(
                "GET",
                "/v1/sessions/agent%3Amain%3Aopenai-user%3Apane-session-second/messages?limit=20",
            )
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(novel_body.to_string())
            .create_async()
            .await;

        let second_result = pull_oc_transcript_for_session("session-second".to_string(), &db, false).await;

        assert!(second_result.is_ok(), "second pull must not error: {:?}", second_result);
        let second_pull = second_result.unwrap();
        assert_eq!(second_pull.fetched_count, 3, "all 3 raw messages must be counted as fetched: {:?}", second_pull);
        assert!(second_pull.skip_reason.is_none(), "a successful post-baseline pull must not carry a skip_reason: {:?}", second_pull);
        let ingest = second_pull.ingest.expect("ingest result must be present on the second (post-baseline) pull");
        assert_eq!(ingest.inserted_count, 1, "only the 1 novel assistant-role message must insert: {:?}", ingest);

        let conn2 = db.0.lock().unwrap();
        let mut stmt = conn2
            .prepare("SELECT oc_message_id, role, content FROM chat_messages_local WHERE thread_id = 'session-second' ORDER BY oc_message_id")
            .unwrap();
        let rows: Vec<(String, String, String)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(
            rows,
            vec![("oc-novel-1".to_string(), "assistant".to_string(), "new proactive output".to_string())],
            "only the novel assistant message must land — baseline-page messages, the user echo, and the toolResult blob must all be absent"
        );
    }

    /// Role filter (§2410): `map_oc_message_to_external` accepts `assistant` and
    /// rejects `user`/`toolResult` (and any other non-assistant role).
    #[test]
    fn test_map_oc_message_role_filter_only_assistant_passes() {
        let assistant_raw = serde_json::json!({
            "role": "assistant",
            "content": "an assistant turn",
            "__openclaw": { "id": "oc-role-assistant" }
        });
        assert!(
            map_oc_message_to_external(&assistant_raw, "session-1", "agent-1").is_some(),
            "an assistant-role message must map successfully"
        );

        let user_raw = serde_json::json!({
            "role": "user",
            "content": "a cron-payload user echo",
            "__openclaw": { "id": "oc-role-user" }
        });
        assert!(
            map_oc_message_to_external(&user_raw, "session-1", "agent-1").is_none(),
            "a user-role message must be rejected — it may be a cron/system echo, not something Matt typed"
        );

        let tool_result_raw = serde_json::json!({
            "role": "toolResult",
            "content": "raw tool output JSON",
            "__openclaw": { "id": "oc-role-toolresult" }
        });
        assert!(
            map_oc_message_to_external(&tool_result_raw, "session-1", "agent-1").is_none(),
            "a toolResult-role message must be rejected — it's raw tool output, not a conversational turn"
        );
    }

    /// Empty-content guard (§2410): an assistant message whose content blocks are
    /// all `toolCall` (no `text` block at all) must map to `None`, not an
    /// empty-content row — the flattening step yields `""` for a zero-text-block
    /// array, which would otherwise slip past the `_ => return None` arm.
    #[test]
    fn test_map_oc_message_empty_content_guard_rejects_pure_tool_call_turn() {
        let raw = serde_json::json!({
            "role": "assistant",
            "content": [
                { "type": "toolCall", "name": "some_tool", "arguments": { "a": 1 } }
            ],
            "__openclaw": { "id": "oc-empty-content" }
        });
        assert!(
            map_oc_message_to_external(&raw, "session-1", "agent-1").is_none(),
            "a pure-toolCall assistant turn (no text block) must be rejected, not inserted as a blank row"
        );
    }

    /// Migration idempotency (§2410, v85): running the v84→v85 migration twice must
    /// not error — the `chat_sessions.oc_baseline_done_at` column-exists pragma check
    /// must make the second ALTER TABLE a clean no-op.
    ///
    /// The v84→v85 dispatch arm calls `create_migration_backup(get_db_path(), ..)`,
    /// which (absent an explicit test override) resolves to the REAL production DB
    /// path and would WAL-checkpoint + snapshot it — matches the exact rationale
    /// documented on the sibling `test_migration_v84_idempotent_on_double_run` test
    /// above. Same `PANE_TEST_MODE`/`PANE_TEST_DB`/`PANE_TEST_ROOT` scratch-tempdir
    /// redirect is required here for the same reason.
    #[test]
    #[serial(pane_test_root)]
    fn test_migration_v85_idempotent_on_double_run() {
        use tempfile::TempDir;
        let temp_dir = TempDir::new().expect("tempdir");
        let test_db_path = temp_dir.path().join("scratch-pane.db");
        unsafe {
            std::env::set_var("PANE_TEST_MODE", "1");
            std::env::set_var("PANE_TEST_DB", test_db_path.to_str().unwrap());
            std::env::set_var("PANE_TEST_ROOT", temp_dir.path());
        }

        let conn = rusqlite::Connection::open_in_memory().expect("in-memory db");
        crate::migrations::run_migrations(&conn).expect("fresh-install migrations should succeed");

        let col_exists_first: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM pragma_table_info('chat_sessions') WHERE name='oc_baseline_done_at'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(col_exists_first, "oc_baseline_done_at column must exist after first migration run");

        // Re-running migrations at the current (already-matching) SCHEMA_VERSION must
        // be a clean no-op.
        let second_run = crate::migrations::run_migrations(&conn);
        assert!(second_run.is_ok(), "second full run at current SCHEMA_VERSION must succeed: {:?}", second_run);

        let col_exists_second: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM pragma_table_info('chat_sessions') WHERE name='oc_baseline_done_at'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(col_exists_second, "oc_baseline_done_at column must still exist after second run");

        // Directly exercise the v84→v85 migration arm's idempotency by stamping
        // schema_version back to "84" (simulating an install that already has the
        // column from a first v84→v85 run) and re-running — the ALTER TABLE
        // pragma-check guard must make this a clean no-op, not a "duplicate column" error.
        conn.execute("UPDATE ai_settings SET value = '84' WHERE key = 'schema_version'", [])
            .expect("stamp back to v84 for idempotency re-test");
        let rerun_from_v84 = crate::migrations::run_migrations(&conn);
        assert!(rerun_from_v84.is_ok(), "v84→v85 migration arm must be safely re-runnable when the column already exists: {:?}", rerun_from_v84);

        let col_exists_third: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM pragma_table_info('chat_sessions') WHERE name='oc_baseline_done_at'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(col_exists_third, "oc_baseline_done_at column must still exist after the simulated v84→v85 re-run");

        unsafe {
            std::env::remove_var("PANE_TEST_MODE");
            std::env::remove_var("PANE_TEST_DB");
        }
    }
}


/// PR 4 (#2410, §30.11.4, echo suppression): tests for
/// `is_recent_local_assistant_echo` (Candidate 3 content-match backstop) and the
/// `suppress_ingest` "defer, don't skip" flag threaded through
/// `pull_oc_transcript_for_session` (Candidate 2 primary mechanism, backend half).
/// Separate module for the same reason as the sibling `oc_pull_baseline_and_role_filter_tests`
/// module — reuses `make_conn`/`seed_agent`/`seed_session` via its own copies, matching
/// this file's existing test-module convention.
#[cfg(test)]
mod echo_suppression_tests {
    use super::*;
    use crate::keychain::test_support::KeychainGuard;
    use serial_test::serial;

    fn make_conn() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().expect("in-memory db");
        crate::migrations::run_migrations(&conn).expect("fresh-install migrations should succeed");
        conn
    }

    fn seed_agent(conn: &rusqlite::Connection, id: &str, runtime_type: Option<&str>, runtime_ref: Option<&str>) {
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO ai_agents (id, name, slug, is_default, is_active, workspace_path, runtime_type, runtime_ref, model, created_at, updated_at)
             VALUES (?1, ?1, ?1, 0, 1, ?1, ?2, ?3, 'test-model', ?4, ?4)",
            rusqlite::params![id, runtime_type, runtime_ref, now],
        )
        .expect("seed agent");
    }

    fn seed_session(conn: &rusqlite::Connection, id: &str, agent_id: &str) {
        conn.execute(
            "INSERT INTO chat_sessions (id, project_id, agent_id) VALUES (?1, 'workspace', ?2)",
            rusqlite::params![id, agent_id],
        )
        .expect("seed session");
    }

    /// `chat_messages_local.created_at` is always written in production via SQLite's
    /// own `datetime('now')` (space-separated `YYYY-MM-DD HH:MM:SS`, per every
    /// `INSERT INTO chat_messages_local` site in this file — e.g. line ~11711), never
    /// chrono's RFC3339 (`T`-separated, offset suffix). `is_recent_local_assistant_echo`'s
    /// window check does a plain SQL string comparison (`created_at >= datetime('now', ...)`),
    /// so test fixtures MUST format timestamps the same way production does — otherwise
    /// `T` (0x54) sorting after the space (0x20) separator silently makes every RFC3339
    /// timestamp compare as "newer" than it really is, masking real window-boundary bugs.
    fn sqlite_ts(dt: chrono::DateTime<chrono::Utc>) -> String {
        dt.format("%Y-%m-%d %H:%M:%S").to_string()
    }

    /// Inserts a local assistant row directly (bypassing the external-ingestion path
    /// entirely — this simulates Pane's OWN dispatch-side save, `oc_message_id = NULL`,
    /// exactly per §1a of Reed's audit) with an explicit `created_at`, so tests can
    /// control the content-match window precisely.
    fn seed_local_assistant_row(conn: &rusqlite::Connection, thread_id: &str, content: &str, created_at: &str) {
        conn.execute(
            "INSERT INTO chat_messages_local (id, project_id, thread_id, role, content, created_at) \
             VALUES (?1, '', ?2, 'assistant', ?3, ?4)",
            rusqlite::params![format!("local-{}", uuid::Uuid::new_v4()), thread_id, content, created_at],
        )
        .expect("seed local assistant row");
    }

    // ── 1. Content-match hit ─────────────────────────────────────────────────────

    /// The incoming OC segment's text is a substring of the local row's content
    /// (simulating an intermediate tool-loop segment whose text got concatenated into
    /// Pane's single `accumulated_text` save, per the pre-step finding) — must report
    /// `true` (echo, skip it).
    #[test]
    fn test_content_match_hit_substring_of_recent_local_row() {
        let conn = make_conn();
        seed_agent(&conn, "agent-1", Some("openclaw"), Some("main"));
        seed_session(&conn, "session-1", "agent-1");
        let now = sqlite_ts(chrono::Utc::now());
        seed_local_assistant_row(
            &conn,
            "session-1",
            "Good question — let me inventory what I have. Now let me check the details. Final answer here.",
            &now,
        );

        let result = is_recent_local_assistant_echo(&conn, "session-1", "Now let me check the details.")
            .expect("lookup should not error");
        assert!(result, "an incoming segment that's a substring of the recent local row must be reported as an echo");
    }

    /// Exact match (the final segment case — Pane's `accumulated_text` for a
    /// single-iteration turn equals the OC transcript's one `stop`-reason segment
    /// verbatim) also counts as a hit — substring containment includes exact equality.
    #[test]
    fn test_content_match_hit_exact_equality_counts_as_substring() {
        let conn = make_conn();
        seed_agent(&conn, "agent-1", Some("openclaw"), Some("main"));
        seed_session(&conn, "session-1", "agent-1");
        let now = sqlite_ts(chrono::Utc::now());
        seed_local_assistant_row(&conn, "session-1", "the final answer, verbatim", &now);

        let result = is_recent_local_assistant_echo(&conn, "session-1", "the final answer, verbatim")
            .expect("lookup should not error");
        assert!(result, "an exact match must also be reported as an echo (substring containment includes equality)");
    }

    // ── 2. Content-match miss (genuine new message) ──────────────────────────────────

    /// A genuinely new cron/proactive message (unrelated content, per §2b of Reed's
    /// audit — "Step 2 complete: Blue" style short canned replies) must NOT be
    /// suppressed just because SOME local assistant row exists in the window.
    #[test]
    fn test_content_match_miss_genuinely_new_cron_message() {
        let conn = make_conn();
        seed_agent(&conn, "agent-1", Some("openclaw"), Some("main"));
        seed_session(&conn, "session-1", "agent-1");
        let now = sqlite_ts(chrono::Utc::now());
        seed_local_assistant_row(&conn, "session-1", "Here's the answer to your inventory question.", &now);

        let result = is_recent_local_assistant_echo(&conn, "session-1", "Step 2 complete: Blue")
            .expect("lookup should not error");
        assert!(!result, "unrelated new content must not be suppressed — a genuine cron message must land");
    }

    /// No local assistant row exists at all for this thread — nothing to match
    /// against, so the check must report `false` (never suppress), not error.
    #[test]
    fn test_content_match_miss_no_local_rows_at_all() {
        let conn = make_conn();
        seed_agent(&conn, "agent-1", Some("openclaw"), Some("main"));
        seed_session(&conn, "session-1", "agent-1");

        let result = is_recent_local_assistant_echo(&conn, "session-1", "any incoming content")
            .expect("lookup should not error even with zero local rows");
        assert!(!result, "with no local assistant rows at all, nothing can be suppressed");
    }

    /// #2730 regression: replays the exact reported failure case — a 4-character
    /// first-sight cron delivery ("Blue", well below the 20-char `CONTENT_LENGTH_FLOOR`)
    /// that happens to be a genuine substring of the most recent LOCAL assistant row,
    /// arriving within the 150s backstop window. Before this fix, the backstop had no
    /// length floor (only an empty-string guard), so this short unrelated cron message
    /// would be false-suppressed as an echo purely because "Blue" is a substring of
    /// "The sky is Blue and the grass is green". The length floor must make the
    /// backstop skip the content-match check entirely for content this short and fail
    /// toward inserting (return `Ok(false)`) — matching Rule 2's identical posture.
    #[test]
    fn test_content_length_floor_short_substring_does_not_suppress() {
        let conn = make_conn();
        seed_agent(&conn, "agent-1", Some("openclaw"), Some("main"));
        seed_session(&conn, "session-1", "agent-1");
        // Within the 150s backstop window (60s ago).
        let recent_ts = sqlite_ts(chrono::Utc::now() - chrono::Duration::seconds(60));
        seed_local_assistant_row(
            &conn,
            "session-1",
            "The sky is Blue and the grass is green",
            &recent_ts,
        );

        // "Blue" is 4 chars — well below the 20-char CONTENT_LENGTH_FLOOR — and IS a
        // genuine substring of the local row above, so pre-fix this would have
        // returned `Ok(true)` (false-suppressed/tombstoned). Post-fix it must insert.
        let result = is_recent_local_assistant_echo(&conn, "session-1", "Blue")
            .expect("lookup should not error");
        assert!(
            !result,
            "a short (<20 char) incoming message must never be suppressed by the backstop, even when it IS a substring of a recent local row — #2730"
        );
    }

    // ── 3. Time-window scoping ────────────────────────────────────────────────

    /// A local assistant row that WOULD content-match, but is older than the
    /// 150s window, must NOT suppress the incoming message — old matches don't
    /// suppress (per the task's required test coverage). This is what keeps a
    /// short/canned genuine proactive reply arriving long after unrelated old
    /// history from being wrongly suppressed.
    #[test]
    fn test_time_window_scoping_old_match_does_not_suppress() {
        let conn = make_conn();
        seed_agent(&conn, "agent-1", Some("openclaw"), Some("main"));
        seed_session(&conn, "session-1", "agent-1");
        // 200 seconds ago — outside the 150s window.
        let old_ts = sqlite_ts(chrono::Utc::now() - chrono::Duration::seconds(200));
        seed_local_assistant_row(&conn, "session-1", "this content would otherwise match exactly", &old_ts);

        let result = is_recent_local_assistant_echo(&conn, "session-1", "this content would otherwise match exactly")
            .expect("lookup should not error");
        assert!(!result, "a content match older than the 150s window must NOT suppress — old matches don't suppress");
    }

    /// A local assistant row just inside the window (e.g. 140s ago) DOES still
    /// suppress — confirms the window boundary itself, not just "old rows never
    /// match" in the abstract.
    #[test]
    fn test_time_window_scoping_recent_match_within_window_does_suppress() {
        let conn = make_conn();
        seed_agent(&conn, "agent-1", Some("openclaw"), Some("main"));
        seed_session(&conn, "session-1", "agent-1");
        // 140 seconds ago — inside the 150s window.
        let recent_ts = sqlite_ts(chrono::Utc::now() - chrono::Duration::seconds(140));
        seed_local_assistant_row(&conn, "session-1", "still within the content-match window", &recent_ts);

        let result = is_recent_local_assistant_echo(&conn, "session-1", "still within the content-match window")
            .expect("lookup should not error");
        assert!(result, "a content match within the 150s window must still suppress");
    }

    /// The window check must be scoped per-thread — a recent matching row in a
    /// DIFFERENT thread must never suppress this thread's incoming message
    /// (rotation-safety / per-session-scoping constraint).
    #[test]
    fn test_content_match_scoped_to_thread_id_not_global() {
        let conn = make_conn();
        seed_agent(&conn, "agent-1", Some("openclaw"), Some("main"));
        seed_session(&conn, "session-1", "agent-1");
        seed_session(&conn, "session-2", "agent-1");
        let now = sqlite_ts(chrono::Utc::now());
        seed_local_assistant_row(&conn, "session-2", "shared identical text", &now);

        let result = is_recent_local_assistant_echo(&conn, "session-1", "shared identical text")
            .expect("lookup should not error");
        assert!(!result, "a matching row in a DIFFERENT thread must never suppress this thread's message");
    }

    // ── 4. #3053 item F: multi-segment turn's echoed text no longer drops anything ──

    /// End-to-end through `pull_oc_transcript_for_session`: a multi-segment turn (per
    /// §2a of Reed's audit — an intermediate commentary segment AND the final segment,
    /// both already present as a local row because Pane's own dispatch already saved
    /// the concatenated `accumulated_text`), plus a genuinely new cron message in the
    /// SAME poll batch.
    ///
    /// **Pre-#3053-item-F behavior (what this test used to assert, and what the base
    /// commit still does):** the content-match backstop caught the intermediate and
    /// final segments as "echoes" of the local dispatch row and SILENTLY DROPPED them
    /// — only the cron message landed. That is exactly the class of bug #3053 item F
    /// fixes: the two echoed segments are **distinct, real OC messages** (their own
    /// `__openclaw.id`s, never before seen) that happened to share text with something
    /// already local — same shape as a user quoting a prior message, or an agent
    /// restating earlier output. Text containment is correlation, not proof of
    /// duplication.
    ///
    /// **Post-#3053-item-F behavior (asserted below):** the backstop still runs and
    /// still logs/counts the match (`content_match_skipped_count` stays a useful
    /// diagnostic signal), but no longer removes anything from the batch — dedup is
    /// id/seq only now, and these three ids have never been classified before, so all
    /// three insert.
    #[tokio::test]
    #[serial(keychain_override)]
    async fn test_integration_multi_segment_turn_all_three_land_text_match_log_only() {
        let mut server = mockito::Server::new_async().await;
        let body = serde_json::json!({
            "messages": [
                {
                    "role": "assistant",
                    "content": "Good question — let me inventory what I have.",
                    "timestamp": 1_756_000_000_000i64,
                    "__openclaw": { "id": "oc-intermediate-1" }
                },
                {
                    "role": "assistant",
                    "content": "Here is my final complete answer to your question.",
                    "timestamp": 1_756_000_010_000i64,
                    "__openclaw": { "id": "oc-final-1" }
                },
                {
                    "role": "assistant",
                    "content": "Step 2 complete: Blue",
                    "timestamp": 1_756_000_020_000i64,
                    "__openclaw": { "id": "oc-cron-1" }
                }
            ]
        });
        let _m = server
            .mock(
                "GET",
                "/v1/sessions/agent%3Amain%3Aopenai-user%3Apane-session-integration/messages?limit=20",
            )
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(body.to_string())
            .create_async()
            .await;

        let conn = make_conn();
        seed_agent(&conn, "agent-oc", Some("openclaw"), Some("main"));
        seed_session(&conn, "session-integration", "agent-oc");
        crate::database::set_ai_setting(&conn, "endpoint", &server.url()).unwrap();
        conn.execute(
            "UPDATE chat_sessions SET oc_baseline_done_at = datetime('now') WHERE id = 'session-integration'",
            [],
        )
        .unwrap();
        // Simulates Pane's OWN dispatch-side save (§1a of the audit): the local row's
        // content is the CONCATENATION of the intermediate + final segments' text
        // (`accumulated_text.push_str` per iteration), NOT the cron message — that's
        // the genuinely new message this test proves still lands.
        let now = sqlite_ts(chrono::Utc::now());
        conn.execute(
            "INSERT INTO chat_messages_local (id, project_id, thread_id, role, content, created_at) \
             VALUES ('local-dispatch-1', '', 'session-integration', 'assistant', \
             'Good question — let me inventory what I have.Here is my final complete answer to your question.', ?1)",
            rusqlite::params![now],
        )
        .unwrap();

        let _kc = KeychainGuard::set();
        crate::ai::credential_migration::write_gateway_token(&conn, "test-token-integration").unwrap();

        let db = DbState::new(conn);

        // suppress_ingest=false: simulates a poll tick landing OUTSIDE the frontend's
        // grace window (Candidate 2 already let it through) — this exercises the
        // Candidate 3 backstop alone, proving it catches what Candidate 2 missed.
        let result = pull_oc_transcript_for_session("session-integration".to_string(), &db, false).await;

        assert!(result.is_ok(), "integration pull must not error: {:?}", result);
        let pull = result.unwrap();
        assert_eq!(pull.fetched_count, 3, "all 3 raw messages must be counted as fetched: {:?}", pull);
        assert_eq!(
            pull.content_match_skipped_count, 2,
            "the intermediate and final segments still trigger a LOGGED content-match observation (diagnostic only, #3053 item F) — the counter is retained for observability, it just no longer means \"dropped\": {:?}", pull
        );
        let ingest = pull.ingest.expect("ingest result must be present (suppress_ingest was false)");
        assert_eq!(ingest.inserted_count, 3, "#3053 item F: all three distinct messages must insert — text containment is log-only now, never a drop/tombstone decision: {:?}", ingest);

        let rows: Vec<(String, String)> = {
            let conn2 = db.0.lock().unwrap();
            let mut stmt = conn2
                .prepare("SELECT oc_message_id, content FROM chat_messages_local WHERE thread_id = 'session-integration' AND oc_message_id IS NOT NULL ORDER BY oc_message_id")
                .unwrap();
            stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().map(|r| r.unwrap()).collect()
        };
        assert_eq!(rows.len(), 3, "all three externally-ingested rows must exist — the two previously-echoed segments AND the cron message, none silently dropped: {:?}", rows);
        assert_eq!(
            rows,
            vec![
                ("oc-cron-1".to_string(), "Step 2 complete: Blue".to_string()),
                ("oc-final-1".to_string(), "Here is my final complete answer to your question.".to_string()),
                ("oc-intermediate-1".to_string(), "Good question — let me inventory what I have.".to_string()),
            ]
        );

        // Confirm no tombstone was written for either previously-echoed segment —
        // #3053 item F: `backstop_echo` is no longer written by any live code path.
        let tombstone_count: i64 = {
            let conn2 = db.0.lock().unwrap();
            conn2.query_row(
                "SELECT COUNT(*) FROM chat_oc_tombstones WHERE thread_id = 'session-integration'",
                [],
                |r| r.get(0),
            ).unwrap()
        };
        assert_eq!(tombstone_count, 0, "no tombstone of any reason should exist — backstop_echo is never written any more");
    }

    // ── 5. suppress_ingest "defer, don't skip" contract ────────────────────────────

    /// `suppress_ingest=true` must still perform the gateway fetch (fetched_count
    /// reflects the real response) but must NOT call ingestion at all — `ingest` comes
    /// back `None`. This proves the fetch is never skipped ("defer, don't skip"): a
    /// genuine cron message present in this same response is simply deferred, not lost.
    #[tokio::test]
    #[serial(keychain_override)]
    async fn test_suppress_ingest_defers_fetch_still_runs_ingest_withheld() {
        let mut server = mockito::Server::new_async().await;
        let body = serde_json::json!({
            "messages": [{
                "role": "assistant",
                "content": "a genuine cron message that must not be lost",
                "timestamp": 1_756_000_000_000i64,
                "__openclaw": { "id": "oc-deferred-1" }
            }]
        });
        let _m = server
            .mock(
                "GET",
                "/v1/sessions/agent%3Amain%3Aopenai-user%3Apane-session-defer/messages?limit=20",
            )
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(body.to_string())
            .create_async()
            .await;

        let conn = make_conn();
        seed_agent(&conn, "agent-oc", Some("openclaw"), Some("main"));
        seed_session(&conn, "session-defer", "agent-oc");
        crate::database::set_ai_setting(&conn, "endpoint", &server.url()).unwrap();
        conn.execute(
            "UPDATE chat_sessions SET oc_baseline_done_at = datetime('now') WHERE id = 'session-defer'",
            [],
        )
        .unwrap();

        let _kc = KeychainGuard::set();
        crate::ai::credential_migration::write_gateway_token(&conn, "test-token-defer").unwrap();

        let db = DbState::new(conn);

        // First call: suppress_ingest=true (simulates a tick landing inside the
        // frontend's grace window).
        let suppressed = pull_oc_transcript_for_session("session-defer".to_string(), &db, true).await;
        assert!(suppressed.is_ok(), "a suppressed pull must not error: {:?}", suppressed);
        let suppressed_pull = suppressed.unwrap();
        assert_eq!(suppressed_pull.fetched_count, 1, "the fetch must still run and report the real fetched_count even while suppressed");
        assert!(suppressed_pull.ingest.is_none(), "ingest must be withheld entirely while suppress_ingest is true");
        assert!(
            suppressed_pull.skip_reason.as_ref().unwrap().contains("suppress"),
            "skip_reason must explain the suppression: {:?}", suppressed_pull.skip_reason
        );

        let row_count_after_suppressed: i64 = {
            let conn2 = db.0.lock().unwrap();
            conn2.query_row("SELECT COUNT(*) FROM chat_messages_local WHERE thread_id = 'session-defer'", [], |r| r.get(0)).unwrap()
        };
        assert_eq!(row_count_after_suppressed, 0, "nothing must be inserted while suppressed");

        // Second call: suppress_ingest=false (next eligible, non-suppressed tick) — the
        // SAME message (no cursor, §30.11.3) must now ingest normally. Proves "defer,
        // not skip": the message was delayed, never dropped.
        let resumed = pull_oc_transcript_for_session("session-defer".to_string(), &db, false).await;
        assert!(resumed.is_ok(), "the resumed pull must not error: {:?}", resumed);
        let resumed_pull = resumed.unwrap();
        let resumed_ingest = resumed_pull.ingest.expect("ingest result must be present once suppression lifts");
        assert_eq!(resumed_ingest.inserted_count, 1, "the deferred message must now insert on the next eligible tick: {:?}", resumed_ingest);

        let row_count_after_resumed: i64 = {
            let conn2 = db.0.lock().unwrap();
            conn2.query_row("SELECT COUNT(*) FROM chat_messages_local WHERE thread_id = 'session-defer'", [], |r| r.get(0)).unwrap()
        };
        assert_eq!(row_count_after_resumed, 1, "the previously-deferred message must have inserted exactly once");
    }
}


/// Stateful echo suppression — tombstoning (#2410, `docs/audits/2026-08-24-tombstone-scope.md`):
/// tests for `is_already_classified` (Rule 1 pre-filter), `check_stale_echo` (refined
/// Rule 2), `write_tombstone`, and the full pipeline wiring in
/// `pull_oc_transcript_for_session`. Separate module for the same reason as the
/// sibling `echo_suppression_tests`/`oc_pull_baseline_and_role_filter_tests` modules
/// — reuses `make_conn`/`seed_agent`/`seed_session`/`sqlite_ts`/`seed_local_assistant_row`
/// via its own copies, matching this file's existing test-module convention.
#[cfg(test)]
mod tombstoning_tests {
    use super::*;
    use crate::keychain::test_support::KeychainGuard;
    use serial_test::serial;

    fn make_conn() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().expect("in-memory db");
        crate::migrations::run_migrations(&conn).expect("fresh-install migrations should succeed");
        conn
    }

    fn seed_agent(conn: &rusqlite::Connection, id: &str, runtime_type: Option<&str>, runtime_ref: Option<&str>) {
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO ai_agents (id, name, slug, is_default, is_active, workspace_path, runtime_type, runtime_ref, model, created_at, updated_at)
             VALUES (?1, ?1, ?1, 0, 1, ?1, ?2, ?3, 'test-model', ?4, ?4)",
            rusqlite::params![id, runtime_type, runtime_ref, now],
        )
        .expect("seed agent");
    }

    fn seed_session(conn: &rusqlite::Connection, id: &str, agent_id: &str) {
        conn.execute(
            "INSERT INTO chat_sessions (id, project_id, agent_id) VALUES (?1, 'workspace', ?2)",
            rusqlite::params![id, agent_id],
        )
        .expect("seed session");
    }

    /// Same convention as `echo_suppression_tests::sqlite_ts` — production always
    /// writes `chat_messages_local.created_at` via SQLite's own `datetime('now')`
    /// (space-separated), never chrono's RFC3339 (`T`-separated) — fixtures must match.
    fn sqlite_ts(dt: chrono::DateTime<chrono::Utc>) -> String {
        dt.format("%Y-%m-%d %H:%M:%S").to_string()
    }

    fn seed_local_assistant_row(conn: &rusqlite::Connection, thread_id: &str, content: &str, created_at: &str) {
        conn.execute(
            "INSERT INTO chat_messages_local (id, project_id, thread_id, role, content, created_at) \
             VALUES (?1, '', ?2, ?3, ?4, ?5)",
            rusqlite::params![format!("local-{}", uuid::Uuid::new_v4()), thread_id, "assistant", content, created_at],
        )
        .expect("seed local assistant row");
    }

    // ── 1. is_already_classified unit coverage ──────────────────────────────────

    #[test]
    fn test_is_already_classified_false_when_neither_row_nor_tombstone_exists() {
        let conn = make_conn();
        seed_agent(&conn, "agent-1", Some("openclaw"), Some("main"));
        seed_session(&conn, "session-1", "agent-1");

        let result = is_already_classified(&conn, "session-1", "oc-msg-1").expect("lookup should not error");
        assert!(!result, "a never-seen (thread_id, oc_message_id) pair must not be classified");
    }

    #[test]
    fn test_is_already_classified_true_when_real_row_exists() {
        let conn = make_conn();
        seed_agent(&conn, "agent-1", Some("openclaw"), Some("main"));
        seed_session(&conn, "session-1", "agent-1");
        conn.execute(
            "INSERT INTO chat_messages_local (id, project_id, thread_id, role, content, oc_message_id) \
             VALUES ('local-1', '', 'session-1', 'assistant', 'hi', 'oc-msg-1')",
            [],
        )
        .unwrap();

        let result = is_already_classified(&conn, "session-1", "oc-msg-1").expect("lookup should not error");
        assert!(result, "an already-ingested oc_message_id must report as classified");
    }

    #[test]
    fn test_is_already_classified_true_when_tombstoned() {
        let conn = make_conn();
        seed_agent(&conn, "agent-1", Some("openclaw"), Some("main"));
        seed_session(&conn, "session-1", "agent-1");
        write_tombstone(&conn, "session-1", "oc-msg-1", "backstop_echo", "echoed text", None, None).unwrap();

        let result = is_already_classified(&conn, "session-1", "oc-msg-1").expect("lookup should not error");
        assert!(result, "a previously-tombstoned oc_message_id must report as classified");
    }

    #[test]
    fn test_is_already_classified_scoped_to_thread_id() {
        let conn = make_conn();
        seed_agent(&conn, "agent-1", Some("openclaw"), Some("main"));
        seed_session(&conn, "session-1", "agent-1");
        seed_session(&conn, "session-2", "agent-1");
        write_tombstone(&conn, "session-2", "oc-msg-shared", "backstop_echo", "text", None, None).unwrap();

        let result = is_already_classified(&conn, "session-1", "oc-msg-shared").expect("lookup should not error");
        assert!(!result, "a tombstone in a DIFFERENT thread must never mark this thread's message as classified");
    }

    // ── 2. Tick-3 re-fetch dedup: id-based now, not backstop-tombstone-based ─────

    /// Reproduces §0a/§4's fixture walk directly against the real pipeline, UPDATED
    /// for #3053 item F: tick 2's "echo" (`oc-echo-1`, content matching the local
    /// dispatch row) is a genuinely distinct, never-before-seen OC message — it now
    /// INSERTS for real (text containment is log-only, never a drop/tombstone
    /// decision). Tick 3 re-fetches the exact same OC page (no cursor, §30.11.3) —
    /// both ids are, by then, real rows in `chat_messages_local` from tick 2's own
    /// insert, so the id-based Rule 1 permanence pre-filter (`is_already_classified`)
    /// catches both and tick 3 inserts zero. This is the same "no duplicate on
    /// re-fetch" guarantee the original tick-3-leak test proved, but via the id-based
    /// mechanism this fix retains, not the backstop tombstone this fix removes.
    #[tokio::test]
    #[serial(keychain_override)]
    async fn test_tick3_refetch_dedup_is_id_based_not_backstop_tombstone_based() {
        let mut server = mockito::Server::new_async().await;

        let conn = make_conn();
        seed_agent(&conn, "agent-oc", Some("openclaw"), Some("main"));
        seed_session(&conn, "session-tick3", "agent-oc");
        crate::database::set_ai_setting(&conn, "endpoint", &server.url()).unwrap();
        conn.execute(
            "UPDATE chat_sessions SET oc_baseline_done_at = datetime('now') WHERE id = 'session-tick3'",
            [],
        )
        .unwrap();
        // Simulates Pane's own dispatch-side save for the echoed turn (§1a of the audit).
        let now = sqlite_ts(chrono::Utc::now());
        conn.execute(
            "INSERT INTO chat_messages_local (id, project_id, thread_id, role, content, created_at) \
             VALUES ('local-dispatch-1', '', 'session-tick3', 'assistant', 'The full concatenated turn text.', ?1)",
            rusqlite::params![now],
        )
        .unwrap();

        let _kc = KeychainGuard::set();
        crate::ai::credential_migration::write_gateway_token(&conn, "test-token-tick3").unwrap();

        let db = DbState::new(conn);

        // Tick 2: OC returns the echo segment (already covered by the dispatch-side
        // save above) plus a genuinely new message.
        let tick2_body = serde_json::json!({
            "messages": [
                { "role": "assistant", "content": "The full concatenated turn text.", "timestamp": 1_756_000_000_000i64, "__openclaw": { "id": "oc-echo-1" } },
                { "role": "assistant", "content": "A genuinely new insert.", "timestamp": 1_756_000_010_000i64, "__openclaw": { "id": "oc-new-1" } }
            ]
        });
        let m_tick2 = server
            .mock("GET", "/v1/sessions/agent%3Amain%3Aopenai-user%3Apane-session-tick3/messages?limit=20")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(tick2_body.to_string())
            .expect(1)
            .create_async()
            .await;

        let tick2 = pull_oc_transcript_for_session("session-tick3".to_string(), &db, false).await.unwrap();
        m_tick2.assert_async().await;
        assert_eq!(
            tick2.content_match_skipped_count, 1,
            "the content match is still observed and logged (diagnostic only, #3053 item F): {:?}", tick2
        );
        let tick2_ingest = tick2.ingest.expect("ingest must run at tick 2");
        assert_eq!(
            tick2_ingest.inserted_count, 2,
            "#3053 item F: BOTH messages insert at tick 2 — oc-echo-1 is a genuinely new, never-before-seen id, and text containment no longer drops it: {:?}", tick2_ingest
        );

        // #3053 item F: confirm NO tombstone was written for oc-echo-1 — backstop_echo
        // is no longer written by any live code path.
        let tombstoned: bool = {
            let conn2 = db.0.lock().unwrap();
            conn2.query_row(
                "SELECT EXISTS(SELECT 1 FROM chat_oc_tombstones WHERE thread_id = 'session-tick3' AND oc_message_id = 'oc-echo-1')",
                [],
                |r| r.get(0),
            ).unwrap()
        };
        assert!(!tombstoned, "#3053 item F: a content-match hit must NOT write a chat_oc_tombstones row any more");

        // Tick 3: OC returns the SAME page again (no cursor, §30.11.3) — both ids are
        // now real rows in chat_messages_local from tick 2's own insert. The id-based
        // Rule 1 permanence pre-filter (`is_already_classified`) catches both this
        // time — not a tombstone, a real row — so tick 3 still inserts zero.
        let tick3_body = serde_json::json!({
            "messages": [
                { "role": "assistant", "content": "The full concatenated turn text.", "timestamp": 1_756_000_000_000i64, "__openclaw": { "id": "oc-echo-1" } },
                { "role": "assistant", "content": "A genuinely new insert.", "timestamp": 1_756_000_010_000i64, "__openclaw": { "id": "oc-new-1" } }
            ]
        });
        let _m_tick3 = server
            .mock("GET", "/v1/sessions/agent%3Amain%3Aopenai-user%3Apane-session-tick3/messages?limit=20")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(tick3_body.to_string())
            .create_async()
            .await;

        let tick3 = pull_oc_transcript_for_session("session-tick3".to_string(), &db, false).await.unwrap();

        assert_eq!(
            tick3.already_classified_count, 2,
            "both ids must be id-pre-filtered at tick 3 — both are now real rows in chat_messages_local from tick 2's insert (not a tombstone this time): {:?}", tick3
        );
        assert_eq!(tick3.content_match_skipped_count, 0, "nothing new reaches the backstop at tick 3 — the id pre-filter already dropped both: {:?}", tick3);
        let tick3_ingest = tick3.ingest.expect("ingest must still run (with an empty/near-empty batch) at tick 3");
        assert_eq!(tick3_ingest.inserted_count, 0, "ZERO spurious inserts at tick 3 — the id-based pre-filter still fully prevents duplicate rows on re-fetch: {:?}", tick3_ingest);

        let row_count: i64 = {
            let conn2 = db.0.lock().unwrap();
            conn2.query_row("SELECT COUNT(*) FROM chat_messages_local WHERE thread_id = 'session-tick3' AND oc_message_id IS NOT NULL", [], |r| r.get(0)).unwrap()
        };
        assert_eq!(row_count, 2, "#3053 item F: BOTH externally-ingested rows exist across both ticks — oc-echo-1 AND oc-new-1, neither silently dropped, no duplicates from the re-fetch");
    }

    // ── 3. Resurrection tombstones via content match (refined Rule 2 hit) ───────

    /// A 107-minute-old segment (well past the 15-min staleness gate) whose content
    /// exists in a local assistant row within ±5min of ITS OWN timestamp gets
    /// tombstoned as `stale_echo` — not inserted.
    #[test]
    fn test_resurrection_tombstones_via_content_match_within_5min_anchor() {
        let conn = make_conn();
        seed_agent(&conn, "agent-1", Some("openclaw"), Some("main"));
        seed_session(&conn, "session-1", "agent-1");

        // The incoming message's own claimed timestamp: 107 minutes ago.
        let msg_time = chrono::Utc::now() - chrono::Duration::minutes(107);
        let msg_timestamp = msg_time.to_rfc3339();

        // The matching local row was written 3 minutes AFTER the message's own
        // timestamp (within the ±5min anchor window) — simulates Pane's own
        // dispatch-side save of the same content, near the same real-world moment.
        let local_created_at = sqlite_ts(msg_time + chrono::Duration::minutes(3));
        seed_local_assistant_row(&conn, "session-1", "This is the resurrected content that already landed once.", &local_created_at);

        let result = check_stale_echo(&conn, "session-1", "This is the resurrected content that already landed once.", &msg_timestamp)
            .expect("lookup should not error");
        assert!(result.is_some(), "a 107-min-old message whose content matches a local row within ±5min of its own timestamp must be flagged (#3053 item F: observed/logged only — see the pipeline-level test below confirming it still inserts)");
        let (matched_created_at, age_secs) = result.unwrap();
        assert_eq!(matched_created_at, local_created_at, "the returned matched-row created_at must be the local row's own created_at");
        assert!(age_secs >= 900.0, "a 107-minute-old message must report age_secs >= STALE_THRESHOLD_SECS (900) — this is a resurrection catch, not a dead-band catch: {}", age_secs);
    }

    /// End-to-end: the same resurrection scenario through `pull_oc_transcript_for_session`
    /// — **#3053 item F:** confirms the content match is still observed/logged (the
    /// counter still increments), but the message now INSERTS — it is no longer
    /// tombstoned, and no `chat_oc_tombstones` row is written for it. A 107-minute-old
    /// message whose text happens to match something already local is exactly the
    /// "genuine late delivery that happens to restate prior content" shape text
    /// matching cannot safely distinguish from a real echo — dedup is id/seq only.
    #[tokio::test]
    #[serial(keychain_override)]
    async fn test_resurrection_content_match_logged_but_message_still_lands() {
        let mut server = mockito::Server::new_async().await;

        let conn = make_conn();
        seed_agent(&conn, "agent-oc", Some("openclaw"), Some("main"));
        seed_session(&conn, "session-resurrect", "agent-oc");
        crate::database::set_ai_setting(&conn, "endpoint", &server.url()).unwrap();
        conn.execute(
            "UPDATE chat_sessions SET oc_baseline_done_at = datetime('now') WHERE id = 'session-resurrect'",
            [],
        )
        .unwrap();

        let msg_time = chrono::Utc::now() - chrono::Duration::minutes(107);
        let local_created_at = sqlite_ts(msg_time + chrono::Duration::minutes(2));
        seed_local_assistant_row(&conn, "session-resurrect", "The 1:15pm resurrected segment content.", &local_created_at);

        let body = serde_json::json!({
            "messages": [{
                "role": "assistant",
                "content": "The 1:15pm resurrected segment content.",
                "timestamp": msg_time.timestamp_millis(),
                "__openclaw": { "id": "oc-resurrect-1" }
            }]
        });
        let _m = server
            .mock("GET", "/v1/sessions/agent%3Amain%3Aopenai-user%3Apane-session-resurrect/messages?limit=20")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(body.to_string())
            .create_async()
            .await;

        let _kc = KeychainGuard::set();
        crate::ai::credential_migration::write_gateway_token(&conn, "test-token-resurrect").unwrap();

        let db = DbState::new(conn);
        let result = pull_oc_transcript_for_session("session-resurrect".to_string(), &db, false).await.unwrap();

        assert_eq!(
            result.stale_echo_tombstoned_count, 1,
            "the content match is still observed and logged (diagnostic only, #3053 item F): {:?}", result
        );
        let ingest = result.ingest.expect("ingest must run");
        assert_eq!(ingest.inserted_count, 1, "#3053 item F: the resurrected segment must insert — a text match is no longer a drop/tombstone decision: {:?}", ingest);

        let row_exists: bool = {
            let conn2 = db.0.lock().unwrap();
            conn2.query_row(
                "SELECT EXISTS(SELECT 1 FROM chat_oc_tombstones WHERE thread_id = 'session-resurrect' AND oc_message_id = 'oc-resurrect-1')",
                [],
                |r| r.get(0),
            ).unwrap()
        };
        assert!(!row_exists, "#3053 item F: no chat_oc_tombstones row must be written for a text-containment match any more");
    }

    
    // ── 3b. Dead-band echo (#2725 regression) ────────────────────────────────

    /// #2725 core regression: a first-sight message aged ~3 minutes (180s — strictly
    /// between the 150s backstop window and the 900s `STALE_THRESHOLD_SECS`
    /// resurrection threshold) whose content matches a local assistant row within
    /// ±5min of its OWN claimed timestamp must be flagged for `stale_echo`
    /// tombstoning. Before #2725, Step A's age gate (`if age_secs <
    /// STALE_THRESHOLD_SECS { return Ok(None); }`) short-circuited `check_stale_echo`
    /// before Steps B/C ever ran for any message this young, so this exact case fell
    /// through both the backstop (whose 150s trailing window had already aged out by
    /// the time this message was re-ingested) and Rule 2 (gated behind 900s) —
    /// inserting as a genuine new message. This is today's production leak
    /// (`docs/audits/2026-08-26-echo-dead-band-scope.md` §0's `ext-3c3d3a01` /
    /// `oc_msg_id 5a840c84` evidence), reproduced directly as a regression test.
    #[test]
    fn test_dead_band_echo_180s_old_tombstones_via_content_match() {
        let conn = make_conn();
        seed_agent(&conn, "agent-1", Some("openclaw"), Some("main"));
        seed_session(&conn, "session-1", "agent-1");

        // The incoming message's own claimed timestamp: 180 seconds ago (150s <
        // 180s < 900s — squarely in the dead band).
        let msg_time = chrono::Utc::now() - chrono::Duration::seconds(180);
        let msg_timestamp = msg_time.to_rfc3339();

        // Pane's own dispatch-side save of the same content, written 30s after the
        // message's own claimed timestamp (well within the ±5min anchor) — the
        // 21s-dispatch-latency shape from the issue's own evidence.
        let local_created_at = sqlite_ts(msg_time + chrono::Duration::seconds(30));
        seed_local_assistant_row(&conn, "session-1", "This exact reply text was already saved locally moments ago.", &local_created_at);

        let result = check_stale_echo(&conn, "session-1", "This exact reply text was already saved locally moments ago.", &msg_timestamp)
            .expect("lookup should not error");
        assert!(result.is_some(), "a ~180s-old message whose content matches a local row within ±5min of its own timestamp must be flagged (#3053 item F: observed/logged only) — this is the #2725 dead band shape");
        let (matched_created_at, age_secs) = result.unwrap();
        assert_eq!(matched_created_at, local_created_at, "the returned matched-row created_at must be the local row's own created_at");
        assert!(
            age_secs >= 150.0 && age_secs < 900.0,
            "a ~180s-old message must report age_secs squarely inside the dead band (150s-900s), confirming this is a dead-band catch, not a resurrection catch: {}", age_secs
        );
    }

    /// End-to-end: the same dead-band scenario through `pull_oc_transcript_for_session`
    /// — **#3053 item F:** confirms the match is still observed/logged but the message
    /// now inserts, with no tombstone written. The #2725 dead-band fix (widening WHEN
    /// the containment check runs) is unaffected by this change; what changed is
    /// purely what happens on a match (log vs. drop).
    #[tokio::test]
    #[serial(keychain_override)]
    async fn test_dead_band_echo_content_match_logged_but_message_still_lands() {
        let mut server = mockito::Server::new_async().await;

        let conn = make_conn();
        seed_agent(&conn, "agent-oc", Some("openclaw"), Some("main"));
        seed_session(&conn, "session-dead-band", "agent-oc");
        crate::database::set_ai_setting(&conn, "endpoint", &server.url()).unwrap();
        conn.execute(
            "UPDATE chat_sessions SET oc_baseline_done_at = datetime('now') WHERE id = 'session-dead-band'",
            [],
        )
        .unwrap();

        // 3 minutes old — dead band. Local dispatch-side save landed 21s after the
        // message's own claimed timestamp (mirrors the issue's own dispatch-latency
        // evidence), well within the ±5min anchor but well outside the backstop's
        // 150s trailing window by the time this poll tick runs.
        let msg_time = chrono::Utc::now() - chrono::Duration::seconds(180);
        let local_created_at = sqlite_ts(msg_time + chrono::Duration::seconds(21));
        seed_local_assistant_row(&conn, "session-dead-band", "Brit's assistant reply, re-ingested by a later poll tick.", &local_created_at);

        let body = serde_json::json!({
            "messages": [{
                "role": "assistant",
                "content": "Brit's assistant reply, re-ingested by a later poll tick.",
                "timestamp": msg_time.timestamp_millis(),
                "__openclaw": { "id": "oc-dead-band-1" }
            }]
        });
        let _m = server
            .mock("GET", "/v1/sessions/agent%3Amain%3Aopenai-user%3Apane-session-dead-band/messages?limit=20")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(body.to_string())
            .create_async()
            .await;

        let _kc = KeychainGuard::set();
        crate::ai::credential_migration::write_gateway_token(&conn, "test-token-dead-band").unwrap();

        let db = DbState::new(conn);
        let result = pull_oc_transcript_for_session("session-dead-band".to_string(), &db, false).await.unwrap();

        assert_eq!(
            result.stale_echo_tombstoned_count, 1,
            "the dead-band content match is still observed and logged (diagnostic only, #3053 item F): {:?}", result
        );
        assert_eq!(result.content_match_skipped_count, 0, "the 150s backstop is not the one observing this — by construction the local row is already >150s old by the time this pull runs: {:?}", result);
        let ingest = result.ingest.expect("ingest must run");
        assert_eq!(ingest.inserted_count, 1, "#3053 item F: the dead-band echo must insert — text containment is no longer a drop/tombstone decision: {:?}", ingest);

        let row_exists: bool = {
            let conn2 = db.0.lock().unwrap();
            conn2.query_row(
                "SELECT EXISTS(SELECT 1 FROM chat_oc_tombstones WHERE thread_id = 'session-dead-band' AND oc_message_id = 'oc-dead-band-1')",
                [],
                |r| r.get(0),
            ).unwrap()
        };
        assert!(!row_exists, "#3053 item F: no chat_oc_tombstones row must be written for a text-containment match any more");
    }

    /// The "moving reference" shape (#2725, active-conversation echo): the anchor
    /// check must find the RIGHT matching local row even when it is NOT the most
    /// recent assistant row in the thread — unlike the 150s backstop
    /// (`is_recent_local_assistant_echo`), which only ever considers a single
    /// `ORDER BY created_at DESC LIMIT 1` row, `check_stale_echo`'s Step C scans
    /// every candidate row within ±5min of the INCOMING message's own timestamp and
    /// checks each for containment, so a genuinely newer (but content-unrelated)
    /// assistant row elsewhere in the thread does not shadow the correct match.
    #[test]
    fn test_moving_reference_anchor_finds_correct_row_despite_newer_rows_in_thread() {
        let conn = make_conn();
        seed_agent(&conn, "agent-1", Some("openclaw"), Some("main"));
        seed_session(&conn, "session-1", "agent-1");

        // The incoming echo's own claimed timestamp: 4 minutes ago (dead band).
        let msg_time = chrono::Utc::now() - chrono::Duration::minutes(4);
        let msg_timestamp = msg_time.to_rfc3339();

        // Row A: an OLD, unrelated local assistant row, outside the ±5min anchor
        // window entirely — must never be considered.
        let row_a_created_at = sqlite_ts(msg_time - chrono::Duration::minutes(20));
        seed_local_assistant_row(&conn, "session-1", "An old, unrelated status update from twenty minutes earlier.", &row_a_created_at);

        // Row B: the TRUE match — Pane's own dispatch-side save of the echoed turn,
        // written 90s after the incoming message's own timestamp (within the ±5min
        // anchor).
        let row_b_created_at = sqlite_ts(msg_time + chrono::Duration::seconds(90));
        seed_local_assistant_row(&conn, "session-1", "The specific turn content this incoming echo is a re-ingest of.", &row_b_created_at);

        // Row C: the MOST RECENT local assistant row in the thread — a genuinely
        // newer, unrelated reply from an ACTIVE ongoing conversation (e.g. the agent
        // has already moved on to a different topic by the time this poll tick
        // lands). Written only 20s ago — far newer than row B, and would be the
        // (wrong) row the 150s backstop's `ORDER BY created_at DESC LIMIT 1` would
        // consider instead.
        let row_c_created_at = sqlite_ts(chrono::Utc::now() - chrono::Duration::seconds(20));
        seed_local_assistant_row(&conn, "session-1", "A completely different, much newer reply about an unrelated topic.", &row_c_created_at);

        let result = check_stale_echo(&conn, "session-1", "The specific turn content this incoming echo is a re-ingest of.", &msg_timestamp)
            .expect("lookup should not error");
        assert!(result.is_some(), "the anchor check must find row B's match even though it is not the most recent assistant row in the thread");
        let (matched_created_at, _age_secs) = result.unwrap();
        assert_eq!(matched_created_at, row_b_created_at, "the anchor check must return row B (the actual matching row within ±5min of the incoming message's own timestamp), not row C (the thread's most-recent row) or row A (outside the anchor window)");
    }

    /// End-to-end variant of the moving-reference case — **#3053 item F:** confirms
    /// the full pipeline still OBSERVES/LOGS the match against row B (the anchor
    /// check logic itself is unchanged by this fix) even with a newer, unrelated
    /// local assistant row (row C) present in the same thread, but the message now
    /// INSERTS rather than being tombstoned — dedup is id/seq only.
    #[tokio::test]
    #[serial(keychain_override)]
    async fn test_moving_reference_content_match_logged_but_message_still_lands() {
        let mut server = mockito::Server::new_async().await;

        let conn = make_conn();
        seed_agent(&conn, "agent-oc", Some("openclaw"), Some("main"));
        seed_session(&conn, "session-moving-ref", "agent-oc");
        crate::database::set_ai_setting(&conn, "endpoint", &server.url()).unwrap();
        conn.execute(
            "UPDATE chat_sessions SET oc_baseline_done_at = datetime('now') WHERE id = 'session-moving-ref'",
            [],
        )
        .unwrap();

        let msg_time = chrono::Utc::now() - chrono::Duration::minutes(4);

        // The TRUE match for the echo, within ±5min of the incoming message's own
        // timestamp.
        let row_b_created_at = sqlite_ts(msg_time + chrono::Duration::seconds(90));
        seed_local_assistant_row(&conn, "session-moving-ref", "The specific turn content this incoming echo is a re-ingest of.", &row_b_created_at);

        // A newer, unrelated local assistant row — the thread's most-recent row,
        // which the 150s backstop alone would have (wrongly) compared against.
        let row_c_created_at = sqlite_ts(chrono::Utc::now() - chrono::Duration::seconds(20));
        seed_local_assistant_row(&conn, "session-moving-ref", "A completely different, much newer reply about an unrelated topic.", &row_c_created_at);

        let body = serde_json::json!({
            "messages": [{
                "role": "assistant",
                "content": "The specific turn content this incoming echo is a re-ingest of.",
                "timestamp": msg_time.timestamp_millis(),
                "__openclaw": { "id": "oc-moving-ref-1" }
            }]
        });
        let _m = server
            .mock("GET", "/v1/sessions/agent%3Amain%3Aopenai-user%3Apane-session-moving-ref/messages?limit=20")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(body.to_string())
            .create_async()
            .await;

        let _kc = KeychainGuard::set();
        crate::ai::credential_migration::write_gateway_token(&conn, "test-token-moving-ref").unwrap();

        let db = DbState::new(conn);
        let result = pull_oc_transcript_for_session("session-moving-ref".to_string(), &db, false).await.unwrap();

        assert_eq!(
            result.stale_echo_tombstoned_count, 1,
            "the match against row B is still observed and logged despite a newer, unrelated local row (row C) existing in the same thread: {:?}", result
        );
        let ingest = result.ingest.expect("ingest must run");
        assert_eq!(ingest.inserted_count, 1, "#3053 item F: the echo must insert — a text match (even the \"correct anchor\" kind) is no longer a drop/tombstone decision: {:?}", ingest);

        let row_exists: bool = {
            let conn2 = db.0.lock().unwrap();
            conn2.query_row(
                "SELECT EXISTS(SELECT 1 FROM chat_oc_tombstones WHERE thread_id = 'session-moving-ref' AND oc_message_id = 'oc-moving-ref-1')",
                [],
                |r| r.get(0),
            ).unwrap()
        };
        assert!(!row_exists, "#3053 item F: no chat_oc_tombstones row must be written for a text-containment match any more");
    }

    // ── 4. Genuine late delivery inserts (§30.11.4 compliance) ──────────────────

    /// A 40-min-old segment with NOVEL content (no matching local row anywhere within
    /// ±5min of its own timestamp) passes the containment check and inserts — this is
    /// the exact scenario §6.5 of the audit walks: app closed 40min, agent finishes,
    /// user reopens.
    #[test]
    fn test_genuine_late_delivery_novel_content_passes_containment() {
        let conn = make_conn();
        seed_agent(&conn, "agent-1", Some("openclaw"), Some("main"));
        seed_session(&conn, "session-1", "agent-1");

        let msg_time = chrono::Utc::now() - chrono::Duration::minutes(40);
        let msg_timestamp = msg_time.to_rfc3339();

        // No local rows at all for this thread — nothing to match against.
        let result = check_stale_echo(&conn, "session-1", "Completely novel content generated while the app was closed.", &msg_timestamp)
            .expect("lookup should not error");
        assert!(result.is_none(), "novel content with no matching local row must NOT be tombstoned — must proceed to insert (§30.11.4)");
    }

    /// End-to-end: confirms the 40-min-old genuinely novel message actually lands in
    /// `chat_messages_local` through the full pipeline.
    #[tokio::test]
    #[serial(keychain_override)]
    async fn test_genuine_late_delivery_inserts_end_to_end() {
        let mut server = mockito::Server::new_async().await;

        let conn = make_conn();
        seed_agent(&conn, "agent-oc", Some("openclaw"), Some("main"));
        seed_session(&conn, "session-late", "agent-oc");
        crate::database::set_ai_setting(&conn, "endpoint", &server.url()).unwrap();
        conn.execute(
            "UPDATE chat_sessions SET oc_baseline_done_at = datetime('now') WHERE id = 'session-late'",
            [],
        )
        .unwrap();

        let msg_time = chrono::Utc::now() - chrono::Duration::minutes(40);
        let body = serde_json::json!({
            "messages": [{
                "role": "assistant",
                "content": "Finished the task while you were away — here is the summary.",
                "timestamp": msg_time.timestamp_millis(),
                "__openclaw": { "id": "oc-late-1" }
            }]
        });
        let _m = server
            .mock("GET", "/v1/sessions/agent%3Amain%3Aopenai-user%3Apane-session-late/messages?limit=20")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(body.to_string())
            .create_async()
            .await;

        let _kc = KeychainGuard::set();
        crate::ai::credential_migration::write_gateway_token(&conn, "test-token-late").unwrap();

        let db = DbState::new(conn);
        let result = pull_oc_transcript_for_session("session-late".to_string(), &db, false).await.unwrap();

        assert_eq!(result.stale_echo_tombstoned_count, 0, "genuinely novel content must not be tombstoned: {:?}", result);
        let ingest = result.ingest.expect("ingest must run");
        assert_eq!(ingest.inserted_count, 1, "the genuine late delivery must insert: {:?}", ingest);

        let row_count: i64 = {
            let conn2 = db.0.lock().unwrap();
            conn2.query_row("SELECT COUNT(*) FROM chat_messages_local WHERE thread_id = 'session-late' AND oc_message_id = 'oc-late-1'", [], |r| r.get(0)).unwrap()
        };
        assert_eq!(row_count, 1, "the late-delivered message must land in chat_messages_local");
    }

    // ── 5. Short-content skips containment (length floor) ───────────────────────

    /// A stale message with content under 20 chars bypasses the containment check
    /// entirely (Step B, the length floor) — even though a matching local row DOES
    /// exist, the short content is never trusted enough to tombstone it.
    #[test]
    fn test_short_content_skips_containment_bypasses_length_floor() {
        let conn = make_conn();
        seed_agent(&conn, "agent-1", Some("openclaw"), Some("main"));
        seed_session(&conn, "session-1", "agent-1");

        let msg_time = chrono::Utc::now() - chrono::Duration::minutes(30);
        let msg_timestamp = msg_time.to_rfc3339();
        let local_created_at = sqlite_ts(msg_time);
        // A local row that WOULD match if containment ran — proves the length floor
        // short-circuits BEFORE the containment query, not just that no match happens
        // to exist.
        seed_local_assistant_row(&conn, "session-1", "Done.", &local_created_at);

        let result = check_stale_echo(&conn, "session-1", "Done.", &msg_timestamp)
            .expect("lookup should not error");
        assert!(result.is_none(), "content under the 20-char length floor must bypass containment entirely and proceed to insert, even with a matching local row present");
    }

    // ── 6. Repeated-content-late-delivery inserts (the #2 rescue case) ──────────

    /// An agent legitimately re-sends earlier content late: the content matches an
    /// OLD local row, but that row is >5min away from the incoming message's OWN
    /// timestamp — no match in the ±5min anchor window → inserts. This is exactly
    /// §6.4 mitigation 2's "repeated legitimate boilerplate" rescue case (e.g. an
    /// agent says "Done." more than once across a long-lived thread, but here with
    /// long-enough content to pass the length floor too).
    #[test]
    fn test_repeated_content_late_delivery_outside_5min_anchor_inserts() {
        let conn = make_conn();
        seed_agent(&conn, "agent-1", Some("openclaw"), Some("main"));
        seed_session(&conn, "session-1", "agent-1");

        let msg_time = chrono::Utc::now() - chrono::Duration::minutes(20);
        let msg_timestamp = msg_time.to_rfc3339();

        // A local row with IDENTICAL content, but written 45 minutes before the
        // incoming message's own timestamp — well outside the ±5min anchor window.
        let old_local_created_at = sqlite_ts(msg_time - chrono::Duration::minutes(45));
        seed_local_assistant_row(&conn, "session-1", "The task status update text that recurs verbatim.", &old_local_created_at);

        let result = check_stale_echo(&conn, "session-1", "The task status update text that recurs verbatim.", &msg_timestamp)
            .expect("lookup should not error");
        assert!(result.is_none(), "a content match OUTSIDE the ±5min anchor window must not tombstone — this is the legitimate repeated-content late-delivery rescue case");
    }

    // ── 7. Migration idempotency (v86) ───────────────────────────────────────────

    /// v86 migration creates the table + index cleanly on a fresh v85 DB, and
    /// re-running is a safe no-op (`IF NOT EXISTS` guards on both).
    #[test]
    #[serial(pane_test_root, keychain_override)]
    fn test_migration_v86_creates_table_and_index_idempotently() {
        use tempfile::TempDir;
        let temp_dir = TempDir::new().expect("tempdir");
        let test_db_path = temp_dir.path().join("scratch-pane.db");
        unsafe {
            std::env::set_var("PANE_TEST_MODE", "1");
            std::env::set_var("PANE_TEST_DB", test_db_path.to_str().unwrap());
            std::env::set_var("PANE_TEST_ROOT", temp_dir.path());
        }

        // `run_migrations` on the v85→v86 arm below calls `create_migration_backup`,
        // which touches the real macOS Keychain unless a MockKeychain is installed
        // — matching the convention `migrate_v82_tests` already uses in migrations.rs.
        let _kc = KeychainGuard::set();

        let conn = rusqlite::Connection::open_in_memory().expect("in-memory db");
        crate::migrations::run_migrations(&conn).expect("fresh-install migrations should succeed");

        let table_exists_first: bool = conn
            .query_row("SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='chat_oc_tombstones'", [], |r| r.get(0))
            .unwrap();
        assert!(table_exists_first, "chat_oc_tombstones table must exist after first migration run");

        let index_exists_first: bool = conn
            .query_row("SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='index' AND name='idx_chat_local_thread_role_ts'", [], |r| r.get(0))
            .unwrap();
        assert!(index_exists_first, "idx_chat_local_thread_role_ts index must exist after first migration run");

        // Sanity: the table actually works as a PK-enforced dedup store. Uses
        // `reason='own_turn'` (not `stale_echo`/`backstop_echo`) deliberately — this
        // test's own re-run below stamps `schema_version` back to "85", which now
        // also re-chains through migrate_v92 (#3053 item F), and that migration
        // deliberately clears rows with those two text-based reasons. `own_turn` is
        // seq-based and untouched by v92, so it's the right choice for a row this
        // test needs to survive the re-run — the PK-uniqueness behavior itself
        // doesn't depend on which reason string is used.
        conn.execute("INSERT INTO chat_oc_tombstones (thread_id, oc_message_id, reason) VALUES ('t1', 'm1', 'own_turn')", []).unwrap();
        let insert_again = conn.execute("INSERT OR IGNORE INTO chat_oc_tombstones (thread_id, oc_message_id, reason) VALUES ('t1', 'm1', 'own_turn')", []).unwrap();
        assert_eq!(insert_again, 0, "the (thread_id, oc_message_id) primary key must reject a second insert for the same pair");

        // Re-running migrations at the current (already-matching) SCHEMA_VERSION must
        // be a clean no-op.
        let second_run = crate::migrations::run_migrations(&conn);
        assert!(second_run.is_ok(), "second run at current SCHEMA_VERSION must succeed: {:?}", second_run);

        // Directly exercise the v85→v86 migration arm's idempotency by stamping
        // schema_version back to "85" (simulating an install that already has the
        // table/index from a first v85→v86 run) and re-running — the IF NOT EXISTS
        // guards must make this a clean no-op, not a "table already exists" error.
        conn.execute("UPDATE ai_settings SET value = '85' WHERE key = 'schema_version'", [])
            .expect("stamp back to v85 for idempotency re-test");
        let rerun_from_v85 = crate::migrations::run_migrations(&conn);
        assert!(rerun_from_v85.is_ok(), "v85→v86 migration arm must be safely re-runnable when the table/index already exist: {:?}", rerun_from_v85);

        let table_exists_second: bool = conn
            .query_row("SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='chat_oc_tombstones'", [], |r| r.get(0))
            .unwrap();
        assert!(table_exists_second, "chat_oc_tombstones table must still exist after the simulated v85→v86 re-run");

        // The pre-existing tombstone row must have survived both re-runs untouched.
        let preserved_reason: String = conn
            .query_row("SELECT reason FROM chat_oc_tombstones WHERE thread_id = 't1' AND oc_message_id = 'm1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(preserved_reason, "own_turn", "pre-existing tombstone data must survive both migration re-runs");

        unsafe {
            std::env::remove_var("PANE_TEST_MODE");
            std::env::remove_var("PANE_TEST_DB");
        }
    }

    /// Upgrade-fixture smoke test (task requirement): a real DB stamped at v85 (the
    /// `tests/fixtures/upgrade-from-v85.db` fixture — built by copying the v84
    /// fixture and applying the v85 migration SQL directly, per the task's
    /// instructions) migrates cleanly to v86, landing the new table + index and
    /// preserving all pre-existing seed data.
    #[test]
    #[serial(pane_test_root, keychain_override)]
    fn test_upgrade_from_v85_fixture_migrates_cleanly_to_v86() {
        use std::path::PathBuf;
        let fixture_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures/upgrade-from-v85.db");
        if !fixture_path.exists() {
            panic!("upgrade-from-v85.db fixture not found at {:?} — see task instructions for how to build it", fixture_path);
        }

        use tempfile::TempDir;
        let temp_dir = TempDir::new().expect("tempdir");
        let scratch_path = temp_dir.path().join("scratch-upgrade-v85.db");
        std::fs::copy(&fixture_path, &scratch_path).expect("copy fixture to scratch path");

        unsafe {
            std::env::set_var("PANE_TEST_MODE", "1");
            std::env::set_var("PANE_TEST_DB", scratch_path.to_str().unwrap());
            std::env::set_var("PANE_TEST_ROOT", temp_dir.path());
        }

        // See the sibling idempotency test's comment above — the v85→v86 arm calls
        // `create_migration_backup`, which touches the real macOS Keychain unless a
        // MockKeychain is installed.
        let _kc = KeychainGuard::set();

        let conn = rusqlite::Connection::open(&scratch_path).expect("open fixture db");

        // The fixture's `projects` table already carries real cr-sqlite CRR triggers
        // (baked in whenever the fixture was built with the extension loaded) — a
        // trigger's WHEN clause calls `crsql_internal_sync_bit()`, which only exists
        // once the extension is loaded on THIS connection. Every real migration run
        // loads the extension before `run_migrations` (see `database.rs`'s init path
        // and every `smoke-launch.sh` log) — not loading it here was fine as long as
        // no migration arm ever wrote DATA into an already-CRR'd table, which changed
        // with #2874 item 50a's v90→v91 arm (`projects.default_folder_id` backfill).
        // Load it the same way `build_v90_fixture.rs` and the app's real startup do.
        let crsqlite_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/debug/resources/crsqlite.dylib");
        if crsqlite_path.exists() {
            unsafe {
                conn.load_extension_enable().expect("enable extension loading");
                let stripped = crsqlite_path.with_extension("");
                conn.load_extension(&stripped, Some("sqlite3_crsqlite_init"))
                    .expect("load crsqlite extension for real-fixture migration test");
                conn.load_extension_disable().expect("disable extension loading");
            }
        } else {
            eprintln!("[TEST] crsqlite.dylib not built yet (run `cargo build` first) — v91's CRR-trigger-rebuild step will no-op, matching test mode elsewhere");
        }

        let version_before: String = conn
            .query_row("SELECT value FROM ai_settings WHERE key = 'schema_version'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version_before, "85", "fixture must be stamped at v85 before migration");

        let row_count_before: i64 = conn.query_row("SELECT COUNT(*) FROM chat_messages_local", [], |r| r.get(0)).unwrap();
        assert!(row_count_before > 0, "fixture must carry pre-existing chat_messages_local seed data");

        let result = crate::migrations::run_migrations(&conn);
        assert!(result.is_ok(), "v85→{} migration on the real fixture must succeed: {:?}", crate::migrations::SCHEMA_VERSION, result);

        let version_after: String = conn
            .query_row("SELECT value FROM ai_settings WHERE key = 'schema_version'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version_after, crate::migrations::SCHEMA_VERSION, "fixture must land at the current SCHEMA_VERSION after migration");

        let table_exists: bool = conn
            .query_row("SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='chat_oc_tombstones'", [], |r| r.get(0))
            .unwrap();
        assert!(table_exists, "chat_oc_tombstones table must exist after migrating the real v85 fixture");

        let index_exists: bool = conn
            .query_row("SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='index' AND name='idx_chat_local_thread_role_ts'", [], |r| r.get(0))
            .unwrap();
        assert!(index_exists, "idx_chat_local_thread_role_ts index must exist after migrating the real v85 fixture");

        let row_count_after: i64 = conn.query_row("SELECT COUNT(*) FROM chat_messages_local", [], |r| r.get(0)).unwrap();
        assert_eq!(row_count_after, row_count_before, "pre-existing chat_messages_local rows must survive the v85→v86 migration untouched");

        unsafe {
            std::env::remove_var("PANE_TEST_MODE");
            std::env::remove_var("PANE_TEST_DB");
        }
    }

    // ── 8. Pre-filter unconditional during suppress_ingest=true ─────────────────

    /// Already-classified messages are dropped by the pre-filter EVEN WHEN
    /// `suppress_ingest=true` is active (§3d: the pre-filter/stale-check run
    /// unconditionally, before the suppress_ingest early-return). Proves this via
    /// the reported counters AND the deferred skip_reason message count, which must
    /// reflect only the SURVIVING (non-pre-filtered) messages.
    #[tokio::test]
    #[serial(keychain_override)]
    async fn test_prefilter_unconditional_during_suppress_ingest() {
        let mut server = mockito::Server::new_async().await;

        let conn = make_conn();
        seed_agent(&conn, "agent-oc", Some("openclaw"), Some("main"));
        seed_session(&conn, "session-suppress-prefilter", "agent-oc");
        crate::database::set_ai_setting(&conn, "endpoint", &server.url()).unwrap();
        conn.execute(
            "UPDATE chat_sessions SET oc_baseline_done_at = datetime('now') WHERE id = 'session-suppress-prefilter'",
            [],
        )
        .unwrap();
        // Pre-tombstone one id directly, simulating a prior tick's backstop hit.
        write_tombstone(&conn, "session-suppress-prefilter", "oc-already-tombstoned", "backstop_echo", "old echo text", None, None).unwrap();

        let body = serde_json::json!({
            "messages": [
                { "role": "assistant", "content": "already tombstoned content", "timestamp": 1_756_000_000_000i64, "__openclaw": { "id": "oc-already-tombstoned" } },
                { "role": "assistant", "content": "a genuinely new message still deferred by suppression", "timestamp": 1_756_000_010_000i64, "__openclaw": { "id": "oc-fresh-1" } }
            ]
        });
        let _m = server
            .mock("GET", "/v1/sessions/agent%3Amain%3Aopenai-user%3Apane-session-suppress-prefilter/messages?limit=20")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(body.to_string())
            .create_async()
            .await;

        let _kc = KeychainGuard::set();
        crate::ai::credential_migration::write_gateway_token(&conn, "test-token-suppress-prefilter").unwrap();

        let db = DbState::new(conn);

        // suppress_ingest=true: the dispatch grace window is active.
        let result = pull_oc_transcript_for_session("session-suppress-prefilter".to_string(), &db, true).await.unwrap();

        assert_eq!(result.fetched_count, 2, "both raw messages must still be counted as fetched even while suppressed: {:?}", result);
        assert_eq!(
            result.already_classified_count, 1,
            "the pre-tombstoned message must be dropped by the pre-filter EVEN WHILE suppress_ingest=true is active: {:?}", result
        );
        assert!(result.ingest.is_none(), "ingest must still be withheld entirely while suppress_ingest is true");
        assert!(
            result.skip_reason.as_ref().unwrap().contains("1 message(s) deferred"),
            "skip_reason must report exactly 1 SURVIVING (post-pre-filter) message deferred, not 2: {:?}", result.skip_reason
        );

        let row_count: i64 = {
            let conn2 = db.0.lock().unwrap();
            conn2.query_row("SELECT COUNT(*) FROM chat_messages_local WHERE thread_id = 'session-suppress-prefilter'", [], |r| r.get(0)).unwrap()
        };
        assert_eq!(row_count, 0, "nothing must be inserted while suppress_ingest is true, regardless of pre-filter activity");
    }
}


/// #3053 item F — "the duplicate filter must never drop a message by text." Tests
/// drive the real `pull_oc_transcript_for_session`/`run_migrations` entry points
/// (Matt's standing rule, §the tombstone-scope audit's own "never INSERT-built"
/// requirement — #3024 violated it while passing). Covers the four shapes the task
/// requires: (1) two distinct OC messages where the second quotes the first — both
/// land; (2) the exact 2026-10-20 seq-154 regression shape (an agent reply quotes
/// text that a real, later-arriving cron result also carries — the cron result must
/// not be dropped); (3) the guard against over-correcting — seq-based own-turn
/// dedup must still work; (4) a message tombstoned by the OLD text-based mechanism,
/// before this fix shipped, must reappear once migrate_v92 clears it and the
/// session is re-polled.
#[cfg(test)]
mod dedup_by_id_and_seq_tests_3053f {
    use super::*;
    use crate::keychain::test_support::KeychainGuard;
    use serial_test::serial;

    fn make_conn() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().expect("in-memory db");
        crate::migrations::run_migrations(&conn).expect("fresh-install migrations should succeed");
        conn
    }

    fn seed_agent(conn: &rusqlite::Connection, id: &str, runtime_type: Option<&str>, runtime_ref: Option<&str>) {
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO ai_agents (id, name, slug, is_default, is_active, workspace_path, runtime_type, runtime_ref, model, created_at, updated_at)
             VALUES (?1, ?1, ?1, 0, 1, ?1, ?2, ?3, 'test-model', ?4, ?4)",
            rusqlite::params![id, runtime_type, runtime_ref, now],
        )
        .expect("seed agent");
    }

    fn seed_session(conn: &rusqlite::Connection, id: &str, agent_id: &str) {
        conn.execute(
            "INSERT INTO chat_sessions (id, project_id, agent_id) VALUES (?1, 'workspace', ?2)",
            rusqlite::params![id, agent_id],
        )
        .expect("seed session");
    }

    fn mark_baselined(conn: &rusqlite::Connection, session_id: &str) {
        conn.execute(
            "UPDATE chat_sessions SET oc_baseline_done_at = datetime('now') WHERE id = ?1",
            rusqlite::params![session_id],
        )
        .unwrap();
    }

    fn setup_gateway(conn: &rusqlite::Connection, server_url: &str, token: &str) {
        crate::database::set_ai_setting(conn, "endpoint", server_url).unwrap();
        crate::ai::credential_migration::write_gateway_token(conn, token).unwrap();
    }

    fn sqlite_ts(dt: chrono::DateTime<chrono::Utc>) -> String {
        dt.format("%Y-%m-%d %H:%M:%S").to_string()
    }

    fn seed_local_assistant_row(conn: &rusqlite::Connection, thread_id: &str, content: &str, created_at: &str) {
        conn.execute(
            "INSERT INTO chat_messages_local (id, project_id, thread_id, role, content, created_at) \
             VALUES (?1, '', ?2, 'assistant', ?3, ?4)",
            rusqlite::params![format!("local-{}", uuid::Uuid::new_v4()), thread_id, content, created_at],
        )
        .expect("seed local assistant row");
    }

    // ── Test 1: two distinct OC messages, the second quotes the first — both land ──

    /// Message 1 lands via a first poll tick. Message 2, a DISTINCT id arriving on a
    /// later tick, is a full quote-reply of message 1 (its content equals message 1's
    /// text verbatim — containment includes exact equality, same shape as a chat UI's
    /// "quote" action embedding the original text as the new message's body with no
    /// added commentary, per the scope audit's own framing: "a later message that
    /// quotes an earlier one... produces exactly the containment relationship this
    /// code treats as proof of 'this is an echo, drop it'"). Both must land; neither
    /// gets tombstoned. This must fail at base (message 2 would be silently dropped
    /// by the backstop, `content_match_skipped_count == 1`, zero tombstone rows
    /// notwithstanding since the base commit's backstop tombstones on every hit).
    #[tokio::test]
    #[serial(keychain_override)]
    async fn test_two_distinct_messages_second_quotes_first_both_land() {
        let _kc = KeychainGuard::set();
        let mut server = mockito::Server::new_async().await;

        let conn = make_conn();
        seed_agent(&conn, "agent-oc", Some("openclaw"), Some("main"));
        seed_session(&conn, "session-quote", "agent-oc");
        setup_gateway(&conn, &server.url(), "test-token-quote");
        mark_baselined(&conn, "session-quote");

        let db = DbState::new(conn);

        // Tick 1: message 1 lands.
        let tick1_body = serde_json::json!({
            "messages": [
                { "role": "assistant", "content": "Backup completed successfully at 3:48pm.", "timestamp": 1_761_000_000_000i64, "__openclaw": { "id": "oc-quote-original" } }
            ]
        });
        let m_tick1 = server
            .mock("GET", "/v1/sessions/agent%3Amain%3Aopenai-user%3Apane-session-quote/messages?limit=20")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(tick1_body.to_string())
            .expect(1)
            .create_async()
            .await;

        let tick1 = pull_oc_transcript_for_session("session-quote".to_string(), &db, false).await.unwrap();
        m_tick1.assert_async().await;
        let tick1_ingest = tick1.ingest.expect("ingest must run at tick 1");
        assert_eq!(tick1_ingest.inserted_count, 1, "message 1 must land at tick 1: {:?}", tick1_ingest);

        // Tick 2: message 2 (distinct id) arrives, quoting message 1's exact text.
        let tick2_body = serde_json::json!({
            "messages": [
                { "role": "assistant", "content": "Backup completed successfully at 3:48pm.", "timestamp": 1_761_000_000_000i64, "__openclaw": { "id": "oc-quote-original" } },
                { "role": "assistant", "content": "Backup completed successfully at 3:48pm.", "timestamp": 1_761_000_010_000i64, "__openclaw": { "id": "oc-quote-reply" } }
            ]
        });
        let _m_tick2 = server
            .mock("GET", "/v1/sessions/agent%3Amain%3Aopenai-user%3Apane-session-quote/messages?limit=20")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(tick2_body.to_string())
            .create_async()
            .await;

        let tick2 = pull_oc_transcript_for_session("session-quote".to_string(), &db, false).await.unwrap();
        let tick2_ingest = tick2.ingest.expect("ingest must run at tick 2");
        assert_eq!(
            tick2_ingest.inserted_count, 1,
            "message 2 (the quoting reply) must land — it is a genuinely new, never-before-seen id, and matching message 1's text must not drop it: {:?}", tick2_ingest
        );

        let ids: Vec<String> = {
            let conn2 = db.0.lock().unwrap();
            let mut stmt = conn2.prepare("SELECT oc_message_id FROM chat_messages_local WHERE thread_id = 'session-quote' ORDER BY oc_message_id").unwrap();
            stmt.query_map([], |r| r.get(0)).unwrap().map(|r| r.unwrap()).collect()
        };
        assert_eq!(ids, vec!["oc-quote-original".to_string(), "oc-quote-reply".to_string()], "both distinct messages must exist as real rows");

        let tombstone_count: i64 = {
            let conn2 = db.0.lock().unwrap();
            conn2.query_row("SELECT COUNT(*) FROM chat_oc_tombstones WHERE thread_id = 'session-quote'", [], |r| r.get(0)).unwrap()
        };
        assert_eq!(tombstone_count, 0, "neither message may be tombstoned");
    }

    // ── Test 2: the exact 2026-10-20 seq-154 regression shape ──────────────────────

    /// Reproduces the literal incident: an agent reply ALREADY saved locally quotes
    /// the cron result's exact text (ordinary behavior — the agent restating/
    /// acknowledging prior content). The REAL cron result then arrives via the OC
    /// pull, carrying that same text, as a genuinely new, distinct message. Pre-fix,
    /// `is_recent_local_assistant_echo`/`check_stale_echo` would see the LOCAL row
    /// (the quoting reply) contains the INCOMING cron text and wrongly tombstone the
    /// cron result — exactly what happened to the real seq-154 cron delivery. This
    /// must fail at base (the cron message would never reach `chat_messages_local`).
    #[tokio::test]
    #[serial(keychain_override)]
    async fn test_3053_f_cron_result_quoted_by_earlier_reply_still_lands() {
        let _kc = KeychainGuard::set();
        let mut server = mockito::Server::new_async().await;

        let conn = make_conn();
        seed_agent(&conn, "agent-oc", Some("openclaw"), Some("main"));
        seed_session(&conn, "session-348", "agent-oc");
        setup_gateway(&conn, &server.url(), "test-token-348");
        mark_baselined(&conn, "session-348");

        // The agent's own reply, already saved locally, quotes the cron text that
        // hasn't landed yet — e.g. "As I mentioned: Nightly backup finished at 3:48pm.
        // Let me know if you need anything else."
        let now = sqlite_ts(chrono::Utc::now());
        seed_local_assistant_row(
            &conn,
            "session-348",
            "As I mentioned: Nightly backup finished at 3:48pm. Let me know if you need anything else.",
            &now,
        );

        // The real cron result arrives, carrying the exact text the reply above
        // already quotes.
        let body = serde_json::json!({
            "messages": [
                { "role": "assistant", "content": "Nightly backup finished at 3:48pm.", "timestamp": 1_761_000_020_000i64, "__openclaw": { "id": "oc-seq-154-cron", "seq": 154 } }
            ]
        });
        let _m = server
            .mock("GET", "/v1/sessions/agent%3Amain%3Aopenai-user%3Apane-session-348/messages?limit=20")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(body.to_string())
            .create_async()
            .await;

        let db = DbState::new(conn);
        let result = pull_oc_transcript_for_session("session-348".to_string(), &db, false).await.unwrap();

        assert_eq!(
            result.stale_echo_tombstoned_count + result.content_match_skipped_count, 1,
            "the text-containment match against the quoting reply must still be observed/logged (diagnostic only): {:?}", result
        );
        let ingest = result.ingest.expect("ingest must run");
        assert_eq!(
            ingest.inserted_count, 1,
            "#3053 item F regression test: the cron result MUST land — this is the exact seq-154 incident, and it must never repeat: {:?}", ingest
        );

        let landed: bool = {
            let conn2 = db.0.lock().unwrap();
            conn2.query_row(
                "SELECT EXISTS(SELECT 1 FROM chat_messages_local WHERE thread_id = 'session-348' AND oc_message_id = 'oc-seq-154-cron')",
                [],
                |r| r.get(0),
            ).unwrap()
        };
        assert!(landed, "the cron result must be a real row in chat_messages_local, never silently dropped");

        let tombstoned: bool = {
            let conn2 = db.0.lock().unwrap();
            conn2.query_row(
                "SELECT EXISTS(SELECT 1 FROM chat_oc_tombstones WHERE thread_id = 'session-348' AND oc_message_id = 'oc-seq-154-cron')",
                [],
                |r| r.get(0),
            ).unwrap()
        };
        assert!(!tombstoned, "the cron result must never be tombstoned by a text match");
    }

    // ── Test 3: over-correction guard — seq-based own-turn dedup must still work ──

    /// Removing text matching's power to drop/tombstone must NOT weaken the
    /// seq-based own-turn mechanism — that would trade silent deletion for duplicate
    /// spam, the opposite failure mode. After a real dispatch sets a baseline seq, an
    /// echo of that same dispatch with a HIGHER seq must still be tombstoned (via
    /// `tombstone_own_turn`, `reason="own_turn"`) and never re-inserted on the next
    /// poll — this mechanism is seq-only and untouched by this fix. (Also covered,
    /// more extensively, by `own_turn_tombstone_tests_2738`; this test exists so
    /// PR F's own test suite demonstrates the guard directly, not just by reference.)
    #[tokio::test]
    #[serial(keychain_override)]
    async fn test_own_turn_echo_above_baseline_seq_still_tombstoned() {
        let _kc = KeychainGuard::set();

        let mut server = mockito::Server::new_async().await;
        let conn = make_conn();
        seed_agent(&conn, "agent-oc", Some("openclaw"), Some("main"));
        seed_session(&conn, "session-seq-guard", "agent-oc");
        setup_gateway(&conn, &server.url(), "test-token-seq-guard");

        // Baseline fetch (dispatch start): max seq = 50.
        let baseline_body = serde_json::json!({
            "messages": [{ "role": "assistant", "content": "earlier turn", "timestamp": 1_761_000_000_000i64, "__openclaw": { "id": "oc-seq-guard-earlier", "seq": 50 } }]
        });
        let _m_baseline = server
            .mock("GET", "/v1/sessions/agent%3Amain%3Aopenai-user%3Apane-session-seq-guard/messages?limit=1")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(baseline_body.to_string())
            .create_async()
            .await;

        let db = DbState::new(conn);
        let baseline_seq = fetch_oc_latest_seq("session-seq-guard", &db).await;
        assert_eq!(baseline_seq, Some(50));

        // Dispatch completes; OC's transcript now also has the dispatch's own echo at
        // seq 51 (content deliberately short/generic — proves this is seq-based, not
        // a coincidental text-length-floor miss).
        let post_dispatch_body = serde_json::json!({
            "messages": [
                { "role": "assistant", "content": "earlier turn", "timestamp": 1_761_000_000_000i64, "__openclaw": { "id": "oc-seq-guard-earlier", "seq": 50 } },
                { "role": "assistant", "content": "OK", "timestamp": 1_761_000_010_000i64, "__openclaw": { "id": "oc-seq-guard-echo", "seq": 51 } }
            ]
        });
        let _m_post = server
            .mock("GET", "/v1/sessions/agent%3Amain%3Aopenai-user%3Apane-session-seq-guard/messages?limit=20")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(post_dispatch_body.to_string())
            .create_async()
            .await;

        let tombstoned = tombstone_own_turn("session-seq-guard", &db, baseline_seq.unwrap()).await;
        assert_eq!(tombstoned, 1, "the seq-51 echo must still be tombstoned by seq — #3053 item F only removed TEXT as a decision input, seq is untouched");

        let reason: String = {
            let conn2 = db.0.lock().unwrap();
            conn2.query_row(
                "SELECT reason FROM chat_oc_tombstones WHERE thread_id = 'session-seq-guard' AND oc_message_id = 'oc-seq-guard-echo'",
                [],
                |r| r.get(0),
            ).unwrap()
        };
        assert_eq!(reason, "own_turn", "the tombstone reason must be own_turn (seq-based), never a text-based reason");

        // Next poll tick must not re-insert the tombstoned echo.
        mark_baselined(&db.0.lock().unwrap(), "session-seq-guard");
        let pull = pull_oc_transcript_for_session("session-seq-guard".to_string(), &db, false).await.unwrap();
        let ingest = pull.ingest.expect("ingest must run");
        assert_eq!(ingest.inserted_count, 1, "only \"earlier turn\" inserts — the seq-based tombstone must still exclude the echo: {:?}", ingest);

        let echo_row_count: i64 = {
            let conn2 = db.0.lock().unwrap();
            conn2.query_row("SELECT COUNT(*) FROM chat_messages_local WHERE thread_id = 'session-seq-guard' AND oc_message_id = 'oc-seq-guard-echo'", [], |r| r.get(0)).unwrap()
        };
        assert_eq!(echo_row_count, 0, "the own-turn echo must never appear as a duplicate row — seq-based dedup must not have weakened");
    }

    // ── Test 4: a previously text-tombstoned message reappears after the fix ───────

    /// Simulates an install that ALREADY has `stale_echo`/`backstop_echo` tombstone
    /// rows from before this fix shipped (the OLD, pre-#3053-item-F code wrote these
    /// via a text-containment hit, permanently hiding the message via the Rule 1
    /// pre-filter). Upgrading past this fix runs migrate_v92
    /// (`run_migrations` dispatches to the new v91→v92 arm), which clears exactly
    /// those two reasons. A subsequent poll, with the message still inside OC's
    /// fetch window, must now land — proving the migration doesn't just stop future
    /// damage, it un-hides what the bug already hid. An `own_turn` tombstone (seq-
    /// based, correct) is seeded alongside and must survive untouched.
    #[tokio::test]
    #[serial(keychain_override)]
    async fn test_previously_text_tombstoned_message_reappears_after_migration_and_repoll() {
        let conn = make_conn(); // lands at the current SCHEMA_VERSION (92) fresh
        seed_agent(&conn, "agent-oc", Some("openclaw"), Some("main"));
        seed_session(&conn, "session-reappear", "agent-oc");

        // Simulate "this install already has v91-era tombstone rows written by the
        // OLD text-based code, before upgrading past this fix": stamp schema_version
        // back to "91" and seed the rows directly (legitimate pre-existing-state setup
        // for a migration test — not a dedup-precondition INSERT; this models what an
        // existing user's real DB already contains, matching this file's own
        // established convention for migration tests, e.g. "stamp back to v85 for
        // idempotency re-test").
        conn.execute("UPDATE ai_settings SET value = '91' WHERE key = 'schema_version'", [])
            .expect("stamp back to v91 to simulate a pre-fix install");
        write_tombstone(&conn, "session-reappear", "oc-wrongly-hidden-stale", "stale_echo", "wrongly hidden by text match", None, None).unwrap();
        write_tombstone(&conn, "session-reappear", "oc-wrongly-hidden-backstop", "backstop_echo", "also wrongly hidden", None, None).unwrap();
        write_tombstone(&conn, "session-reappear", "oc-legit-own-turn", "own_turn", "a real echo of the agent's own turn", None, None).unwrap();

        // Before the migration runs: all three are classified (the pre-filter honors
        // any existing tombstone regardless of reason) — confirms the starting state
        // is as claimed, not assumed.
        assert!(is_already_classified(&conn, "session-reappear", "oc-wrongly-hidden-stale").unwrap());
        assert!(is_already_classified(&conn, "session-reappear", "oc-wrongly-hidden-backstop").unwrap());
        assert!(is_already_classified(&conn, "session-reappear", "oc-legit-own-turn").unwrap());

        let _kc = KeychainGuard::set();
        let migration_result = crate::migrations::run_migrations(&conn);
        assert!(migration_result.is_ok(), "v91→v92 migration must succeed: {:?}", migration_result);

        let schema_version_after: String = conn
            .query_row("SELECT value FROM ai_settings WHERE key = 'schema_version'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(schema_version_after, crate::migrations::SCHEMA_VERSION, "schema_version must land at the current version after the v91→v92 migration");

        // Both text-based reasons must be gone; the seq-based own_turn reason must survive.
        let remaining_reasons: Vec<String> = {
            let mut stmt = conn.prepare("SELECT reason FROM chat_oc_tombstones WHERE thread_id = 'session-reappear' ORDER BY reason").unwrap();
            stmt.query_map([], |r| r.get(0)).unwrap().map(|r| r.unwrap()).collect()
        };
        assert_eq!(remaining_reasons, vec!["own_turn".to_string()], "only the seq-based own_turn tombstone may survive the v91→v92 migration: {:?}", remaining_reasons);

        // Idempotency: running migrations again (already at current SCHEMA_VERSION)
        // must be a safe no-op — it takes the "schema matches" verify-only arm, never
        // re-invokes migrate_v92, and the surviving own_turn row is untouched either way.
        let second_run = crate::migrations::run_migrations(&conn);
        assert!(second_run.is_ok(), "second run at current SCHEMA_VERSION must succeed: {:?}", second_run);
        let remaining_reasons_after_second_run: Vec<String> = {
            let mut stmt = conn.prepare("SELECT reason FROM chat_oc_tombstones WHERE thread_id = 'session-reappear' ORDER BY reason").unwrap();
            stmt.query_map([], |r| r.get(0)).unwrap().map(|r| r.unwrap()).collect()
        };
        assert_eq!(remaining_reasons_after_second_run, vec!["own_turn".to_string()], "a second run must not change anything further");

        // Now prove the user-visible half: with the tombstone cleared, a re-poll that
        // still finds the message inside OC's fetch window (no cursor, §30.11.3) lands
        // it for real — the bug's damage is actually undone, not just stopped from
        // recurring.
        mark_baselined(&conn, "session-reappear");
        let mut server = mockito::Server::new_async().await;
        setup_gateway(&conn, &server.url(), "test-token-reappear");
        let body = serde_json::json!({
            "messages": [
                { "role": "assistant", "content": "the message that was wrongly hidden", "timestamp": 1_761_000_000_000i64, "__openclaw": { "id": "oc-wrongly-hidden-stale" } }
            ]
        });
        let _m = server
            .mock("GET", "/v1/sessions/agent%3Amain%3Aopenai-user%3Apane-session-reappear/messages?limit=20")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(body.to_string())
            .create_async()
            .await;

        let db = DbState::new(conn);
        let pull = pull_oc_transcript_for_session("session-reappear".to_string(), &db, false).await.unwrap();
        assert_eq!(pull.already_classified_count, 0, "the cleared id must no longer be pre-filtered: {:?}", pull);
        let ingest = pull.ingest.expect("ingest must run");
        assert_eq!(ingest.inserted_count, 1, "the previously-hidden message must reappear and land for real: {:?}", ingest);

        let landed: bool = {
            let conn2 = db.0.lock().unwrap();
            conn2.query_row(
                "SELECT EXISTS(SELECT 1 FROM chat_messages_local WHERE thread_id = 'session-reappear' AND oc_message_id = 'oc-wrongly-hidden-stale')",
                [],
                |r| r.get(0),
            ).unwrap()
        };
        assert!(landed, "the message must now be a real row — the migration's clearing + a re-poll together undo the original silent deletion");
    }
}


/// #2738 — identity-based own-turn tombstoning. Tests exercise `fetch_oc_latest_seq`
/// and `tombstone_own_turn` directly (same convention as `tombstoning_tests` above,
/// which tests `is_already_classified`/`write_tombstone` directly rather than
/// spinning up a full `ai_chat_stream` spawn — the spawned-task wiring itself is a
/// thin, mechanical call-through with no branching logic of its own to test), plus
/// one full round-trip through `pull_oc_transcript_for_session` to prove a
/// `reason="own_turn"` tombstone actually suppresses re-ingestion end to end.
#[cfg(test)]
mod own_turn_tombstone_tests_2738 {
    use super::*;
    use crate::keychain::test_support::KeychainGuard;
    use serial_test::serial;

    fn make_conn() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().expect("in-memory db");
        crate::migrations::run_migrations(&conn).expect("fresh-install migrations should succeed");
        conn
    }

    fn seed_agent(conn: &rusqlite::Connection, id: &str, runtime_type: Option<&str>, runtime_ref: Option<&str>) {
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO ai_agents (id, name, slug, is_default, is_active, workspace_path, runtime_type, runtime_ref, model, created_at, updated_at)
             VALUES (?1, ?1, ?1, 0, 1, ?1, ?2, ?3, 'test-model', ?4, ?4)",
            rusqlite::params![id, runtime_type, runtime_ref, now],
        )
        .expect("seed agent");
    }

    fn seed_session(conn: &rusqlite::Connection, id: &str, agent_id: &str) {
        conn.execute(
            "INSERT INTO chat_sessions (id, project_id, agent_id) VALUES (?1, 'workspace', ?2)",
            rusqlite::params![id, agent_id],
        )
        .expect("seed session");
    }

    /// Baselines the session the same way `pull_oc_transcript_for_session`'s
    /// first-ever-pull gate requires (`oc_baseline_done_at IS NULL` short-circuits
    /// every ingest into a no-op baseline pull) — every test below that calls
    /// `pull_oc_transcript_for_session` for real assertions needs this pre-stamped,
    /// exactly like `test_oc_pull_502_graceful_skip_no_panic` and friends do.
    fn mark_baselined(conn: &rusqlite::Connection, session_id: &str) {
        conn.execute(
            "UPDATE chat_sessions SET oc_baseline_done_at = datetime('now') WHERE id = ?1",
            rusqlite::params![session_id],
        )
        .unwrap();
    }

    fn setup_gateway(conn: &rusqlite::Connection, server_url: &str, token: &str) {
        crate::database::set_ai_setting(conn, "endpoint", server_url).unwrap();
        crate::ai::credential_migration::write_gateway_token(conn, token).unwrap();
    }

    // ── 1. Short live reply ("Gold", 4 chars) is tombstoned and never re-ingests ──

    /// End-to-end reproduction of the #2738 bug report: a 4-char assistant reply
    /// ("Gold") is well below `CONTENT_LENGTH_FLOOR` (20 chars), so the pre-existing
    /// content-based backstop CANNOT catch it — only identity-based (`seq`) matching
    /// can. `fetch_oc_latest_seq` records a baseline BEFORE "Gold" exists in OC's
    /// transcript; `tombstone_own_turn` then sees "Gold" with `seq > baseline` and
    /// tombstones it with `reason="own_turn"`. The next poll tick
    /// (`pull_oc_transcript_for_session`) must then find it already classified and
    /// must NOT insert it as a duplicate row.
    #[tokio::test]
    #[serial(keychain_override)]
    async fn test_short_reply_gold_is_tombstoned_and_never_reingested() {
        let _kc = KeychainGuard::set();

        let mut server = mockito::Server::new_async().await;
        let conn = make_conn();
        seed_agent(&conn, "agent-oc", Some("openclaw"), Some("main"));
        seed_session(&conn, "session-gold", "agent-oc");
        setup_gateway(&conn, &server.url(), "test-token-gold");

        // Step 1: dispatch-start baseline fetch — OC transcript has only pre-existing
        // history, max seq = 10. "Gold" (seq 11) doesn't exist yet at this point.
        let baseline_body = serde_json::json!({
            "messages": [
                { "role": "assistant", "content": "earlier turn", "timestamp": 1_756_000_000_000i64, "__openclaw": { "id": "oc-earlier", "seq": 10 } }
            ]
        });
        let _m_baseline = server
            .mock("GET", "/v1/sessions/agent%3Amain%3Aopenai-user%3Apane-session-gold/messages?limit=1")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(baseline_body.to_string())
            .create_async()
            .await;

        let db = DbState::new(conn);
        let baseline_seq = fetch_oc_latest_seq("session-gold", &db).await;
        assert_eq!(baseline_seq, Some(10), "baseline must capture the pre-dispatch max seq");

        // Step 2: dispatch completes. OC's transcript now ALSO contains "Gold" at
        // seq 11 — the short live reply the bug report describes.
        let post_dispatch_body = serde_json::json!({
            "messages": [
                { "role": "assistant", "content": "earlier turn", "timestamp": 1_756_000_000_000i64, "__openclaw": { "id": "oc-earlier", "seq": 10 } },
                { "role": "assistant", "content": "Gold", "timestamp": 1_756_000_010_000i64, "__openclaw": { "id": "oc-gold", "seq": 11 } }
            ]
        });
        let _m_post = server
            .mock("GET", "/v1/sessions/agent%3Amain%3Aopenai-user%3Apane-session-gold/messages?limit=20")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(post_dispatch_body.to_string())
            .create_async()
            .await;

        let tombstoned = tombstone_own_turn("session-gold", &db, baseline_seq.unwrap()).await;
        assert_eq!(tombstoned, 1, "exactly one own-turn message (\"Gold\", seq 11 > baseline 10) must be tombstoned");

        {
            let conn2 = db.0.lock().unwrap();
            let reason: String = conn2
                .query_row(
                    "SELECT reason FROM chat_oc_tombstones WHERE thread_id = 'session-gold' AND oc_message_id = 'oc-gold'",
                    [],
                    |r| r.get(0),
                )
                .expect("tombstone row for oc-gold must exist");
            assert_eq!(reason, "own_turn", "the tombstone reason must be own_turn, not backstop_echo/stale_echo");
        }

        // Step 3: the frontend's next 60s poll tick re-fetches the SAME page (no
        // cursor) — "Gold" must be caught by the permanence pre-filter and NOT
        // inserted as a duplicate row, even though its 4-char content could never
        // have been caught by CONTENT_LENGTH_FLOOR.
        mark_baselined(&db.0.lock().unwrap(), "session-gold");
        let pull = pull_oc_transcript_for_session("session-gold".to_string(), &db, false).await.unwrap();

        assert_eq!(pull.fetched_count, 2, "both raw messages must be counted as fetched: {:?}", pull);
        assert_eq!(
            pull.already_classified_count, 1,
            "the own_turn-tombstoned \"Gold\" message must be dropped by the pre-filter: {:?}", pull
        );
        let ingest = pull.ingest.expect("ingest must run (suppress_ingest was false)");
        // "earlier turn" (oc-earlier, seq 10) is genuinely new to `chat_messages_local`
        // — this test never pulled before this call — so it correctly inserts. The
        // ONLY thing this assertion needs to prove is that "Gold" is NOT among what
        // inserted, which the exact inserted_count of 1 (not 2) confirms.
        assert_eq!(ingest.inserted_count, 1, "only \"earlier turn\" should insert — \"Gold\" must be excluded by the own_turn tombstone: {:?}", ingest);

        let row_count: i64 = {
            let conn2 = db.0.lock().unwrap();
            conn2.query_row("SELECT COUNT(*) FROM chat_messages_local WHERE thread_id = 'session-gold' AND content = 'Gold'", [], |r| r.get(0)).unwrap()
        };
        assert_eq!(row_count, 0, "\"Gold\" must never appear as a duplicate row in chat_messages_local");
    }

    // ── 2. Genuine cron delivery after done:true still inserts normally ─────────

    /// A dispatch completes, its own-turn tombstone pass runs and correctly leaves
    /// the transcript with nothing new above baseline. Some time later, a genuinely
    /// new cron/proactive message lands on the SAME session with a HIGHER seq than
    /// anything the tombstone pass ever saw. It must NOT be tombstoned and must
    /// insert normally on the next poll — proves the fix never blocks real deliveries.
    #[tokio::test]
    #[serial(keychain_override)]
    async fn test_genuine_cron_delivery_after_done_still_inserts() {
        let _kc = KeychainGuard::set();

        let mut server = mockito::Server::new_async().await;
        let conn = make_conn();
        seed_agent(&conn, "agent-oc", Some("openclaw"), Some("main"));
        seed_session(&conn, "session-cron", "agent-oc");
        setup_gateway(&conn, &server.url(), "test-token-cron");
        mark_baselined(&conn, "session-cron");

        let db = DbState::new(conn);

        // Dispatch's own tombstone pass: baseline was seq 5, dispatch produced
        // nothing new above it (e.g. a tool-only turn) — zero own-turn tombstones.
        let dispatch_done_body = serde_json::json!({
            "messages": [
                { "role": "assistant", "content": "pre-existing turn", "timestamp": 1_756_000_000_000i64, "__openclaw": { "id": "oc-pre", "seq": 5 } }
            ]
        });
        let _m_done = server
            .mock("GET", "/v1/sessions/agent%3Amain%3Aopenai-user%3Apane-session-cron/messages?limit=20")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(dispatch_done_body.to_string())
            .expect(1)
            .create_async()
            .await;

        let tombstoned = tombstone_own_turn("session-cron", &db, 5).await;
        assert_eq!(tombstoned, 0, "no message exceeds the baseline yet — zero own-turn tombstones expected");
        drop(_m_done);

        // A NEW cron delivery arrives later with seq 6 — higher than the tombstone
        // pass's baseline (5) but was never seen or evaluated by that pass (it
        // hadn't landed yet). It must reach ingestion normally.
        let poll_body = serde_json::json!({
            "messages": [
                { "role": "assistant", "content": "pre-existing turn", "timestamp": 1_756_000_000_000i64, "__openclaw": { "id": "oc-pre", "seq": 5 } },
                { "role": "assistant", "content": "Cron delivery: nightly backup complete", "timestamp": 1_756_000_020_000i64, "__openclaw": { "id": "oc-cron-real", "seq": 6 } }
            ]
        });
        let _m_poll = server
            .mock("GET", "/v1/sessions/agent%3Amain%3Aopenai-user%3Apane-session-cron/messages?limit=20")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(poll_body.to_string())
            .create_async()
            .await;

        let pull = pull_oc_transcript_for_session("session-cron".to_string(), &db, false).await.unwrap();

        assert_eq!(pull.already_classified_count, 0, "the cron message must NOT be tombstoned/already-classified: {:?}", pull);
        let ingest = pull.ingest.expect("ingest must run");
        // Both "pre-existing turn" (oc-pre, never tombstoned — seq 5 does not exceed
        // its own baseline of 5) AND the new cron message insert; the assertion this
        // test cares about is that the cron message (oc-cron-real) is among them,
        // confirmed below by its row lookup.
        assert_eq!(ingest.inserted_count, 2, "both the pre-existing message and the new cron message must insert — neither was tombstoned: {:?}", ingest);

        let content: String = {
            let conn2 = db.0.lock().unwrap();
            conn2
                .query_row(
                    "SELECT content FROM chat_messages_local WHERE thread_id = 'session-cron' AND oc_message_id = 'oc-cron-real'",
                    [],
                    |r| r.get(0),
                )
                .expect("the cron row must exist")
        };
        assert_eq!(content, "Cron delivery: nightly backup complete");
    }

    // ── 3. Session-id rotation mid-turn re-baselines from a fresh fetch ─────────

    /// Reed's audit, Task 1 exception #3: OC can rotate its internal `sessionId`
    /// under a stable `sessionKey` mid-turn. Because both `fetch_oc_latest_seq` and
    /// `tombstone_own_turn` re-derive the OC session KEY fresh from `runtime_ref`/
    /// `session_id` on every call (never caching OC's own rotatable `sessionId`,
    /// which lives only in the gateway's own `session_mappings` table), a rotation
    /// occurring between the baseline call and the tombstone-pass call is
    /// TRANSPARENT to Pane: each call simply hits the gateway's CURRENT resolution
    /// for that stable key, never a stale snapshot. This test simulates the gateway
    /// returning a DIFFERENT transcript (as if the underlying sessionId rotated)
    /// between the baseline call and the tombstone-pass call, and proves the
    /// tombstone pass correctly evaluates seq against the FRESH (post-rotation)
    /// transcript rather than anything cached from the baseline call.
    #[tokio::test]
    #[serial(keychain_override)]
    async fn test_rotation_mid_turn_rebaselines_from_fresh_fetch() {
        let _kc = KeychainGuard::set();

        let mut server = mockito::Server::new_async().await;
        let conn = make_conn();
        seed_agent(&conn, "agent-oc", Some("openclaw"), Some("main"));
        seed_session(&conn, "session-rotate", "agent-oc");
        setup_gateway(&conn, &server.url(), "test-token-rotate");

        // Pre-rotation baseline fetch: max seq = 100 (a long-running transcript file).
        let pre_rotation_body = serde_json::json!({
            "messages": [
                { "role": "assistant", "content": "pre-rotation turn", "timestamp": 1_756_000_000_000i64, "__openclaw": { "id": "oc-pre-rotation", "seq": 100 } }
            ]
        });
        let _m_baseline = server
            .mock("GET", "/v1/sessions/agent%3Amain%3Aopenai-user%3Apane-session-rotate/messages?limit=1")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(pre_rotation_body.to_string())
            .create_async()
            .await;

        let db = DbState::new(conn);
        let baseline_seq = fetch_oc_latest_seq("session-rotate", &db).await;
        assert_eq!(baseline_seq, Some(100), "pre-rotation baseline must reflect the pre-rotation transcript's max seq");

        // Mid-turn: OC rotates sessionId under the same stable sessionKey (gateway-side,
        // invisible to Pane's key derivation, per Reed's audit — the gateway's own
        // `session_mappings` table is what tracks the rotatable internal sessionId;
        // Pane's `oc_session_key` derivation never changes). The rotated transcript's
        // own `chat.history?limit=1` response now surfaces a DIFFERENT latest message
        // (seq 2, "Silver") — nothing like the pre-rotation file's seq 100. Deregister
        // the pre-rotation mock and register the post-rotation one on the exact same
        // route: if `fetch_oc_latest_seq` cached anything from the first call instead
        // of re-deriving fresh, this second call would still see the stale value.
        drop(_m_baseline);
        let post_rotation_body = serde_json::json!({
            "messages": [
                { "role": "assistant", "content": "Silver", "timestamp": 1_756_000_011_000i64, "__openclaw": { "id": "oc-rotated-2", "seq": 2 } }
            ]
        });
        let _m_post_rotation = server
            .mock("GET", "/v1/sessions/agent%3Amain%3Aopenai-user%3Apane-session-rotate/messages?limit=1")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(post_rotation_body.to_string())
            .create_async()
            .await;

        let fresh_baseline = fetch_oc_latest_seq("session-rotate", &db).await;
        assert_eq!(fresh_baseline, Some(2), "fetch_oc_latest_seq called again post-rotation must reflect the NEW transcript's max seq (2), never the stale pre-rotation value (100)");

    }

    // ── 4. Error-path done still tombstones own-turn messages ───────────────────

    /// Reed's audit, Requirement 2: the tombstone pass must run at ALL THREE
    /// `done:true` emit sites, including the reconnect-budget-exhausted error path
    /// — an aborted/errored turn that produced partial output must still tombstone
    /// its own echoes. This test calls `tombstone_own_turn` exactly as the error-site
    /// call in `ai_chat_stream` does (same function, same arguments), simulating that
    /// an error occurred but the OC-side turn still produced transcript output before
    /// failing (e.g. the SSE connection dropped after OC had already appended the
    /// assistant's partial reply to its own transcript).
    #[tokio::test]
    #[serial(keychain_override)]
    async fn test_error_path_done_still_tombstones_own_turn() {
        let _kc = KeychainGuard::set();

        let mut server = mockito::Server::new_async().await;
        let conn = make_conn();
        seed_agent(&conn, "agent-oc", Some("openclaw"), Some("main"));
        seed_session(&conn, "session-error", "agent-oc");
        setup_gateway(&conn, &server.url(), "test-token-error");
        mark_baselined(&conn, "session-error");

        let db = DbState::new(conn);

        // Baseline was seq 20 (recorded before dispatch). The dispatch then hit a
        // reconnect-budget-exhausted error (e.g. gateway dropped mid-stream after
        // OC's own internal agent had already appended a partial reply, "Amber", to
        // the transcript at seq 21) — the error path must STILL tombstone "Amber"
        // exactly like the success path would have.
        let post_error_body = serde_json::json!({
            "messages": [
                { "role": "assistant", "content": "earlier turn", "timestamp": 1_756_000_000_000i64, "__openclaw": { "id": "oc-earlier-err", "seq": 20 } },
                { "role": "assistant", "content": "Amber", "timestamp": 1_756_000_030_000i64, "__openclaw": { "id": "oc-amber", "seq": 21 } }
            ]
        });
        let _m = server
            .mock("GET", "/v1/sessions/agent%3Amain%3Aopenai-user%3Apane-session-error/messages?limit=20")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(post_error_body.to_string())
            .create_async()
            .await;

        // Simulates the exact call the error-site (`~9431-9449`, Reed's Site 2 of 3)
        // makes: `tombstone_own_turn(&own_turn_session_id, &db_clone, baseline_seq).await`
        // inside the `if !did_emit_done { ... }` block, regardless of the fact that
        // this branch is reached via an error (`e`), not a clean success.
        let tombstoned = tombstone_own_turn("session-error", &db, 20).await;
        assert_eq!(tombstoned, 1, "the error path must still tombstone the own-turn message (Amber, seq 21 > baseline 20)");

        // Confirm the tombstone is real and reason-tagged correctly, and that a
        // subsequent poll tick would not re-ingest "Amber" as a duplicate.
        let pull = pull_oc_transcript_for_session("session-error".to_string(), &db, false).await.unwrap();

        assert_eq!(pull.already_classified_count, 1, "Amber must be caught by the permanence pre-filter after the error-path tombstone: {:?}", pull);
        let ingest = pull.ingest.expect("ingest must run");
        // "earlier turn" (oc-earlier-err, seq 20) is genuinely new to
        // `chat_messages_local` and correctly inserts; "Amber" must be excluded,
        // which the exact inserted_count of 1 (not 2) confirms.
        assert_eq!(ingest.inserted_count, 1, "only \"earlier turn\" should insert — Amber must be excluded by the error-path own_turn tombstone: {:?}", ingest);

        let row_count: i64 = {
            let conn2 = db.0.lock().unwrap();
            conn2.query_row("SELECT COUNT(*) FROM chat_messages_local WHERE thread_id = 'session-error' AND content = 'Amber'", [], |r| r.get(0)).unwrap()
        };
        assert_eq!(row_count, 0, "Amber must never appear as a duplicate row after the error-path tombstone pass ran");
    }
}


// #3050 PR 7, commit 5's `job_session_key_resolution_tests_3050_pr7` module
// (the stored-vs-derived `oc_session_key` branch tests) is deleted here,
// #3129 (C28(b)): the production branch it tested (`pull_oc_transcript_
// for_session`'s `stored_job_oc_session_key` lookup/fallback) is retired
// — a job session is never an OC-side session at all once it runs
// through `run_agent_loop`, and `select_oc_pickup_eligible_sessions`
// (`ai::scheduler`) now excludes job sessions from this function's caller
// entirely. Both of its own tests (job-session-polls-stored-key,
// chat-session-polls-derived-key-as-the-inverse) asserted a distinction
// that no longer exists in production; deleting them is cleanup, not a
// behavior change — where a mutation would prove nothing, say so rather
// than inventing one (this is that case).


/// PR 3 (#2410, §30.11.4): tests for the frontend-facing Tauri command wrappers
/// (`pull_oc_transcript_for_session_cmd`, `session_uses_openclaw_runtime_cmd`).
/// Per SOUL.md §3 ("runtime-only bugs must be caught by integration tests that
/// actually invoke the command through Tauri's handler"), these mount a real
/// `tauri::App<MockRuntime>` with `DbState` managed via `app.manage()` — the exact
/// state-extraction path Tauri's IPC layer uses in production — rather than calling
/// the inner functions directly (already covered by the `ingest_external_messages_tests`
/// module above).
#[cfg(test)]
mod oc_pull_command_wrapper_tests {
    use super::*;
    use crate::database::DbState;
    use crate::keychain::test_support::KeychainGuard;
    use serial_test::serial;
    use tauri::Manager;

    fn setup() -> tauri::App<tauri::test::MockRuntime> {
        let conn = rusqlite::Connection::open_in_memory().expect("in-memory db");
        crate::migrations::run_migrations(&conn).expect("fresh-install migrations should succeed");
        let app = tauri::test::mock_app();
        app.manage(DbState::new(conn));
        app
    }

    fn seed_agent(conn: &rusqlite::Connection, id: &str, runtime_type: Option<&str>, runtime_ref: Option<&str>) {
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO ai_agents (id, name, slug, is_default, is_active, workspace_path, runtime_type, runtime_ref, model, created_at, updated_at)
             VALUES (?1, ?1, ?1, 0, 1, ?1, ?2, ?3, 'test-model', ?4, ?4)",
            rusqlite::params![id, runtime_type, runtime_ref, now],
        )
        .expect("seed agent");
    }

    fn seed_session(conn: &rusqlite::Connection, id: &str, agent_id: &str) {
        conn.execute(
            "INSERT INTO chat_sessions (id, project_id, agent_id) VALUES (?1, 'workspace', ?2)",
            rusqlite::params![id, agent_id],
        )
        .expect("seed session");
    }

    /// Happy path: `session_uses_openclaw_runtime_cmd` invoked through the real Tauri
    /// handler signature (`State<'_, DbState>`, the same signature `invoke()` targets
    /// in production) reports true for an openclaw-runtime session.
    #[test]
    fn test_session_uses_openclaw_runtime_cmd_true_for_openclaw_session() {
        let app = setup();
        {
            let db_state = app.state::<DbState>();
            let conn = db_state.0.lock().unwrap();
            seed_agent(&conn, "agent-oc", Some("openclaw"), Some("main"));
            seed_session(&conn, "session-oc", "agent-oc");
        }
        let db_state = app.state::<DbState>();
        let result = session_uses_openclaw_runtime_cmd("session-oc".to_string(), db_state)
            .expect("command should not error");
        assert!(result, "an openclaw-runtime session must report true through the real command handler");
    }

    /// Error path (well-formed "false", not an Err — matches the underlying helper's
    /// contract): an openrouter-runtime session reports false, not an error.
    #[test]
    fn test_session_uses_openclaw_runtime_cmd_false_for_openrouter_session() {
        let app = setup();
        {
            let db_state = app.state::<DbState>();
            let conn = db_state.0.lock().unwrap();
            seed_agent(&conn, "agent-or", Some("openrouter"), None);
            seed_session(&conn, "session-or", "agent-or");
        }
        let db_state = app.state::<DbState>();
        let result = session_uses_openclaw_runtime_cmd("session-or".to_string(), db_state)
            .expect("command should not error");
        assert!(!result, "an openrouter-runtime session must report false through the real command handler");
    }

    /// Edge case: a session id with no matching row reports false, not an error —
    /// matches `session_uses_openclaw_runtime`'s own edge-case contract, verified
    /// here through the actual command wrapper.
    #[test]
    fn test_session_uses_openclaw_runtime_cmd_false_for_missing_session() {
        let app = setup();
        let db_state = app.state::<DbState>();
        let result = session_uses_openclaw_runtime_cmd("does-not-exist".to_string(), db_state)
            .expect("command should not error");
        assert!(!result, "a missing session must report false, not error, through the real command handler");
    }

    /// Happy path: `pull_oc_transcript_for_session_cmd` invoked through the real
    /// Tauri handler signature end-to-end against a mocked gateway 200 response —
    /// proves the wrapper correctly threads `db: State<'_, DbState>` into
    /// `pull_oc_transcript_for_session`'s `&DbState` parameter (the exact
    /// State-type-mismatch class of bug SOUL.md §3 calls out as compilation-invisible).
    #[tokio::test]
    #[serial(keychain_override)]
    async fn test_pull_oc_transcript_for_session_cmd_happy_path() {
        let mut server = mockito::Server::new_async().await;
        let body = serde_json::json!({
            "messages": [{
                "role": "assistant",
                "content": "hello via the command wrapper",
                "timestamp": 1_756_000_000_000i64,
                "__openclaw": { "id": "oc-cmd-1" }
            }]
        });
        // §30.11.3: composite key contract, same as the direct-fn happy-path test.
        let _m = server
            .mock(
                "GET",
                "/v1/sessions/agent%3Amain%3Aopenai-user%3Apane-session-cmd/messages?limit=20",
            )
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(body.to_string())
            .create_async()
            .await;

        let app = setup();
        {
            let db_state = app.state::<DbState>();
            let conn = db_state.0.lock().unwrap();
            seed_agent(&conn, "agent-oc", Some("openclaw"), Some("main"));
            seed_session(&conn, "session-cmd", "agent-oc");
            crate::database::set_ai_setting(&conn, "endpoint", &server.url()).unwrap();
            // §2410 ingestion filters: pre-baseline this session so the pull below exercises
            // the steady-state ingest path, not the first-pull baseline gate.
            conn.execute(
                "UPDATE chat_sessions SET oc_baseline_done_at = datetime('now') WHERE id = 'session-cmd'",
                [],
            )
            .unwrap();
        }

        let _kc = KeychainGuard::set();
        {
            let db_state = app.state::<DbState>();
            let conn = db_state.0.lock().unwrap();
            crate::ai::credential_migration::write_gateway_token(&conn, "test-token-cmd").unwrap();
        }

        let db_state = app.state::<DbState>();
        let result = pull_oc_transcript_for_session_cmd("session-cmd".to_string(), None, db_state).await;

        assert!(result.is_ok(), "command wrapper must not error on a successful pull: {:?}", result);
        let pull = result.unwrap();
        assert_eq!(pull.fetched_count, 1);
        let ingest = pull.ingest.expect("ingest result must be present");
        assert_eq!(ingest.inserted_count, 1, "the one message must insert: {:?}", ingest);

        let row_count: i64 = {
            let db_state = app.state::<DbState>();
            let conn = db_state.0.lock().unwrap();
            conn.query_row("SELECT COUNT(*) FROM chat_messages_local WHERE thread_id = 'session-cmd'", [], |r| r.get(0))
                .unwrap()
        };
        assert_eq!(row_count, 1, "exactly 1 row must exist after the command-wrapper pull");
    }

    /// Error/skip path through the real command wrapper: a non-openclaw session must
    /// skip cleanly (no gateway call, no panic) — confirms the wrapper doesn't need
    /// its own eligibility check, since `pull_oc_transcript_for_session` already gates.
    #[tokio::test]
    async fn test_pull_oc_transcript_for_session_cmd_skips_non_openclaw_session() {
        let app = setup();
        {
            let db_state = app.state::<DbState>();
            let conn = db_state.0.lock().unwrap();
            seed_agent(&conn, "agent-or", Some("openrouter"), None);
            seed_session(&conn, "session-or-cmd", "agent-or");
        }
        let db_state = app.state::<DbState>();
        let result = pull_oc_transcript_for_session_cmd("session-or-cmd".to_string(), None, db_state).await;

        assert!(result.is_ok());
        let pull = result.unwrap();
        assert!(pull.ingest.is_none());
        assert!(pull.skip_reason.is_some());
        assert!(
            pull.skip_reason.as_ref().unwrap().contains("openclaw"),
            "skip_reason should explain the non-openclaw gate: {:?}", pull.skip_reason
        );
    }
}


// #2277 Fix 1: ai_get_soul, ai_set_soul, ai_get_workspace_memory, and ai_set_workspace_memory
// (formerly here, reading/writing the GLOBAL `.pane/ai/soul.md` and `.pane/ai/workspace-memory.md`
// files) have been REMOVED. That global read path contaminated every agent's prompt with
// whoever's personality/memory was onboarded or saved last (ADR-0060 D1). Per-agent
// SOUL.md / MEMORY.md under `.pane/system/{UUID}/` (written by both create_local_agent_impl
// and onboarding's write_system_file) are now the sole source, read via the §24 block in
// build_context_prompt with an empty-file fallback to DEFAULT_SOUL_FALLBACK (Fix 2). See
// docs/spec/ai-integration.md §26.9 for the status note.

// ai_save_upload (formerly here) wrote onboarding uploads to the GLOBAL
// `~/Pane/.pane/ai/uploads/` directory. Those files were orphans — written to
// disk, never referenced again after the model synthesized SOUL.md/MEMORY.md
// from their content. Removed per agents.md §24.10.1b (#2288 PR 4). Per-agent
// upload saving (`.pane/system/{UUID}/uploads/`) is a future enhancement
// tracked separately.

/// Detect available agents from OpenClaw gateway during onboarding.
/// Direct HTTP to OC — onboarding runs before gateway WS is established.
/// See INVARIANT #1 in docs/INVARIANTS.md.
#[tauri::command]
pub async fn detect_openclaw_agents(
    url: String,
    token: String,
    db: tauri::State<'_, DbState>,
) -> Result<Vec<serde_json::Value>, String> {
    eprintln!("[ONBOARD] Detecting OpenClaw agents from {}", url);

    // Use gateway client (cert should be stored from pairing flow)
    let client = crate::gateway_client::build_gateway_client(&db)?;
    let base = url.trim_end_matches('/').trim_end_matches("/v1");
    let resp = client
        .get(format!("{}/v1/models", base))
        .header("Authorization", format!("Bearer {}", token))
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await;

    match resp {
        Ok(r) if r.status().is_success() => {
            let body: serde_json::Value = r
                .json()
                .await
                .map_err(|e| format!("Failed to parse /v1/models response: {}", e))?;

            // OC returns {"object": "list", "data": [{"id": "model-name", ...}, ...]}
            if let Some(data) = body.get("data").and_then(|d| d.as_array()) {
                eprintln!("[ONBOARD] Found {} agent(s)", data.len());
                Ok(data.clone())
            } else {
                eprintln!("[ONBOARD] No 'data' array in response, returning empty list");
                Ok(vec![])
            }
        }
        Ok(r) => {
            let status = r.status();
            eprintln!("[ONBOARD] OpenClaw detection failed: HTTP {}", status);
            Err(format!("OpenClaw returned HTTP {}", status))
        }
        Err(e) => {
            eprintln!("[ONBOARD] OpenClaw detection request failed: {}", e);
            Err(format!("Failed to connect to OpenClaw: {}", e))
        }
    }
}


/// Perform initial sync from OpenClaw gateway (§6.5.1)
/// Retrieves full current state of all agent files for fresh install
/// Return shape for `sync_initial_import` (#2647 D17 fix).
///
/// `agent_names` is exactly the old `Vec<String>` return value — display names in
/// gateway order, unchanged in content or ordering from before this fix.
///
/// `agents_missing_model` lists the subset of `agent_names` (today: at most the
/// main agent, since only `oc_id == "main"` ever gets a model from `/v1/agents`)
/// that landed with `model = NULL` because the internal `fetch_oc_agent_model` call
/// soft-failed (gateway briefly unreachable for `/v1/agents` specifically, even
/// though `/v1/sync/initial` succeeded). The frontend's import-completion popup
/// must check this list and name the affected agent(s) with actionable copy (e.g.
/// "Agent 'Brit' imported without a model — set one in Settings → AI") rather than
/// silently absorbing the gap — a log-only soft-fail here is NOT acceptable; that is
/// the exact D17 symptom (user discovers "No model configured" on first chat with no
/// prior warning) applied to a slightly different trigger.
#[derive(serde::Serialize, Debug, Clone, PartialEq)]
pub struct SyncImportOutcome {
    pub agent_names: Vec<String>,
    pub agents_missing_model: Vec<String>,
}


/// Creates agent records in DB with runtime_ref populated (#364)
///
/// Converted to async + spawn_blocking (#1296) — mirrors lib.rs:921 startup-sync pattern.
/// Blocking HTTP (reqwest::blocking) and DB writes run on a thread-pool thread so the
/// Tauri main/UI thread is never blocked during onboarding.
///
/// #2647 D17 fix: return type widened from `Vec<String>` to `SyncImportOutcome` so the
/// model soft-fail (see below) can be surfaced to the frontend for the import-completion
/// popup, not just logged. `agent_names` preserves the exact old `Vec<String>` contents
/// and ordering — both existing callers (Onboarding.svelte, ProviderConnectionCard.svelte)
/// are updated to read `.agent_names` instead of the bare array.
#[tauri::command]
pub async fn sync_initial_import(
    gateway_url: String,
    token: String,
    db: tauri::State<'_, DbState>,
) -> Result<SyncImportOutcome, String> {
    eprintln!(
        "[SYNC] sync_initial_import called: url={}, token_prefix={}",
        gateway_url,
        &token.chars().take(8).collect::<String>()
    );

    // #2647 D17 fix: resolve the main (`oc_id == "main"`) agent's gateway-side model
    // BEFORE the DB transaction, via the same `/v1/agents` endpoint + parsing logic
    // `ai_configure_and_save` already uses (Reed's ON CONFLICT audit §2 Q3, shape 1 —
    // reuse `fetch_oc_agent_model`, don't invent a second parser). Must happen here,
    // in the async context, before `spawn_blocking` — `fetch_oc_agent_model` is itself
    // async and does its own HTTP GET.
    //
    // Soft-fail (CRITICAL, per Matt's ruling — Reed's audit §5 risk #2): if this fetch
    // fails (e.g. the gateway is briefly unreachable for `/v1/agents` specifically, even
    // though `/v1/sync/initial` above just succeeded), do NOT abort the whole import.
    // Bind `model = NULL` for the main agent, log a warning, and track the agent name so
    // the caller can surface a real, actionable warning to the user ("Agent 'X' imported
    // without a model — set one in Settings → AI") rather than a silent log-only failure
    // (the exact D17 symptom this PR fixes).
    let main_agent_model: Option<String> = match fetch_oc_agent_model(&gateway_url, &token, &db).await {
        Ok(model) => {
            eprintln!("[SYNC] Resolved main agent model for import: {}", model);
            Some(model)
        }
        Err(e) => {
            eprintln!(
                "[SYNC] WARNING: failed to resolve main agent's model from gateway (soft-fail, \
                 model will be NULL and the import will still proceed): {}",
                e
            );
            None
        }
    };

    // Clone the Arc so it can be moved into spawn_blocking.
    // The lock is acquired inside the closure — never held across an await point.
    let db_arc = db.0.clone();

    tokio::task::spawn_blocking(move || {
    // Pre-read cert_pem from DB before calling initial_sync so the SPKI-pinned
    // builder is used for the /v1/sync/initial request (#2011).
    let cert_pem = {
        let conn = db_arc.lock().map_err(|e| e.to_string())?;
        crate::database::get_ai_setting(&conn, "gateway_tls_cert_pem")
    };

    // Call initial_sync to retrieve files from gateway
    eprintln!("[SYNC] Calling sync_client::initial_sync...");
    let imported_agents = crate::sync_client::initial_sync(&gateway_url, &token, cert_pem)?;

    if imported_agents.is_empty() {
        eprintln!("[SYNC] No agents to import");
        return Ok(SyncImportOutcome { agent_names: vec![], agents_missing_model: vec![] });
    }

    eprintln!("[SYNC] initial_sync returned {} agent(s)", imported_agents.len());

    // Create agent records in database with runtime_ref
    // Lock acquired inside spawn_blocking — not held across any await point
    let conn = db_arc.lock().map_err(|e| e.to_string())?;
    let now = chrono::Utc::now().to_rfc3339();
    let pane_fs = PaneFs::for_runtime().map_err(|e| policy_error("Filesystem policy", e))?;

    // BUG 5 FIX: Wrap in transaction
    conn.execute("BEGIN TRANSACTION", [])
        .map_err(|e| format!("Failed to begin transaction: {}", e))?;

    // #2640 D13 FIX: Pre-flight check for an existing default agent, INSIDE the
    // transaction (same rollback boundary as the per-agent INSERT loop below).
    // Reed's audit (docs/audits/2026-08-21-d13-is-default-collision-scoping.md §5):
    // without this check, an OC main agent import unconditionally sets
    // is_default=1, which collides with idx_ai_agents_default (a partial UNIQUE
    // index on is_default WHERE is_default=1) whenever a non-OC default agent
    // (e.g. an OpenRouter-onboarded default) already exists — the ON CONFLICT
    // clause only names the (runtime_type, runtime_ref) index, so this collision
    // is not caught and rolls back the entire batch.
    let default_agent_exists: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM ai_agents WHERE is_default = 1)",
            [],
            |r| r.get(0),
        )
        .map_err(|e| format!("Failed to check for existing default agent: {}", e))?;
    eprintln!("[SYNC] Pre-flight check: default_agent_exists={}", default_agent_exists);

    // #2640 D13 FIX: Pre-populate claimed_slugs from all existing ai_agents rows so
    // the slug-collision probe below covers pre-batch state (ported from
    // sync_re_import's claimed_slugs pattern, sync_client.rs:1550-1591).
    let mut claimed_slugs: std::collections::HashSet<String> = std::collections::HashSet::new();
    {
        let mut stmt = conn
            .prepare("SELECT slug FROM ai_agents")
            .map_err(|e| format!("Failed to prepare slug query: {}", e))?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|e| format!("Failed to query slugs: {}", e))?;
        for row in rows {
            let slug = row.map_err(|e| format!("Slug row error: {}", e))?;
            claimed_slugs.insert(slug);
        }
    }

    // #2647 D17 fix: track which imported agent(s) landed without a model so the
    // caller can surface a real warning (see the struct doc comment on
    // `SyncImportOutcome`) instead of a silent log-only soft-fail.
    let mut agents_missing_model: Vec<String> = Vec::new();

    let result = (|| -> Result<(), String> {
        for (oc_id, name, uuid, oc_reported_default) in &imported_agents {
            // BUG 12 FIX: Handle empty agent names
            let agent_name = if name.trim().is_empty() {
                format!("Agent {}", oc_id)
            } else {
                name.clone()
            };

            // #2647 D17 fix: only the `oc_id == "main"` agent gets a model from this
            // fetch (subagents have no gateway-side model concept exposed by
            // `/v1/agents` — confirmed by `fetch_oc_agent_model`'s own "Today: always
            // resolve against the `main` agent" comment and Reed's audit §2 Q3).
            let agent_model: Option<&str> = if oc_id == "main" {
                main_agent_model.as_deref()
            } else {
                None
            };
            if oc_id == "main" && agent_model.is_none() {
                agents_missing_model.push(agent_name.clone());
            }

            // #2640 D13 FIX: Slug collision avoidance (ported from sync_re_import,
            // sync_client.rs:1832-1856). No rollback on collision; the batch
            // continues with a suffixed slug.
            let base_slug = agent_name.to_lowercase().replace(" ", "-").replace("_", "-");
            let slug = if !claimed_slugs.contains(&base_slug) {
                base_slug.clone()
            } else {
                let mut n = 1usize;
                loop {
                    let candidate = format!("{}-{}", base_slug, n);
                    if !claimed_slugs.contains(&candidate) {
                        break candidate;
                    }
                    n += 1;
                }
            };
            if slug != base_slug {
                eprintln!("[SYNC] Slug collision: {} already claimed, using {}", base_slug, slug);
            }
            claimed_slugs.insert(slug.clone());

            // Every row in this sync-import loop is OC-imported by construction (this
            // function handles exclusively the OC initial-sync path), so runtime_type is
            // always 'openclaw' here.
            let runtime_type = "openclaw";
            // BUG 2 FIX: Determine if this is the default agent by checking runtime_ref.
            // ADR-0063 D1 semantic-trap ruling: `runtime_ref == "main"` is OC's own "main"
            // slug in OC's namespace, never Pane's default-agent flag by itself — gated
            // on `runtime_type == 'openclaw'` per the ruling (trivially true in this
            // exclusively-OC-import loop, made explicit rather than assumed).
            //
            // #2640 D13 FIX: additionally gated on `!default_agent_exists` — per Matt's
            // ruling, "No import path may ever steal or collide on default status." If a
            // default agent already exists (e.g. an OpenRouter-onboarded default), the
            // imported OC main agent lands as non-default; the existing default keeps
            // its seat. This is a one-directional check, NOT a demote-then-promote (that
            // pattern belongs to sync_re_import and is explicitly out of scope here).
            //
            // ADR-0067 D5 / spec §22C.6.2: routed through the shared three-tier
            // resolver (`resolve_should_be_default`, sync_client.rs) — tier 1 is
            // `oc_reported_default` (OC's own isDefault, correct-but-dormant on
            // 2026.8.2), tier 2 is the `default_agent_exists` protection above
            // (every agent here is a FRESH import, so `already_default_here` is
            // always false — no row can already hold this seat), tier 3 is the
            // `oc_id == "main"` fallback, unchanged from before this fix.
            let is_default = runtime_type == "openclaw"
                && crate::sync_client::resolve_should_be_default(
                    *oc_reported_default,
                    oc_id == "main",
                    default_agent_exists,
                    false, // fresh import: no row can already hold the default seat
                );

            // #2640 D13 FIX: role derivation.
            //   - is_default=true            -> role='main'  (the actual default agent)
            //   - is_default=false, oc_id=="main" -> role='local' (OC main agent, but not
            //     default because one already existed; must stay pickable in the agent
            //     picker per DefaultAgentPickerModal.svelte's `role !== 'sub'` filter —
            //     role='sub' would get it filtered out, which is wrong for OC's own main
            //     agent)
            //   - is_default=false, oc_id!="main" -> role='sub' (actual OC subagents)
            let role = if is_default {
                "main"
            } else if oc_id == "main" {
                "local"
            } else {
                "sub"
            };

            // ADR-0039 Decision 7: Write IDENTITY.md BEFORE DB insert.
            // If the write fails, abort the entire import — do not create an agent record without IDENTITY.md.
            {
                PaneFs::validate_name(uuid)
                    .map_err(|e| policy_error("Unsafe imported agent workspace", e))?;
                let agent_relative = PaneFs::relative_from_names([uuid.as_str()])
                    .map_err(|e| policy_error("Unsafe imported agent directory", e))?;
                pane_fs
                    .create_dir_all(PaneRoot::System, &agent_relative)
                    .map_err(|e| format!("Failed to create agent dir for {}: {}", agent_name, e))?;
                let identity_relative = PaneFs::relative_from_names([uuid.as_str(), "IDENTITY.md"])
                    .map_err(|e| policy_error("Unsafe imported identity path", e))?;
                let identity_path = pane_fs
                    .authorize_write(PaneRoot::System, &identity_relative)
                    .map_err(|e| policy_error("Unsafe imported identity destination", e))?;
                if !identity_path.as_path().exists() {
                    eprintln!("[SYNC] Writing IDENTITY.md for agent: {}", agent_name);
                    let content = format!("# {}\n\nYour name is {}.\n", agent_name, agent_name);
                    atomic_write(identity_path.as_path(), &content)
                        .map_err(|e| format!("Failed to write IDENTITY.md for {}: {}", agent_name, e))?;
                    eprintln!("[SYNC] IDENTITY.md written for: {}", agent_name);
                }
            }

            // BUG 6 FIX: Use ON CONFLICT(runtime_type, runtime_ref) for duplicate detection
            // — matches the new idx_ai_agents_runtime_unique composite index (ADR-0063 D4
            // step 4; the conflict target must name the index's exact column set).
            //
            // #2647 D17 fix: `model` added to the INSERT column list (previously absent
            // entirely — the D17 gap this PR closes; ai_configure_and_save's old model
            // UPDATE could never reach a row this INSERT hadn't created yet, and even
            // once it existed the INSERT itself had no column to receive it). The
            // ON CONFLICT clause uses Reed's simplified Option C
            // (docs/audits/2026-08-21-pr1-on-conflict-scope-check.md §1 Q1): preserve any
            // existing non-NULL model (never clobber a user's chosen model on a stale
            // pre-D18 re-pair through this same function), but self-heal a NULL row (e.g.
            // one created before this fix landed) from `excluded.model`.
            //
            // F5 fix (#2455 venice live-test v2): a role self-heal added to the
            // ON CONFLICT SET list, so a second sync_initial_import re-run repairs any
            // role corruption inflicted by the pre-fix 1b safety net (which used to
            // force-write role='main' onto whatever row it picked) without waiting for
            // a fresh migration/app-upgrade — matching migrate_v89's one-time repair
            // pass for rows that never get re-imported again.
            //
            // IMPORTANT — this is deliberately NOT `role = excluded.role`. This loop's
            // `is_default`/`role` locals (computed above) are gated on the one-shot
            // `!default_agent_exists` pre-flight check taken before the whole loop
            // started (see #2640 D13 FIX above) — they do NOT account for "this row
            // was already the default before this re-run started" the way
            // sync_re_import's `land_as_default`/`already_default_here` (#2644) does.
            // Blindly writing `excluded.role` would, on a second import run, treat OC's
            // own already-promoted main-agent row (runtime_ref='main') as if
            // `default_agent_exists` were still true from THIS run's perspective and
            // force it from role='main' down to role='local' — corrupting a row that
            // was never broken. Scoping the self-heal to `excluded.runtime_ref != 'main'`
            // (same refined predicate as migrate_v89's repair, per ADR-0063 D1: 'main'
            // is OC's own reserved sentinel, never a generic subagent signal) fixes
            // exactly the corruption this bug targets — real OC subagent rows — while
            // leaving OC's own main/local row's role completely untouched here, same as
            // migrate_v89's repair pass does for the same reason.
            conn.execute(
                "INSERT INTO ai_agents (id, name, slug, role, is_default, workspace_path, runtime_type, runtime_ref, model, source, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'import', ?10, ?11)
                 ON CONFLICT(runtime_type, runtime_ref) DO UPDATE SET
                   name = excluded.name,
                   workspace_path = excluded.workspace_path,
                   updated_at = excluded.updated_at,
                   model = CASE WHEN ai_agents.model IS NULL THEN excluded.model ELSE ai_agents.model END,
                   role = CASE WHEN excluded.runtime_ref != 'main' THEN 'sub' ELSE ai_agents.role END,
                   is_active = 1
                 WHERE runtime_ref IS NOT NULL",
                rusqlite::params![
                    uuid,
                    agent_name,
                    slug,
                    role,
                    if is_default { 1 } else { 0 },
                    uuid,      // workspace_path = UUID
                    runtime_type,
                    oc_id,     // runtime_ref from gateway
                    agent_model, // NULL for subagents and on model-fetch soft-fail
                    &now,
                    &now
                ],
            ).map_err(|e| format!("Failed to insert agent {}: {}", agent_name, e))?;

            eprintln!("[SYNC] Created agent record: {} (oc_id: {}, uuid: {}, is_default: {}, role: {}, model: {:?})", agent_name, oc_id, uuid, is_default, role, agent_model);
        }
        Ok(())
    })();

    match result {
        Ok(_) => {
            conn.execute("COMMIT", [])
                .map_err(|e| format!("Failed to commit transaction: {}", e))?;
            eprintln!("[SYNC] Transaction committed successfully");

            // BUG 22 FIX: Log import success summary
            eprintln!("[SYNC] sync_initial_import complete: imported {} agent(s)", imported_agents.len());
            for (oc_id, name, uuid, _) in &imported_agents {
                let agent_name = if name.trim().is_empty() {
                    format!("Agent {}", oc_id)
                } else {
                    name.clone()
                };
                eprintln!("  - {} (oc_id: {}, uuid: {})", agent_name, oc_id, uuid);
            }

            // Return list of agent names for onboarding display, plus (#2647 D17 fix)
            // the list of agents that landed without a model so the caller can surface a
            // real warning instead of a silent log-only soft-fail.
            if !agents_missing_model.is_empty() {
                eprintln!(
                    "[SYNC] WARNING: {} agent(s) imported without a model: {:?}",
                    agents_missing_model.len(),
                    agents_missing_model
                );
            }
            Ok(SyncImportOutcome {
                agent_names: imported_agents.iter().map(|(oc_id, name, _, _)| {
                    if name.trim().is_empty() {
                        format!("Agent {}", oc_id)
                    } else {
                        name.clone()
                    }
                }).collect(),
                agents_missing_model,
            })
        }
        Err(e) => {
            conn.execute("ROLLBACK", [])
                .map_err(|rollback_err| format!("Failed to rollback after error '{}': {}", e, rollback_err))?;

            // BUG 5 FIX: Clean up orphaned files on rollback
            for (_, _, uuid, _) in &imported_agents {
                let cleanup_result = PaneFs::validate_name(uuid)
                    .map_err(|e| policy_error("Unsafe rollback agent workspace", e))
                    .and_then(|_| {
                        pane_fs
                            .remove_dir_all(PaneRoot::System, std::path::Path::new(uuid))
                            .map_err(|e| e.to_string())
                    });
                match cleanup_result {
                    Ok(()) => {
                        eprintln!("[SYNC] Rollback cleanup: removed {}", uuid);
                    }
                    Err(cleanup_err) => {
                        eprintln!("[SYNC] Warning: Failed to clean up {} after rollback: {}", uuid, cleanup_err);
                    }
                }
            }

            eprintln!("[SYNC] Import failed and rolled back: {}", e);
            Err(format!("Import failed, rolled back: {}", e))
        }
    }
    }) // end spawn_blocking
    .await
    .map_err(|e| format!("[SYNC] spawn_blocking panicked: {:?}", e))?
}


/// #2647/#2648/#2644 (PR1 5a) — DB row-existence check, NOT a connectivity/token check.
///
/// Returns `true` iff any `ai_agents` row with `runtime_type = 'openclaw'` exists,
/// regardless of `is_active`, keychain token validity, or `ai_list_runtime_connections`'
/// `connected` flag. This is the first-pair-vs-re-pair signal `completePairing()`
/// (`ProviderConnectionCard.svelte`) uses to branch between `sync_initial_import`
/// (first pair) and `reimport_oc_agents` (re-pair).
///
/// Reed's e2e audit (docs/audits/2026-08-21-oc-settings-path-e2e-scoping.md, NEW-3 /
/// §5a) explicitly warns against reusing `info.connected` for this: `connected` is
/// derived from `build_provider_for_runtime_type` succeeding, which requires a valid
/// keychain token — a paired-but-token-revoked install (rows present, token gone)
/// would incorrectly report `connected: false` and route to the first-pair path,
/// which would then call `sync_initial_import` against an already-imported agent
/// (D18's exact bug, re-triggered). This command sidesteps that entirely by querying
/// the DB directly.
#[tauri::command]
pub fn oc_agent_row_exists(db: State<'_, DbState>) -> Result<bool, String> {
    eprintln!("[PAIRING] oc_agent_row_exists called");
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let exists: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM ai_agents WHERE runtime_type = 'openclaw')",
            [],
            |r| r.get(0),
        )
        .map_err(|e| format!("Failed to check for existing OpenClaw agent row: {}", e))?;
    eprintln!("[PAIRING] oc_agent_row_exists result: {}", exists);
    Ok(exists)
}


#[cfg(test)]
mod oc_agent_row_exists_tests {
    use super::*;
    use crate::database::DbState;
    use crate::keychain::test_support::KeychainGuard;
    use serial_test::serial;
    use tauri::Manager;

    fn setup() -> tauri::App<tauri::test::MockRuntime> {
        let conn = rusqlite::Connection::open_in_memory().expect("in-memory db");
        crate::migrations::run_migrations(&conn).expect("fresh-install migrations should succeed");
        let app = tauri::test::mock_app();
        app.manage(DbState::new(conn));
        app
    }

    /// Happy path: no rows at all → false (fresh install, must route to first-pair).
    #[test]
    fn test_no_rows_returns_false() {
        let app = setup();
        let db_state = app.state::<DbState>();
        let result = oc_agent_row_exists(db_state).expect("command should not error on empty table");
        assert!(!result, "a fresh install with zero ai_agents rows must report false");
    }

    /// Happy path: an openclaw row exists → true, regardless of is_active or model.
    #[test]
    fn test_openclaw_row_exists_returns_true() {
        let app = setup();
        {
            let db_state = app.state::<DbState>();
            let conn = db_state.0.lock().unwrap();
            let now = chrono::Utc::now().to_rfc3339();
            conn.execute(
                "INSERT INTO ai_agents (id, name, slug, role, is_default, is_active, workspace_path, runtime_type, runtime_ref, model, created_at, updated_at)
                 VALUES ('uuid-1', 'Stia', 'stia', 'main', 1, 1, 'uuid-1', 'openclaw', 'main', NULL, ?1, ?1)",
                rusqlite::params![now],
            )
            .unwrap();
        }
        let db_state = app.state::<DbState>();
        let result = oc_agent_row_exists(db_state).expect("command should not error");
        assert!(result, "an existing runtime_type='openclaw' row must report true, even with model=NULL");
    }

    /// Edge case: the row exists but the keychain token is revoked/missing. This is
    /// exactly the D18-adjacent scenario Reed's audit flags — the check must still
    /// return true (it never touches the keychain at all).
    #[test]
    #[serial(keychain_override)]
    fn test_returns_true_even_with_no_keychain_token() {
        let _kc = KeychainGuard::set();
        let app = setup();
        {
            let db_state = app.state::<DbState>();
            let conn = db_state.0.lock().unwrap();
            let now = chrono::Utc::now().to_rfc3339();
            conn.execute(
                "INSERT INTO ai_agents (id, name, slug, role, is_default, is_active, workspace_path, runtime_type, runtime_ref, model, created_at, updated_at)
                 VALUES ('uuid-1', 'Stia', 'stia', 'main', 1, 1, 'uuid-1', 'openclaw', 'main', NULL, ?1, ?1)",
                rusqlite::params![now],
            )
            .unwrap();
        }
        // No keychain token written at all (MockKeychain starts empty) — simulates a
        // revoked/missing token. The row-existence check must be unaffected.
        let db_state = app.state::<DbState>();
        let result = oc_agent_row_exists(db_state).expect("command should not error");
        assert!(result, "row-existence must return true even when no gateway token exists in the keychain");
    }

    /// Error path: a non-openclaw row (e.g. openrouter default) must NOT count.
    #[test]
    fn test_non_openclaw_rows_return_false() {
        let app = setup();
        {
            let db_state = app.state::<DbState>();
            let conn = db_state.0.lock().unwrap();
            let now = chrono::Utc::now().to_rfc3339();
            conn.execute(
                "INSERT INTO ai_agents (id, name, slug, role, is_default, is_active, workspace_path, runtime_type, runtime_ref, created_at, updated_at)
                 VALUES ('uuid-2', 'Opal', 'opal', 'main', 1, 1, 'uuid-2', 'openrouter', NULL, ?1, ?1)",
                rusqlite::params![now],
            )
            .unwrap();
        }
        let db_state = app.state::<DbState>();
        let result = oc_agent_row_exists(db_state).expect("command should not error");
        assert!(!result, "a non-openclaw runtime_type row must not count as an existing OC pairing");
    }
}


// ── §24.5.5 Provider-Switch Re-import Commands ───────────────────────────────
//
// #956 — Settings: switching to OpenClaw must trigger agent discovery + sync
// Spec: docs/spec/agents.md §24.5.5

/// Collision entry returned by `check_oc_agent_collisions`.
#[derive(serde::Serialize)]
pub struct AgentCollision {
    pub oc_agent_id: String,
    pub local_name: String,
    pub gateway_name: String,
}


/// New (net-new) agent entry returned by `check_oc_agent_collisions`.
#[derive(serde::Serialize)]
pub struct NewAgentEntry {
    pub oc_agent_id: String,
    pub name: String,
}


/// Result returned by `check_oc_agent_collisions`.
#[derive(serde::Serialize)]
pub struct CollisionCheckResult {
    pub collisions: Vec<AgentCollision>,
    pub new_agents: Vec<NewAgentEntry>,
}


/// §24.5.5 — Read-only pre-flight check before provider-switch re-import.
///
/// Fetches the gateway agent list and compares each gateway `oc_id` against
/// existing `ai_agents` rows' `runtime_ref`. Returns `{ collisions, new_agents }` with
/// **zero DB mutations and zero file writes**.
///
/// The frontend shows a warn-then-confirm modal when `collisions.len() >= 1`;
/// this command provides the data without executing any mutation.
#[tauri::command]
pub async fn check_oc_agent_collisions(
    url: String,
    token: String,
    db: tauri::State<'_, DbState>,
) -> Result<CollisionCheckResult, String> {
    eprintln!(
        "[REIMPORT] check_oc_agent_collisions: url={}, token_prefix={}",
        url,
        &token.chars().take(8).collect::<String>()
    );

    // Clone Arc so it can be moved into spawn_blocking
    let db_arc = db.0.clone();

    tokio::task::spawn_blocking(move || {
        // Pre-read cert PEM before taking the lock to avoid re-entrant deadlock (#1350).
        let cert_pem = {
            let conn = db_arc.lock().map_err(|e| e.to_string())?;
            crate::database::get_ai_setting(&conn, "gateway_tls_cert_pem")
        };

        // Fetch gateway agents — read-only, no DB writes.
        let gateway_agents = crate::sync_client::fetch_gateway_agents(&url, &token, cert_pem)
            .map_err(|e| e.to_string())?;

        // Pre-query existing OC-linked agents: runtime_ref → (id, name).
        let conn = db_arc.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare(
                "SELECT id, runtime_ref, workspace_path, name \
                 FROM ai_agents \
                 WHERE runtime_ref IS NOT NULL",
            )
            .map_err(|e| format!("[REIMPORT] Failed to prepare query: {}", e))?;

        // runtime_ref → local name
        let mut local_map: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(1)?, // runtime_ref
                    row.get::<_, String>(3)?, // name
                ))
            })
            .map_err(|e| format!("[REIMPORT] Query error: {}", e))?;
        for row in rows {
            let (oc_id, name) = row.map_err(|e| format!("[REIMPORT] Row error: {}", e))?;
            local_map.insert(oc_id, name);
        }

        let mut collisions: Vec<AgentCollision> = Vec::new();
        let mut new_agents: Vec<NewAgentEntry> = Vec::new();

        for agent in &gateway_agents {
            let gateway_name = if agent.name.trim().is_empty() {
                format!("Agent {}", agent.oc_id)
            } else {
                agent.name.clone()
            };

            match local_map.get(&agent.oc_id) {
                Some(local_name) => collisions.push(AgentCollision {
                    oc_agent_id: agent.oc_id.clone(),
                    local_name: local_name.clone(),
                    gateway_name,
                }),
                None => new_agents.push(NewAgentEntry {
                    oc_agent_id: agent.oc_id.clone(),
                    name: gateway_name,
                }),
            }
        }

        eprintln!(
            "[REIMPORT] check_oc_agent_collisions: {} collision(s), {} new agent(s)",
            collisions.len(),
            new_agents.len()
        );

        Ok(CollisionCheckResult {
            collisions,
            new_agents,
        })
    })
    .await
    .map_err(|e| format!("[REIMPORT] spawn_blocking panicked: {:?}", e))?
}


/// §24.5.5 — Mutating provider-switch re-import command.
///
/// SELF-CONTAINED: performs its OWN `fetch_gateway_agents` call and its OWN
/// `ai_agents` pre-query at execution time. Does NOT consume or receive any
/// result from `check_oc_agent_collisions`. The two commands share NO state.
/// The TOCTOU gap between the modal and the user click means this command
/// always re-fetches fresh data from the gateway.
///
/// Calls `sync_re_import`, returns the list of imported/updated agent names.
/// Uses `tokio::task::spawn_blocking` (same pattern as `sync_initial_import`).
/// Uses `gateway_client::build_gateway_client_blocking_with_cert` for pinned TLS.
#[tauri::command]
pub async fn reimport_oc_agents(
    url: String,
    token: String,
    db: tauri::State<'_, DbState>,
) -> Result<Vec<String>, String> {
    eprintln!(
        "[REIMPORT] reimport_oc_agents: url={}, token_prefix={}",
        url,
        &token.chars().take(8).collect::<String>()
    );

    // Clone Arc so it can be moved into spawn_blocking.
    // Lock is acquired inside the closure — never held across an await point.
    let db_arc = db.0.clone();

    tokio::task::spawn_blocking(move || {
        // Pre-read cert PEM before taking the lock to avoid re-entrant deadlock (#1350).
        let cert_pem = {
            let conn = db_arc.lock().map_err(|e| e.to_string())?;
            crate::database::get_ai_setting(&conn, "gateway_tls_cert_pem")
        };

        // Acquire fresh DB connection for the mutating phase.
        // sync_re_import will hold this connection for the transaction duration.
        let conn = db_arc.lock().map_err(|e| e.to_string())?;

        // Delegate to sync_re_import — self-contained fetch + file writes + DB mutations.
        let results = crate::sync_client::sync_re_import(&conn, &url, &token, cert_pem, None)?;

        let names: Vec<String> = results.into_iter().map(|(_, name, _)| name).collect();

        eprintln!(
            "[REIMPORT] reimport_oc_agents complete: {} agent(s)",
            names.len()
        );
        Ok(names)
    })
    .await
    .map_err(|e| format!("[REIMPORT] spawn_blocking panicked: {:?}", e))?
}


/// Deduplicate Pane System projects and system notes.
/// Safe to call multiple times — idempotent.
/// Called from sync_pull after applying changes.
pub fn dedup_pane_system(conn: &rusqlite::Connection) {
    // 1. Count Pane System projects
    let ps_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM projects WHERE name = 'Pane System'",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);

    if ps_count <= 1 {
        // No duplicate projects — still check for duplicate folders/notes within the single one
        let pane_system_id: String = match conn.query_row(
            "SELECT id FROM projects WHERE name = 'Pane System'",
            [],
            |row| row.get(0),
        ) {
            Ok(id) => id,
            Err(_) => return, // No Pane System at all
        };
        dedup_folders_and_notes(conn, &pane_system_id);
        return;
    }

    eprintln!(
        "[DEDUP] Found {} 'Pane System' projects, deduplicating",
        ps_count
    );

    // 2. Keep the one with is_system=1 (or first created)
    let keep_id: String = match conn.query_row(
        "SELECT id FROM projects WHERE name = 'Pane System' AND is_system = 1 LIMIT 1",
        [],
        |row| row.get(0),
    ) {
        Ok(id) => id,
        Err(_) => {
            match conn.query_row(
                "SELECT id FROM projects WHERE name = 'Pane System' ORDER BY created_at ASC LIMIT 1",
                [],
                |row| row.get(0),
            ) {
                Ok(id) => id,
                Err(_) => return,
            }
        }
    };

    // 3. Get IDs of duplicate Pane System projects
    let dup_ps_ids: Vec<String> =
        match conn.prepare("SELECT id FROM projects WHERE name = 'Pane System' AND id != ?1") {
            Ok(mut stmt) => stmt
                .query_map(rusqlite::params![keep_id], |row| row.get(0))
                .unwrap()
                .filter_map(|r| r.ok())
                .collect(),
            Err(_) => return,
        };

    for dup_id in &dup_ps_ids {
        // Get child folders of the duplicate
        let child_ids: Vec<String> =
            match conn.prepare("SELECT id FROM projects WHERE parent_id = ?1") {
                Ok(mut stmt) => stmt
                    .query_map(rusqlite::params![dup_id], |row| row.get(0))
                    .unwrap()
                    .filter_map(|r| r.ok())
                    .collect(),
                Err(e) => {
                    eprintln!("[DEDUP] Failed to prepare child folder query for dup_id={}: {}", dup_id, e);
                    Vec::new()
                }
            };

        // Soft-delete notes in duplicate folders
        for child_id in &child_ids {
            conn.execute(
                "UPDATE notes SET deleted_at = datetime('now') WHERE folder_id = ?1 AND deleted_at IS NULL",
                rusqlite::params![child_id],
            ).ok();
        }

        // Soft-delete notes directly in the duplicate project
        conn.execute(
            "UPDATE notes SET deleted_at = datetime('now') WHERE project_id = ?1 AND deleted_at IS NULL",
            rusqlite::params![dup_id],
        ).ok();

        // Delete child folder entries
        for child_id in &child_ids {
            conn.execute(
                "DELETE FROM projects WHERE id = ?1",
                rusqlite::params![child_id],
            )
            .ok();
        }

        // Delete the duplicate project
        conn.execute(
            "DELETE FROM projects WHERE id = ?1",
            rusqlite::params![dup_id],
        )
        .ok();
    }

    eprintln!(
        "[DEDUP] Kept Pane System id={}, removed {} duplicates",
        keep_id,
        dup_ps_ids.len()
    );

    // Now dedup folders and notes within the kept project
    dedup_folders_and_notes(conn, &keep_id);
}


/// Deduplicate agent folders and system notes within a single Pane System project.
fn dedup_folders_and_notes(conn: &rusqlite::Connection, pane_system_id: &str) {
    // 4. Dedup agent folders within the kept Pane System
    let dup_folders: Vec<(String, String)> = match conn.prepare(
        "SELECT name, GROUP_CONCAT(id) as ids FROM projects WHERE parent_id = ?1 GROUP BY name HAVING COUNT(*) > 1"
    ) {
        Ok(mut stmt) => stmt.query_map(rusqlite::params![pane_system_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        }).unwrap().filter_map(|r| r.ok()).collect(),
        Err(e) => {
            eprintln!("[DEDUP] Failed to prepare duplicate-folder query for pane_system_id={}: {}", pane_system_id, e);
            Vec::new()
        }
    };

    for (folder_name, ids_csv) in &dup_folders {
        let ids: Vec<&str> = ids_csv.split(',').collect();
        if ids.len() <= 1 {
            continue;
        }

        // Keep the one with is_system=1, or first one
        let keep_folder: String = conn.query_row(
            &format!("SELECT id FROM projects WHERE id IN ({}) ORDER BY is_system DESC, created_at ASC LIMIT 1",
                ids.iter().map(|_| "?").collect::<Vec<_>>().join(",")),
            rusqlite::params_from_iter(ids.iter()),
            |row| row.get(0),
        ).unwrap_or_else(|_| ids[0].to_string());

        for id in &ids {
            if *id != keep_folder.as_str() {
                // Move notes from duplicate folder to kept folder
                conn.execute(
                    "UPDATE notes SET folder_id = ?1 WHERE folder_id = ?2 AND deleted_at IS NULL",
                    rusqlite::params![keep_folder, id],
                )
                .ok();
                // Soft-delete remaining notes (duplicates that were already moved or are exact copies)
                conn.execute(
                    "UPDATE notes SET deleted_at = datetime('now') WHERE folder_id = ?1 AND deleted_at IS NULL",
                    rusqlite::params![id],
                ).ok();
                // Delete the duplicate folder
                conn.execute("DELETE FROM projects WHERE id = ?1", rusqlite::params![id])
                    .ok();
                eprintln!(
                    "[DEDUP] Removed duplicate folder '{}' (kept {})",
                    folder_name, keep_folder
                );
            }
        }
    }

    // 5. Dedup notes with same note_type within each agent folder
    let folders: Vec<(String, String)> =
        match conn.prepare("SELECT id, name FROM projects WHERE parent_id = ?1") {
            Ok(mut stmt) => stmt
                .query_map(rusqlite::params![pane_system_id], |row| {
                    Ok((row.get(0)?, row.get(1)?))
                })
                .unwrap()
                .filter_map(|r| r.ok())
                .collect(),
            Err(e) => {
                eprintln!("[DEDUP] Failed to prepare agent-folder query for pane_system_id={}: {}", pane_system_id, e);
                Vec::new()
            }
        };

    let note_types = ["soul", "memory", "identity", "user", "tools", "rules"];
    for (folder_id, folder_name) in &folders {
        for nt in &note_types {
            let note_ids: Vec<String> = match conn.prepare(
                "SELECT id FROM notes WHERE folder_id = ?1 AND note_type = ?2 AND deleted_at IS NULL ORDER BY updated_at DESC"
            ) {
                Ok(mut stmt) => stmt.query_map(rusqlite::params![folder_id, nt], |row| row.get(0))
                    .unwrap().filter_map(|r| r.ok()).collect(),
                Err(e) => {
                    eprintln!("[DEDUP] Failed to prepare note query for folder_id={} note_type={}: {}", folder_id, nt, e);
                    Vec::new()
                }
            };

            if note_ids.len() > 1 {
                // Keep first (most recently updated), soft-delete rest
                for id in &note_ids[1..] {
                    conn.execute(
                        "UPDATE notes SET deleted_at = datetime('now') WHERE id = ?1",
                        rusqlite::params![id],
                    )
                    .ok();
                    eprintln!("[DEDUP] Removed duplicate {} note in {}", nt, folder_name);
                }
            }
        }
    }

    eprintln!("[DEDUP] Post-sync dedup complete");
}


/// §24 Integrity Check — Verify ai_agents + filesystem structure
/// Recreates agent records from filesystem if DB records are missing
/// Internal function for testing: verify_agents_integrity with configurable root path
pub fn verify_agents_integrity_with_path(
    conn: &rusqlite::Connection,
    pane_root: std::path::PathBuf,
) {
    use crate::watcher::IMPORT_ACTIVE;
    use std::sync::atomic::Ordering;

    IMPORT_ACTIVE.store(true, Ordering::SeqCst);
    eprintln!("[INTEGRITY §24] Starting agent integrity check");

    let system_dir = pane_root.join(".pane").join("system");
    let pane_fs = match PaneFs::from_pane_root(&pane_root) {
        Ok(pane_fs) => pane_fs,
        Err(e) => {
            eprintln!(
                "[INTEGRITY §24] Filesystem policy initialization failed: {}",
                e
            );
            IMPORT_ACTIVE.store(false, Ordering::SeqCst);
            return;
        }
    };
    let internal_dir = pane_root.join(".pane");
    for path in [&internal_dir, &system_dir] {
        match std::fs::symlink_metadata(path) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                IMPORT_ACTIVE.store(false, Ordering::SeqCst);
                return;
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => break,
            Err(_) => {
                IMPORT_ACTIVE.store(false, Ordering::SeqCst);
                return;
            }
        }
    }

    // Clean up legacy tasks/ directory (Bug #288 fix)
    let tasks_dir = pane_root.join(".pane").join("tasks");
    if tasks_dir.exists() {
        if pane_fs
            .remove_dir_all(PaneRoot::Internal, std::path::Path::new("tasks"))
            .is_ok()
        {
            eprintln!("[INTEGRITY §24] Removed legacy .pane/tasks/ directory");
        }
    }

    if !system_dir.exists() {
        eprintln!("[INTEGRITY §24] No .pane/system directory found, skipping");
        IMPORT_ACTIVE.store(false, Ordering::SeqCst);
        return;
    }

    // 1. Get all agents from database.
    // NOTE: this list is also used below by the filesystem-verification loop, which
    // must still cover every active agent (including imported subagents) — filtering
    // this query would silently skip subagent directory checks as a side effect.
    // The Bug 1b fix (subagent-exclusion filter) is instead applied to a separate,
    // narrower query used only for safety-net default-promotion below.
    let mut stmt = match conn
        .prepare("SELECT id, name, workspace_path, is_default FROM ai_agents WHERE is_active = 1")
    {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[INTEGRITY §24] Failed to query agents: {}", e);
            IMPORT_ACTIVE.store(false, Ordering::SeqCst);
            return;
        }
    };

    let agents: Vec<(String, String, String, bool)> = match stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, i32>(3)? != 0,
        ))
    }) {
        Ok(rows) => rows.filter_map(|r| r.ok()).collect(),
        Err(e) => {
            eprintln!("[INTEGRITY §24] Failed to collect agents: {}", e);
            IMPORT_ACTIVE.store(false, Ordering::SeqCst);
            return;
        }
    };

    eprintln!(
        "[INTEGRITY §24] Found {} agent(s) in database",
        agents.len()
    );

    // 1b. Safety net: if no agent is default, promote the first eligible one.
    //
    // Bug 1b fix (#2777 venice live-test scoping / B5 #2567): the prior version of
    // this block selected from the unordered `agents` list above (no role filter,
    // no ORDER BY) and force-overwrote `role = 'main'` on whatever row SQLite
    // happened to return first — which could be an imported OC subagent
    // (role = 'sub' AND runtime_ref IS NOT NULL), silently relabeling it as main.
    // This is a regression relative to the two other promotion sites in this
    // codebase that already get it right: the openclaw disconnect-arm promotion
    // (commands.rs, `ai_disconnect_runtime_impl`) and `set_default_agent_impl`
    // (agents.rs, the #B5/#2567 fix). Both use the compound "is this an imported
    // subagent" predicate — role = 'sub' AND runtime_ref IS NOT NULL — NOT a
    // nonexistent `is_subagent` column. Locally-created role='sub' agents
    // (runtime_ref IS NULL) remain eligible for promotion.
    //
    // Query separately (rather than filtering `agents` above) so the
    // filesystem-verification loop below still covers every active agent,
    // subagents included.
    if !agents.iter().any(|(_, _, _, is_default)| *is_default) {
        let promote_target: Option<String> = conn
            .query_row(
                "SELECT id FROM ai_agents WHERE is_active = 1 \
                 AND NOT (role = 'sub' AND runtime_ref IS NOT NULL) \
                 ORDER BY created_at DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .optional()
            .unwrap_or(None);

        if let Some(ref first_id) = promote_target {
            eprintln!(
                "[INTEGRITY §24] No default agent found — promoting {} (role preserved)",
                first_id
            );
            // Preserve the existing role — do NOT force-overwrite to 'main'. A
            // legitimately-eligible locally-created role='sub' agent keeps its role;
            // only is_default changes.
            conn.execute(
                "UPDATE ai_agents SET is_default = 1 WHERE id = ?1",
                rusqlite::params![first_id],
            )
            .ok();
        } else {
            eprintln!("[INTEGRITY §24] No eligible agent to promote as default — left unset");
        }
    }

    // 2. Verify each agent's filesystem structure
    for (agent_id, name, workspace_path, is_default) in &agents {
        if PaneFs::validate_name(workspace_path).is_err() {
            continue;
        }
        if pane_fs
            .create_dir_all(PaneRoot::System, std::path::Path::new(workspace_path))
            .is_err()
        {
            continue;
        }
        for (filename, content) in [
            ("SOUL.md", format!("# SOUL.md\n\n**Agent:** {}\n\n*(This file was auto-created by integrity check. Please update with agent personality and directives.)*\n", name)),
            ("MEMORY.md", format!("# MEMORY.md\n\n**Agent:** {}\n\n*(This file was auto-created by integrity check. Agent memories will be written here.)*\n", name)),
        ] {
            let relative = PaneFs::relative_from_names([workspace_path.as_str(), filename]).unwrap();
            if let Ok(destination) = pane_fs.authorize_write(PaneRoot::System, &relative) {
                if !destination.as_path().exists() {
                    let _ = atomic_write(destination.as_path(), &content);
                }
            }
        }
        let identity_relative =
            PaneFs::relative_from_names([workspace_path.as_str(), "IDENTITY.md"]).unwrap();
        let identity_path = match pane_fs.authorize_write(PaneRoot::System, &identity_relative) {
            Ok(path) => path,
            Err(_) => continue,
        };
        let needs_identity_write = if identity_path.as_path().exists() {
            std::fs::read_to_string(identity_path.as_path())
                .map(|content| {
                    content.trim().is_empty() || content.contains("auto-created by integrity check")
                })
                .unwrap_or(true)
        } else {
            true
        };
        if needs_identity_write {
            let content = format!("# {}\n\nYour name is {}.\n", name, name);
            let _ = atomic_write(identity_path.as_path(), &content);
        } else if let Some(file_name) = parse_identity_name(identity_path.as_path()) {
            if file_name != *name {
                if let Ok(old_content) = std::fs::read_to_string(identity_path.as_path()) {
                    let new_content = repair_identity_name(&old_content, name);
                    let _ = atomic_write(identity_path.as_path(), &new_content);
                }
            }
        }
        if *is_default {
            let logs_relative =
                PaneFs::relative_from_names([workspace_path.as_str(), "logs"]).unwrap();
            let _ = pane_fs.create_dir_all(PaneRoot::System, &logs_relative);
        }
        let _ = agent_id;
    }

    // 3. Scan filesystem for agents not in database
    if let Ok(entries) = std::fs::read_dir(&system_dir) {
        for entry in entries.filter_map(|e| e.ok()) {
            if !entry
                .file_type()
                .map(|ft| !ft.is_symlink() && ft.is_dir())
                .unwrap_or(false)
            {
                continue;
            }

            let folder_name = entry.file_name().to_string_lossy().to_string();

            // Skip non-agent directories
            if folder_name.starts_with('.') || folder_name == "logs" || folder_name == "memory" {
                continue;
            }

            // §24.2 invariant: workspace_path must be a 32-char hex UUID.
            // Non-UUID folders are not Pane agent dirs — skip and warn rather than inserting a bad record.
            let is_uuid_format =
                folder_name.len() == 32 && folder_name.chars().all(|c| c.is_ascii_hexdigit());
            if !is_uuid_format {
                eprintln!(
                    "[INTEGRITY §24] Skipping non-UUID directory '{}' — not a valid agent workspace_path (§24.2)",
                    folder_name
                );
                continue;
            }
            if PaneFs::validate_name(&folder_name).is_err()
                || pane_fs
                    .authorize_existing(
                        PaneRoot::System,
                        std::path::Path::new(&folder_name),
                        ExistingKind::Directory,
                    )
                    .is_err()
            {
                continue;
            }

            // Check if this agent exists in database (workspace_path is now the UUID/folder name)
            let exists: bool = conn
                .query_row(
                    "SELECT COUNT(*) > 0 FROM ai_agents WHERE workspace_path = ?1",
                    params![&folder_name],
                    |row| row.get(0),
                )
                .unwrap_or(false);

            if !exists {
                eprintln!(
                    "[INTEGRITY §24] Found agent directory not in database: {}",
                    folder_name
                );

                // Check if SOUL.md exists to confirm this is an agent directory
                let soul_relative =
                    PaneFs::relative_from_names([folder_name.as_str(), "SOUL.md"]).unwrap();
                if let Ok(soul_path) =
                    pane_fs.authorize_existing(PaneRoot::System, &soul_relative, ExistingKind::File)
                {
                    // SOUL.md present — try to read agent name and register in DB
                    // Try to read agent name from SOUL.md or IDENTITY.md
                    let agent_name =
                        if let Ok(soul_content) = std::fs::read_to_string(soul_path.as_path()) {
                            // Try to extract name from SOUL.md (look for "# Agent:" or similar)
                            soul_content
                                .lines()
                                .find(|line| {
                                    line.starts_with("**Agent:**") || line.starts_with("# Agent:")
                                })
                                .and_then(|line| line.split(':').nth(1))
                                .map(|name| name.trim().to_string())
                                .unwrap_or_else(|| folder_name.clone())
                        } else {
                            folder_name.clone()
                        };

                    let slug = agent_name.to_lowercase().replace(" ", "-");
                    let agent_id = folder_name.clone(); // Use folder name (UUID) as the DB id
                    let now = chrono::Utc::now().to_rfc3339();

                    // Check if this should be the default agent (has logs/ directory)
                    let logs_relative =
                        PaneFs::relative_from_names([folder_name.as_str(), "logs"]).unwrap();
                    let is_default = pane_fs
                        .authorize_existing(
                            PaneRoot::System,
                            &logs_relative,
                            ExistingKind::Directory,
                        )
                        .is_ok();

                    // Filesystem-recovered agents have no OC linkage (runtime_ref = NULL) and
                    // no creation-time provider-selection context to read — resolve the same
                    // way any other NULL-runtime_ref local row is resolved (ADR-0063 D1 backend
                    // enforcement: non-NULL runtime_type on every write path).
                    let runtime_type = crate::agents::resolve_runtime_type_for_new_local_agent(&conn);

                    // #2567 B6: this INSERT never sets runtime_ref (filesystem-recovered
                    // agents have no OC linkage), so a recovered non-default agent is
                    // definitionally local, never a subagent — role='local', not 'sub'.
                    match conn.execute(
                        "INSERT INTO ai_agents (id, name, slug, role, is_default, workspace_path, runtime_type, source, created_at, updated_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'recovered', ?8, ?9)",
                        params![
                            agent_id,
                            agent_name,
                            slug,
                            if is_default { "main" } else { "local" },
                            if is_default { 1 } else { 0 },
                            &folder_name,  // workspace_path = UUID/folder name
                            &runtime_type,
                            &now,
                            &now
                        ],
                    ) {
                        Ok(_) => eprintln!("[INTEGRITY §24] Created agent record from filesystem: {} ({})", agent_name, folder_name),
                        Err(e) => eprintln!("[INTEGRITY §24] Failed to create agent record: {}", e),
                    }
                } else {
                    // No SOUL.md — this may be an empty orphan UUID dir left by a crashed
                    // create_local_agent. Delete it only if policy authorization succeeds and
                    // the directory is truly empty (Item 4 fix).
                    let agent_relative = std::path::Path::new(&folder_name);
                    let is_empty = pane_fs
                        .authorize_existing(
                            PaneRoot::System,
                            agent_relative,
                            ExistingKind::Directory,
                        )
                        .ok()
                        .and_then(|agent_path| std::fs::read_dir(agent_path.as_path()).ok())
                        .map(|mut entries| entries.next().is_none())
                        .unwrap_or(false);
                    if is_empty {
                        eprintln!(
                            "[INTEGRITY §24] Removing empty orphan UUID dir: {}",
                            folder_name
                        );
                        let _ = pane_fs.remove_dir_all(PaneRoot::System, agent_relative);
                    }
                }
            }
        }
    }

    // 4. Reconstitute daily logs from flat files
    // Re-query agents to include any newly discovered ones from Step 3
    let mut stmt = match conn
        .prepare("SELECT id, name, workspace_path, is_default FROM ai_agents WHERE is_active = 1")
    {
        Ok(s) => s,
        Err(e) => {
            eprintln!(
                "[INTEGRITY §24] Failed to prepare agent query for log reconstitution: {}",
                e
            );
            eprintln!("[INTEGRITY §24] Agent integrity check complete");
            IMPORT_ACTIVE.store(false, Ordering::SeqCst);
            return;
        }
    };

    let agents: Vec<(String, String, String, bool)> = match stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, i32>(3)? != 0,
        ))
    }) {
        Ok(rows) => rows.filter_map(|r| r.ok()).collect(),
        Err(e) => {
            eprintln!(
                "[INTEGRITY §24] Failed to query agents for log reconstitution: {}",
                e
            );
            eprintln!("[INTEGRITY §24] Agent integrity check complete");
            IMPORT_ACTIVE.store(false, Ordering::SeqCst);
            return;
        }
    };

    eprintln!(
        "[INTEGRITY §24] Reconstituting daily logs for {} agent(s)",
        agents.len()
    );

    for (agent_id, name, workspace_path, _is_default) in &agents {
        if PaneFs::validate_name(workspace_path).is_err() {
            continue;
        }
        let logs_relative = PaneFs::relative_from_names([workspace_path.as_str(), "logs"]).unwrap();
        let logs_dir = match pane_fs.authorize_existing(
            PaneRoot::System,
            &logs_relative,
            ExistingKind::Directory,
        ) {
            Ok(path) => path,
            Err(_) => continue,
        };

        if let Ok(entries) = std::fs::read_dir(logs_dir.as_path()) {
            for entry in entries.filter_map(|e| e.ok()) {
                let filename = entry.file_name().to_string_lossy().to_string();

                // Only process YYYY-MM-DD.md files
                if !filename.ends_with(".md") || filename.len() != 13 {
                    continue;
                }

                let date_str = &filename[..10]; // "YYYY-MM-DD"

                // Validate it's a real date format
                if chrono::NaiveDate::parse_from_str(date_str, "%Y-%m-%d").is_err() {
                    continue;
                }

                let file_path = format!(".pane/system/{}/logs/{}", workspace_path, filename);

                // Check if this log already exists in notes table
                let exists: bool = conn.query_row(
                    "SELECT COUNT(*) > 0 FROM notes WHERE file_path = ?1 AND deleted_at IS NULL",
                    params![&file_path],
                    |row| row.get(0),
                ).unwrap_or(false);

                if exists {
                    continue;
                }

                // Also check by title + agent_id (alternative match)
                let exists_by_title: bool = conn.query_row(
                    "SELECT COUNT(*) > 0 FROM notes WHERE title = ?1 AND agent_id = ?2 AND note_type = 'daily' AND deleted_at IS NULL",
                    params![date_str, agent_id],
                    |row| row.get(0),
                ).unwrap_or(false);

                if exists_by_title {
                    continue;
                }

                // Read only a policy-authorized regular file.
                if PaneFs::validate_name(&filename).is_err() {
                    continue;
                }
                let log_relative = PaneFs::relative_from_names([
                    workspace_path.as_str(),
                    "logs",
                    filename.as_str(),
                ])
                .unwrap();
                let full_path = match pane_fs.authorize_existing(
                    PaneRoot::System,
                    &log_relative,
                    ExistingKind::File,
                ) {
                    Ok(path) => path,
                    Err(_) => continue,
                };
                let content = match std::fs::read_to_string(full_path.as_path()) {
                    Ok(c) => c,
                    Err(e) => {
                        eprintln!(
                            "[INTEGRITY §24] Failed to read log file {}: {}",
                            file_path, e
                        );
                        continue;
                    }
                };

                // Generate content preview (first 200 chars)
                let content_preview = if content.len() > 200 {
                    format!("{}...", content.chars().take(200).collect::<String>())
                } else {
                    content.clone()
                };

                // Get file metadata for timestamps
                let metadata = std::fs::metadata(full_path.as_path()).ok();
                let created = metadata
                    .as_ref()
                    .and_then(|m| m.created().ok())
                    .map(|t| chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339())
                    .unwrap_or_else(|| chrono::Utc::now().to_rfc3339());
                let modified = metadata
                    .as_ref()
                    .and_then(|m| m.modified().ok())
                    .map(|t| chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339())
                    .unwrap_or_else(|| chrono::Utc::now().to_rfc3339());

                // Create notes record
                let note_id = uuid::Uuid::new_v4().to_string();

                match conn.execute(
                    "INSERT INTO notes (id, title, content_preview, file_path, note_type, agent_id, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, 'daily', ?5, ?6, ?7)",
                    params![
                        note_id,
                        date_str,
                        content_preview,
                        file_path,
                        agent_id,
                        created,
                        modified,
                    ],
                ) {
                    Ok(_) => {
                        eprintln!("[INTEGRITY §24] Reconstituted daily log: {} for agent {}", date_str, name);

                        // FTS insert happens automatically via trigger (§486)
                    }
                    Err(e) => eprintln!("[INTEGRITY §24] Failed to reconstitute log {}: {}", date_str, e),
                }
            }
        }
    }

    eprintln!("[INTEGRITY §24] Agent integrity check complete");
    IMPORT_ACTIVE.store(false, Ordering::SeqCst);
}


/// Parse the agent name from an IDENTITY.md file.
/// Accepts `# {name}` (first heading) or `Your name is {name}.` line.
pub(crate) fn parse_identity_name(path: &std::path::Path) -> Option<String> {
    let content = std::fs::read_to_string(path).ok()?;
    for line in content.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("# ") {
            let name = rest.trim().to_string();
            if !name.is_empty() {
                return Some(name);
            }
        }
        if let Some(rest) = line.strip_prefix("Your name is ") {
            let name = rest.trim_end_matches('.').trim().to_string();
            if !name.is_empty() {
                return Some(name);
            }
        }
    }
    None
}


/// Rewrite the first `# Heading` line in an IDENTITY.md to use `new_name`.
/// Falls back to prepending a new heading if none is found.
fn repair_identity_name(content: &str, new_name: &str) -> String {
    let mut replaced = false;
    let mut lines: Vec<String> = content
        .lines()
        .map(|line| {
            if !replaced && line.trim().starts_with("# ") {
                replaced = true;
                format!("# {}", new_name)
            } else {
                line.to_string()
            }
        })
        .collect();
    if !replaced {
        lines.insert(0, format!("# {}", new_name));
    }
    lines.join("\n") + "\n"
}


/// Public wrapper: verify_agents_integrity using default Pane root path
pub fn verify_agents_integrity(conn: &rusqlite::Connection) {
    let pane_root = crate::database::get_pane_root();
    verify_agents_integrity_with_path(conn, pane_root);
}


/// Tauri command to trigger the integrity check from the frontend (e.g., during onboarding).
#[tauri::command]
pub fn run_integrity_check(db: tauri::State<'_, DbState>) -> Result<String, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;

    // Check if ai_agents table exists (post-migration)
    let has_agents_table: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='ai_agents'",
            [],
            |row| row.get(0),
        )
        .unwrap_or(false);

    if has_agents_table {
        verify_agents_integrity(&conn);

        // Trigger embedding computation for newly reconstituted notes (Bug #390 fix)
        // Drop the connection lock before spawning thread
        drop(conn);

        // Spawn in background thread to avoid blocking the response
        // Use a fresh DB connection so we don't hold the shared mutex during slow ONNX inference (#1306)
        std::thread::spawn(move || {
            eprintln!("[INTEGRITY §24] Starting background embedding computation...");

            let db_path = crate::database::get_db_path();
            let conn = match rusqlite::Connection::open(&db_path) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("[INTEGRITY-ERROR] Failed to open DB for embeddings: {}", e);
                    return;
                }
            };

            // Check if embeddings are enabled
            let embeddings_enabled: Option<String> =
                crate::database::get_ai_setting(&conn, "embeddings_enabled");
            if embeddings_enabled.as_deref() != Some("true") {
                eprintln!("[INTEGRITY §24] Embeddings disabled, skipping background computation");
                return;
            }

            // Read engine + credentials; skip if not configured
            let embeddings_engine =
                crate::database::get_ai_setting(&conn, "embeddings_engine").unwrap_or_default();
            let embeddings_model =
                crate::database::get_ai_setting(&conn, "embeddings_model").unwrap_or_default();

            if embeddings_engine.is_empty() || embeddings_model.is_empty() {
                eprintln!(
                    "[INTEGRITY §24] Embeddings engine/model not configured — skipping pipeline"
                );
                return;
            }

            let embed_fn_result =
                crate::build_http_embed_fn(&conn, &embeddings_engine, &embeddings_model);
            let (embed_fn, engine_tag) = match embed_fn_result {
                Ok(pair) => pair,
                Err(e) => {
                    eprintln!("[INTEGRITY §24] Failed to build embed_fn: {}", e);
                    return;
                }
            };

            // #2776: read the persisted token limit (fallback 512 if the install
            // configured its engine/model before this setting existed).
            let max_tokens = crate::embeddings::read_embeddings_max_tokens(&conn);

            eprintln!(
                "[INTEGRITY §24] Running embedding pipeline: engine={} model={} max_tokens={}",
                embeddings_engine, embeddings_model, max_tokens
            );

            let pane_root = crate::database::get_pane_root();
            match crate::embeddings::run_embedding_pipeline(
                &conn,
                &pane_root,
                &embed_fn,
                &engine_tag,
                &embeddings_model,
                max_tokens,
                |_| {},
            ) {
                Ok((notes, facts, chats, failed_count)) => {
                    eprintln!("[INTEGRITY §24] Embedding pipeline complete: {} notes, {} facts, {} chats, {} failed", notes, facts, chats, failed_count);
                    let terminal_stage = if failed_count > 0 { "partial" } else { "ready" };
                    let _ =
                        crate::database::set_ai_setting(&conn, "embeddings_status", terminal_stage);
                    let _ = crate::database::set_ai_setting(
                        &conn,
                        "embeddings_failed_count",
                        &failed_count.to_string(),
                    );
                }
                Err(e) => {
                    eprintln!("[INTEGRITY §24] Embedding pipeline error: {}", e);
                    let _ = crate::database::set_ai_setting(&conn, "embeddings_status", "error");
                    // Do not write embeddings_failed_count on auth abort — leave prior state intact
                }
            }
        });
    }
    // legacy system notes function removed — no pre-§24 databases exist

    Ok("Integrity check complete".to_string())
}


/// Returned by `check_identity_divergence` for each divergent agent.
#[derive(Debug, Serialize)]
pub struct IdentityDivergence {
    pub agent_id: String,
    pub db_name: String,
    pub file_name: String,
}


/// Tauri command: scan all active agents and return any whose IDENTITY.md name
/// does not match ai_agents.name. Auto-repairs divergences (DB is authoritative).
#[tauri::command]
pub fn check_identity_divergence(
    db: tauri::State<'_, DbState>,
) -> Result<Vec<IdentityDivergence>, String> {
    eprintln!("[CHECK_IDENTITY_DIVERGENCE] called");
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let pane_fs = PaneFs::for_runtime().map_err(|e| policy_error("Filesystem policy", e))?;

    let mut stmt = conn
        .prepare("SELECT id, name, workspace_path FROM ai_agents WHERE is_active = 1")
        .map_err(|e| e.to_string())?;

    let agents: Vec<(String, String, String)> = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();

    let mut divergences = Vec::new();
    for (agent_id, db_name, workspace_path) in &agents {
        PaneFs::validate_name(workspace_path)
            .map_err(|e| policy_error("Unsafe agent workspace path in database", e))?;
        let identity_relative =
            PaneFs::relative_from_names([workspace_path.as_str(), "IDENTITY.md"])
                .map_err(|e| policy_error("Unsafe agent identity path", e))?;
        let identity_path = match pane_fs.authorize_existing(
            PaneRoot::System,
            &identity_relative,
            ExistingKind::File,
        ) {
            Ok(path) => path,
            Err(_) => continue,
        };
        if let Some(file_name) = parse_identity_name(identity_path.as_path()) {
            if file_name != *db_name {
                eprintln!(
                    "[INTEGRITY] Agent name divergence: DB=\"{}\" vs IDENTITY.md=\"{}\" for agent {}",
                    db_name, file_name, agent_id
                );
                // Auto-repair
                if let Ok(old_content) = std::fs::read_to_string(identity_path.as_path()) {
                    let new_content = repair_identity_name(&old_content, db_name);
                    if let Err(e) = atomic_write(identity_path.as_path(), &new_content) {
                        eprintln!(
                            "[INTEGRITY] Failed to repair IDENTITY.md for {}: {}",
                            agent_id, e
                        );
                    } else {
                        eprintln!(
                            "[INTEGRITY] Repaired IDENTITY.md for agent {}: set name to \"{}\"",
                            agent_id, db_name
                        );
                    }
                }
                divergences.push(IdentityDivergence {
                    agent_id: agent_id.clone(),
                    db_name: db_name.clone(),
                    file_name,
                });
            }
        }
    }

    Ok(divergences)
}

#[cfg(test)]
mod integrity_check_fts5_tests {
    use super::*;

    #[test]
    fn test_integrity_check_updates_fts5() {
        // Bug #390: Verify that reconstituted daily logs are indexed in FTS5
        use std::fs;
        use tempfile::TempDir;

        // Create temp directory structure
        let temp_dir = TempDir::new().unwrap();
        let pane_root = temp_dir.path().to_path_buf();
        let system_dir = pane_root.join(".pane").join("system");
        fs::create_dir_all(&system_dir).unwrap();

        // Create test database
        let db_path = pane_root.join("test.db");
        let conn = rusqlite::Connection::open(&db_path).unwrap();

        // Create minimal schema for test
        conn.execute_batch(
            "
            CREATE TABLE ai_agents (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                slug TEXT NOT NULL,
                role TEXT NOT NULL DEFAULT 'sub',
                is_default INTEGER NOT NULL DEFAULT 0,
                is_active INTEGER NOT NULL DEFAULT 1,
                workspace_path TEXT NOT NULL,
                runtime_ref TEXT,
                source TEXT NOT NULL DEFAULT 'local',
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE TABLE notes (
                id TEXT PRIMARY KEY,
                title TEXT NOT NULL,
                body TEXT,
                content_preview TEXT,
                file_path TEXT,
                note_type TEXT,
                agent_id TEXT,
                deleted_at TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE VIRTUAL TABLE notes_fts USING fts5(
                title, body, content='notes', content_rowid='rowid', tokenize='porter unicode61'
            );

            -- FTS5 triggers for automatic indexing
            CREATE TRIGGER notes_fts_ai AFTER INSERT ON notes BEGIN
                INSERT INTO notes_fts(rowid, title, body) VALUES (new.rowid, new.title, new.body);
            END;

            CREATE TRIGGER notes_fts_ad AFTER DELETE ON notes BEGIN
                INSERT INTO notes_fts(notes_fts, rowid, title, body) VALUES('delete', old.rowid, old.title, old.body);
            END;

            CREATE TRIGGER notes_fts_au AFTER UPDATE ON notes BEGIN
                INSERT INTO notes_fts(notes_fts, rowid, title, body) VALUES('delete', old.rowid, old.title, old.body);
                INSERT INTO notes_fts(rowid, title, body) VALUES (new.rowid, new.title, new.body);
            END;
            "
        ).unwrap();

        // Create test agent with workspace directory
        let agent_id = uuid::Uuid::new_v4().to_string();
        let workspace_path = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO ai_agents (id, name, slug, role, is_default, workspace_path, source, created_at, updated_at)
             VALUES (?1, 'Test Agent', 'test-agent', 'main', 1, ?2, 'test', datetime('now'), datetime('now'))",
            rusqlite::params![&agent_id, &workspace_path],
        ).unwrap();

        // Create agent directory and logs folder
        let agent_dir = system_dir.join(&workspace_path);
        let logs_dir = agent_dir.join("logs");
        fs::create_dir_all(&logs_dir).unwrap();

        // Create a daily log file
        let log_content = "# Test Log\n\nThis is a test daily log entry.";
        fs::write(logs_dir.join("2026-04-07.md"), log_content).unwrap();

        // Run integrity check
        verify_agents_integrity_with_path(&conn, pane_root.clone());

        // Verify note was created in notes table
        let note_exists: bool = conn.query_row(
            "SELECT COUNT(*) > 0 FROM notes WHERE agent_id = ?1 AND title = '2026-04-07' AND note_type = 'daily'",
            rusqlite::params![&agent_id],
            |row| row.get(0),
        ).unwrap();
        assert!(
            note_exists,
            "Daily log should be reconstituted in notes table"
        );

        // Verify FTS5 index was updated (Bug #390 fix)
        // Daily logs are system notes (note_type='daily') with body=NULL,
        // so only the title is indexed in FTS5, not body content.
        // Search by the date in the title ("2026-04-07" contains "2026")
        let fts_exists: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM notes_fts WHERE notes_fts MATCH '2026'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            fts_exists,
            "FTS5 index should be updated for reconstituted daily log (title indexed)"
        );

        // Verify search works by title
        let search_result: Option<String> = conn
            .query_row(
                "SELECT n.title FROM notes n
             JOIN notes_fts fts ON fts.rowid = n.rowid
             WHERE notes_fts MATCH '2026' AND n.deleted_at IS NULL",
                [],
                |row| row.get(0),
            )
            .ok();
        assert_eq!(
            search_result,
            Some("2026-04-07".to_string()),
            "Should be able to search for reconstituted note by title"
        );
    }

    /// Bug 1b (#2777 venice live-test scoping / B5 #2567): verify_agents_integrity_with_path's
    /// safety-net default-promotion block (§24, "1b. Safety net") must never promote an
    /// imported OC subagent (role = 'sub' AND runtime_ref IS NOT NULL), must pick the
    /// eligible agent with the latest created_at deterministically, and must NOT
    /// overwrite the promoted agent's existing role column.
    #[test]
    fn test_integrity_safety_net_excludes_subagents_and_preserves_role() {
        use std::fs;
        use tempfile::TempDir;

        // Create temp directory structure
        let temp_dir = TempDir::new().unwrap();
        let pane_root = temp_dir.path().to_path_buf();
        let system_dir = pane_root.join(".pane").join("system");
        fs::create_dir_all(&system_dir).unwrap();

        // Create test database
        let db_path = pane_root.join("test.db");
        let conn = rusqlite::Connection::open(&db_path).unwrap();

        // Minimal schema matching production's ai_agents columns used by the
        // safety-net query (role, runtime_ref, is_active, created_at) plus the
        // columns the rest of verify_agents_integrity_with_path touches.
        conn.execute_batch(
            "
            CREATE TABLE ai_agents (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                slug TEXT NOT NULL,
                role TEXT NOT NULL DEFAULT 'sub',
                is_default INTEGER NOT NULL DEFAULT 0,
                is_active INTEGER NOT NULL DEFAULT 1,
                workspace_path TEXT NOT NULL,
                runtime_ref TEXT,
                source TEXT NOT NULL DEFAULT 'local',
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );
            "
        ).unwrap();

        // No agent is default. Two non-sub agents (eligible) with different
        // created_at, plus a role='sub' + runtime_ref IS NOT NULL subagent that
        // is the ONLY active agent with a later created_at than either eligible
        // agent — the exact scenario the exclusion filter guards against.
        let older_id = uuid::Uuid::new_v4().to_string().replace('-', "");
        let newer_id = uuid::Uuid::new_v4().to_string().replace('-', "");
        let sub_id = uuid::Uuid::new_v4().to_string().replace('-', "");

        conn.execute(
            "INSERT INTO ai_agents (id, name, slug, role, is_default, is_active, workspace_path, runtime_ref, created_at, updated_at)
             VALUES (?1, 'Older Local', 'older-local', 'local', 0, 1, ?1, NULL, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
            rusqlite::params![&older_id],
        ).unwrap();
        conn.execute(
            "INSERT INTO ai_agents (id, name, slug, role, is_default, is_active, workspace_path, runtime_ref, created_at, updated_at)
             VALUES (?1, 'Newer Local', 'newer-local', 'sub', 0, 1, ?1, NULL, '2026-01-02T00:00:00Z', '2026-01-02T00:00:00Z')",
            rusqlite::params![&newer_id],
        ).unwrap();
        // Imported OC subagent: role='sub' AND runtime_ref IS NOT NULL, latest created_at
        // of the three — if the exclusion filter were missing, this row would win the
        // ORDER BY created_at DESC LIMIT 1 and get force-promoted.
        conn.execute(
            "INSERT INTO ai_agents (id, name, slug, role, is_default, is_active, workspace_path, runtime_ref, created_at, updated_at)
             VALUES (?1, 'Imported Sub', 'imported-sub', 'sub', 0, 1, ?1, 'oc-runtime-ref-123', '2026-01-03T00:00:00Z', '2026-01-03T00:00:00Z')",
            rusqlite::params![&sub_id],
        ).unwrap();

        verify_agents_integrity_with_path(&conn, pane_root.clone());

        // 1. Never promotes the subagent.
        let sub_is_default: i64 = conn
            .query_row(
                "SELECT is_default FROM ai_agents WHERE id = ?1",
                rusqlite::params![&sub_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            sub_is_default, 0,
            "safety net must NEVER promote an imported OC subagent (role='sub' AND runtime_ref IS NOT NULL), even though it has the latest created_at"
        );

        // 2. Picks deterministically by created_at DESC among eligible (non-sub) agents:
        // 'Newer Local' (role='sub' but runtime_ref IS NULL, so eligible) must win over
        // 'Older Local'.
        let newer_is_default: i64 = conn
            .query_row(
                "SELECT is_default FROM ai_agents WHERE id = ?1",
                rusqlite::params![&newer_id],
                |row| row.get(0),
            )
            .unwrap();
        let older_is_default: i64 = conn
            .query_row(
                "SELECT is_default FROM ai_agents WHERE id = ?1",
                rusqlite::params![&older_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            newer_is_default, 1,
            "the eligible agent with the latest created_at ('Newer Local') must be promoted"
        );
        assert_eq!(
            older_is_default, 0,
            "the older eligible agent must NOT be promoted when a newer eligible agent exists"
        );

        // 3. Preserves role — 'Newer Local' had role='sub' (locally-created, runtime_ref
        // IS NULL) before promotion; the safety net must not force-overwrite it to 'main'.
        let newer_role: String = conn
            .query_row(
                "SELECT role FROM ai_agents WHERE id = ?1",
                rusqlite::params![&newer_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            newer_role, "sub",
            "promoted agent's role column must be preserved, not overwritten to 'main'"
        );

        // Exactly one agent should hold is_default = 1 across the whole table.
        let default_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM ai_agents WHERE is_default = 1", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            default_count, 1,
            "exactly one agent must be promoted to default by the safety net"
        );
    }
}

#[cfg(test)]
mod identity_divergence_and_repair_tests {
    use super::*;

    // === Identity Divergence Tests (#1281, ADR-0039 Decision 7) ===

    pub(super) fn make_agent_schema(conn: &rusqlite::Connection) {
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS ai_agents (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                slug TEXT NOT NULL,
                role TEXT NOT NULL DEFAULT 'sub',
                is_default INTEGER NOT NULL DEFAULT 0,
                is_active INTEGER NOT NULL DEFAULT 1,
                workspace_path TEXT NOT NULL,
                runtime_ref TEXT,
                runtime_type TEXT,
                source TEXT NOT NULL DEFAULT 'local',
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS ai_settings (
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS notes (
                id TEXT PRIMARY KEY,
                title TEXT NOT NULL,
                body TEXT,
                content_preview TEXT,
                file_path TEXT,
                note_type TEXT,
                agent_id TEXT,
                deleted_at TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );
            CREATE VIRTUAL TABLE IF NOT EXISTS notes_fts USING fts5(
                title, body, content='notes', content_rowid='rowid', tokenize='porter unicode61'
            );
        ",
        )
        .unwrap();
    }

    #[test]
    fn test_identity_divergence_detected_and_repaired() {
        // DB name "Alice", IDENTITY.md says "# Bob" → divergence detected, IDENTITY.md updated to Alice
        use std::fs;
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let pane_root = temp_dir.path().to_path_buf();
        let workspace_path = uuid::Uuid::new_v4().to_string();
        let agent_dir = pane_root.join(".pane").join("system").join(&workspace_path);
        fs::create_dir_all(&agent_dir).unwrap();

        // Write IDENTITY.md with wrong name
        fs::write(
            agent_dir.join("IDENTITY.md"),
            "# Bob\n\nYour name is Bob.\n",
        )
        .unwrap();

        let conn = rusqlite::Connection::open_in_memory().unwrap();
        make_agent_schema(&conn);
        let agent_id = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO ai_agents (id, name, slug, role, is_default, workspace_path, source, created_at, updated_at)
             VALUES (?1, 'Alice', 'alice', 'main', 1, ?2, 'test', datetime('now'), datetime('now'))",
            rusqlite::params![&agent_id, &workspace_path],
        ).unwrap();

        verify_agents_integrity_with_path(&conn, pane_root.clone());

        let identity_content = fs::read_to_string(agent_dir.join("IDENTITY.md")).unwrap();
        assert!(
            identity_content.contains("# Alice"),
            "IDENTITY.md should have been repaired to \"# Alice\", got: {}",
            identity_content
        );
        assert!(
            !identity_content.contains("# Bob"),
            "IDENTITY.md should no longer contain \"# Bob\", got: {}",
            identity_content
        );
    }

    #[test]
    fn test_identity_no_divergence_file_unchanged() {
        // DB name "Alice", IDENTITY.md says "# Alice" → no divergence, file unchanged
        use std::fs;
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let pane_root = temp_dir.path().to_path_buf();
        let workspace_path = uuid::Uuid::new_v4().to_string();
        let agent_dir = pane_root.join(".pane").join("system").join(&workspace_path);
        fs::create_dir_all(&agent_dir).unwrap();

        let original = "# Alice\n\nSome notes here.\n";
        fs::write(agent_dir.join("IDENTITY.md"), original).unwrap();

        let conn = rusqlite::Connection::open_in_memory().unwrap();
        make_agent_schema(&conn);
        let agent_id = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO ai_agents (id, name, slug, role, is_default, workspace_path, source, created_at, updated_at)
             VALUES (?1, 'Alice', 'alice', 'main', 1, ?2, 'test', datetime('now'), datetime('now'))",
            rusqlite::params![&agent_id, &workspace_path],
        ).unwrap();

        verify_agents_integrity_with_path(&conn, pane_root.clone());

        let identity_content = fs::read_to_string(agent_dir.join("IDENTITY.md")).unwrap();
        // File was already correct — body content must be preserved
        assert!(
            identity_content.contains("Some notes here."),
            "File should be unchanged when no divergence, got: {}",
            identity_content
        );
    }

    #[test]
    fn test_identity_missing_creates_stub_no_divergence() {
        // DB name "Alice", IDENTITY.md missing → creates stub, no divergence after
        use std::fs;
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let pane_root = temp_dir.path().to_path_buf();
        let workspace_path = uuid::Uuid::new_v4().to_string();
        let agent_dir = pane_root.join(".pane").join("system").join(&workspace_path);
        fs::create_dir_all(&agent_dir).unwrap();
        // No IDENTITY.md written

        let conn = rusqlite::Connection::open_in_memory().unwrap();
        make_agent_schema(&conn);
        let agent_id = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO ai_agents (id, name, slug, role, is_default, workspace_path, source, created_at, updated_at)
             VALUES (?1, 'Alice', 'alice', 'main', 1, ?2, 'test', datetime('now'), datetime('now'))",
            rusqlite::params![&agent_id, &workspace_path],
        ).unwrap();

        verify_agents_integrity_with_path(&conn, pane_root.clone());

        let identity_path = agent_dir.join("IDENTITY.md");
        assert!(
            identity_path.exists(),
            "IDENTITY.md stub should have been created"
        );
        let content = fs::read_to_string(&identity_path).unwrap();
        assert!(
            content.contains("# Alice"),
            "Created stub should contain \"# Alice\", got: {}",
            content
        );
        // Parse must agree with DB name — no divergence
        let parsed = parse_identity_name(&identity_path).unwrap();
        assert_eq!(
            parsed, "Alice",
            "Parsed name should match DB name after stub creation"
        );
    }

    #[test]
    fn test_parse_identity_name_heading() {
        use tempfile::TempDir;
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("IDENTITY.md");
        std::fs::write(&path, "# Stia\n\nYour name is Stia.\n").unwrap();
        assert_eq!(parse_identity_name(&path), Some("Stia".to_string()));
    }

    #[test]
    fn test_parse_identity_name_your_name_line() {
        use tempfile::TempDir;
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("IDENTITY.md");
        // No heading, only "Your name is" line
        std::fs::write(&path, "Your name is Reed.\n\nSome notes.\n").unwrap();
        assert_eq!(parse_identity_name(&path), Some("Reed".to_string()));
    }

    #[test]
    fn test_repair_identity_name_replaces_heading() {
        let content = "# Bob\n\nYour name is Bob.\nSome notes.\n";
        let repaired = repair_identity_name(content, "Alice");
        assert!(
            repaired.starts_with("# Alice\n"),
            "Should replace heading: {}",
            repaired
        );
        assert!(
            repaired.contains("Your name is Bob."),
            "Should preserve rest: {}",
            repaired
        );
    }
}

#[cfg(test)]
mod verify_integrity_orphan_cleanup_tests {
    use super::*;
    use super::identity_divergence_and_repair_tests::make_agent_schema;

    #[test]
    fn test_verify_integrity_removes_empty_orphan_uuid_dir() {
        // Regression test for #2030 Item 4: verify_agents_integrity removes empty UUID-format
        // orphan directories that have no DB record and no SOUL.md (left by crashed create_local_agent).
        // Non-empty dirs and SOUL.md dirs must be preserved.
        use std::fs;
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let pane_root = temp_dir.path().to_path_buf();
        let system_dir = pane_root.join(".pane").join("system");
        fs::create_dir_all(&system_dir).unwrap();

        // DB with no agents — every UUID dir in system/ is an orphan
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        make_agent_schema(&conn);

        // Case 1: empty UUID dir (32 hex chars, no files, no DB record) → must be removed
        let empty_uuid = uuid::Uuid::new_v4().simple().to_string();
        let empty_dir = system_dir.join(&empty_uuid);
        fs::create_dir_all(&empty_dir).unwrap();
        assert!(
            empty_dir.exists(),
            "precondition: empty orphan dir must exist before check"
        );

        // Case 2: non-empty UUID dir (has a file but no SOUL.md, no DB record) → preserved
        let nonempty_uuid = uuid::Uuid::new_v4().simple().to_string();
        let nonempty_dir = system_dir.join(&nonempty_uuid);
        fs::create_dir_all(&nonempty_dir).unwrap();
        fs::write(nonempty_dir.join("partial_data.txt"), "partial agent data").unwrap();

        // Case 3: UUID dir with SOUL.md (no DB record) → preserved via recovery path
        let soul_uuid = uuid::Uuid::new_v4().simple().to_string();
        let soul_dir = system_dir.join(&soul_uuid);
        fs::create_dir_all(&soul_dir).unwrap();
        fs::write(
            soul_dir.join("SOUL.md"),
            "# SOUL.md

Recoverable agent.
",
        )
        .unwrap();

        verify_agents_integrity_with_path(&conn, pane_root.clone());

        // Assert: empty orphan dir was removed (#2030 Item 4)
        assert!(
            !empty_dir.exists(),
            "Empty orphan UUID dir must be removed by integrity check (#2030 Item 4)"
        );

        // Assert: non-empty orphan dir without SOUL.md is preserved (may contain partial data)
        assert!(
            nonempty_dir.exists(),
            "Non-empty orphan UUID dir (no SOUL.md) must be preserved"
        );

        // Assert: dir with SOUL.md is preserved (agent is recovered into DB)
        assert!(
            soul_dir.exists(),
            "UUID dir with SOUL.md must be preserved via recovery path"
        );

        // Assert: the SOUL.md agent was registered in DB during recovery
        let recovered: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM ai_agents WHERE workspace_path = ?1",
                rusqlite::params![&soul_uuid],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            recovered,
            "Agent from SOUL.md dir must be registered in DB via integrity recovery path"
        );
    }
}


#[cfg(test)]
mod detect_openclaw_agents_tests {

    #[test]
    fn test_parse_oc_models_response() {
        // Test parsing of OC's /v1/models response format
        let oc_response = serde_json::json!({
            "object": "list",
            "data": [
                {
                    "id": "main",
                    "object": "model",
                    "created": 1234567890,
                    "owned_by": "openclaw",
                    "permission": []
                },
                {
                    "id": "assistant",
                    "object": "model",
                    "created": 1234567891,
                    "owned_by": "openclaw",
                    "permission": []
                }
            ]
        });

        // Simulate what detect_openclaw_agents does with the response
        let data = oc_response.get("data").and_then(|d| d.as_array());
        assert!(data.is_some(), "Should extract 'data' array from response");

        let models = data.unwrap();
        assert_eq!(models.len(), 2, "Should have 2 models");
        assert_eq!(models[0]["id"], "main");
        assert_eq!(models[1]["id"], "assistant");
    }

    #[test]
    fn test_parse_oc_models_response_no_data() {
        // Test handling of malformed response (no 'data' field)
        let bad_response = serde_json::json!({
            "object": "list"
        });

        let data = bad_response.get("data").and_then(|d| d.as_array());
        assert!(
            data.is_none(),
            "Should return None when 'data' field is missing"
        );
    }

    #[test]
    fn test_parse_oc_models_response_empty_data() {
        // Test handling of empty models list
        let empty_response = serde_json::json!({
            "object": "list",
            "data": []
        });

        let data = empty_response.get("data").and_then(|d| d.as_array());
        assert!(
            data.is_some(),
            "Should extract 'data' array even when empty"
        );
        assert_eq!(data.unwrap().len(), 0, "Should have 0 models");
    }
}


// === D11 (#2635): sync_initial_import creates the ai_agents row New-Provider
// pairing depends on ===
//
// Reed's audit (docs/audits/2026-08-21-d11-sync-endpoint-mismatch.md) confirmed the
// gap: `ai_configure_and_save`'s openclaw branch only UPDATEs an `ai_agents` row - it
// never INSERTs one - so `sync_client::has_active_openclaw_agent` (and therefore
// `get_gateway_url`) permanently sees zero rows unless something else creates that
// row first. `sync_initial_import` is the ONLY code path that INSERTs it (see the
// `ON CONFLICT(runtime_type, runtime_ref)` write above). This module proves the
// full #[tauri::command] handler - not just the inner INSERT - leaves the DB in the
// post-condition `sync_client::get_gateway_url()` depends on, using a real
// migrated schema (`run_migrations`, not a hand-rolled minimal table set) plus a
// real `tauri::test::mock_app` state so the test exercises the exact handler
// signature Svelte's `invoke('sync_initial_import', ...)` calls in production.
//
// Option 1 (teaching `ai_configure_and_save` to INSERT) is explicitly OUT of scope
// for this PR per the task ruling - these tests cover Option 2 (completePairing
// calling sync_initial_import), not a change to ai_configure_and_save.
#[cfg(test)]
mod d11_sync_initial_import_creates_agent_row_tests {
    use super::*;
    use crate::database::DbState;
    use crate::keychain::test_support::KeychainGuard;
    use serial_test::serial;
    use tauri::Manager;
    use tempfile::TempDir;

    /// Mirrors agents.rs's `upsert_agent_explicit_runtime_type_tests::setup` — a real
    /// tauri::App<MockRuntime> with a fresh-migrated in-memory DB managed as state, plus
    /// PANE_TEST_ROOT pointed at a scratch filesystem dir (sync_initial_import writes
    /// IDENTITY.md under .pane/system before the DB INSERT, same as upsert_agent).
    fn setup() -> (tauri::App<tauri::test::MockRuntime>, TempDir) {
        let fs_dir = TempDir::new().expect("failed to create scratch fs dir");
        std::env::set_var("PANE_TEST_ROOT", fs_dir.path());

        let conn = rusqlite::Connection::open_in_memory().expect("in-memory db");
        crate::migrations::run_migrations(&conn).expect("fresh-install migrations should succeed");

        let app = tauri::test::mock_app();
        app.manage(DbState::new(conn));
        (app, fs_dir)
    }

    fn teardown() {
    }

    /// End-to-end: mocked gateway returns one agent ("main") from `/v1/sync/initial`;
    /// calling the real `sync_initial_import` command must leave an active `ai_agents`
    /// row with `runtime_type = 'openclaw'` and `runtime_ref` populated - the exact
    /// precondition `sync_client::has_active_openclaw_agent` / `get_gateway_url` gate
    /// on. This is the "pairing via ProviderConnectionCard yields an active ai_agents
    /// row" acceptance test from the task description, minus the Svelte layer (that's
    /// covered separately in ProviderConnectionCard.test.ts by asserting the new
    /// `sync_initial_import` invoke call happens after `ai_configure_and_save`).
    #[tokio::test]
    #[serial(pane_test_root, keychain_override)]
    async fn test_sync_initial_import_creates_active_openclaw_agent_row() {
        let _kc = KeychainGuard::set();
        let (app, _fs_dir) = setup();

        let mut server = mockito::Server::new_async().await;
        let body = r#"{"agents":[{"oc_id":"main","name":"Stia","files":[{"filename":"SOUL.md","content":"SOUL content","checksum":"abc"}]}]}"#;
        let _m = server
            .mock("GET", "/v1/sync/initial")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(body)
            .create_async()
            .await;

        let db_state = app.state::<DbState>();
        let imported = sync_initial_import(server.url(), "test-token".to_string(), db_state)
            .await
            .expect("sync_initial_import should succeed against the mocked gateway");

        assert_eq!(imported.agent_names, vec!["Stia".to_string()], "must report the imported agent's display name");

        let conn_state = app.state::<DbState>();
        let conn = conn_state.0.lock().unwrap();

        // The precondition sync_client::has_active_openclaw_agent gates on.
        let has_active: bool = conn
            .query_row(
                "SELECT 1 FROM ai_agents WHERE runtime_type = 'openclaw' AND is_active = 1 LIMIT 1",
                [],
                |_| Ok(()),
            )
            .is_ok();
        assert!(has_active, "sync_initial_import must create an active runtime_type='openclaw' ai_agents row");

        let runtime_ref: String = conn
            .query_row(
                "SELECT runtime_ref FROM ai_agents WHERE runtime_type = 'openclaw' LIMIT 1",
                [],
                |row| row.get(0),
            )
            .expect("runtime_ref column must be readable");
        assert_eq!(runtime_ref, "main", "runtime_ref must be the gateway's oc_id ('main')");

        drop(conn);
        teardown();
    }

    /// The other half of the acceptance test: once the ai_agents row exists (via
    /// sync_initial_import, as above) AND `ai_settings.endpoint` is set (as
    /// `ai_configure_and_save` already does correctly per Reed's audit),
    /// `sync_client::get_gateway_url()` must return the endpoint - not None. Proves
    /// the fix closes the actual reported symptom ("gateway paired, but sync never
    /// detects it") at the sync_client read boundary, not just at the DB-row level.
    #[tokio::test]
    #[serial(pane_test_root, keychain_override)]
    async fn test_get_gateway_url_returns_endpoint_after_sync_initial_import() {
        let _kc = KeychainGuard::set();
        let (app, _fs_dir) = setup();

        let mut server = mockito::Server::new_async().await;
        let body = r#"{"agents":[{"oc_id":"main","name":"Stia","files":[]}]}"#;
        let _m = server
            .mock("GET", "/v1/sync/initial")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(body)
            .create_async()
            .await;

        let gateway_url = server.url();

        {
            // Mirrors what ai_configure_and_save already writes correctly (Reed's audit
            // §1: the endpoint key name is NOT the bug) - this test seeds it directly
            // rather than calling ai_configure_and_save, since that command also makes a
            // real fetch_oc_agent_model HTTP call this test isn't mocking.
            let conn_state = app.state::<DbState>();
            let conn = conn_state.0.lock().unwrap();
            crate::database::set_ai_setting(&conn, "endpoint", &gateway_url).unwrap();
        }

        let db_state = app.state::<DbState>();
        sync_initial_import(gateway_url.clone(), "test-token".to_string(), db_state)
            .await
            .expect("sync_initial_import should succeed against the mocked gateway");

        let conn_state = app.state::<DbState>();
        let conn = conn_state.0.lock().unwrap();
        assert_eq!(
            crate::sync_client::get_gateway_url(&conn).as_deref(),
            Some(gateway_url.as_str()),
            "get_gateway_url must return the endpoint once sync_initial_import has run - this is the D11 (#2635) regression this PR fixes"
        );

        drop(conn);
        teardown();
    }
}


// === D13 (#2640): sync_initial_import is_default collision ===
//
// Reed's scoping audit (docs/audits/2026-08-21-d13-is-default-collision-scoping.md)
// found `sync_initial_import` unconditionally set `is_default = (oc_id == "main")`
// with no awareness of a pre-existing default agent, causing the INSERT to collide
// with `idx_ai_agents_default` (a partial UNIQUE index on `is_default WHERE
// is_default = 1`) whenever a non-OC default agent already exists — rolling back
// the entire import batch. This module proves the three fixes from Matt's ruling:
// (1) the pre-flight default-agent-exists check correctly suppresses is_default on
// the OC main agent when a default already exists, without touching the existing
// default or rolling back the batch; (2) a genuinely-fresh DB still lets the OC
// main agent become the default; (3) slug collisions are resolved with a suffix
// instead of propagating a raw UNIQUE constraint error.
#[cfg(test)]
mod d13_sync_initial_import_is_default_collision_tests {
    use super::*;
    use crate::database::DbState;
    use crate::keychain::test_support::KeychainGuard;
    use serial_test::serial;
    use tauri::Manager;
    use tempfile::TempDir;

    /// Same setup pattern as `d11_sync_initial_import_creates_agent_row_tests::setup`
    /// — a real tauri::App<MockRuntime> with a fresh-migrated in-memory DB, plus
    /// PANE_TEST_ROOT pointed at a scratch filesystem dir.
    fn setup() -> (tauri::App<tauri::test::MockRuntime>, TempDir) {
        let fs_dir = TempDir::new().expect("failed to create scratch fs dir");
        std::env::set_var("PANE_TEST_ROOT", fs_dir.path());

        let conn = rusqlite::Connection::open_in_memory().expect("in-memory db");
        crate::migrations::run_migrations(&conn).expect("fresh-install migrations should succeed");

        let app = tauri::test::mock_app();
        app.manage(DbState::new(conn));
        (app, fs_dir)
    }

    fn teardown() {
    }

    /// Insert a non-OC default agent directly, mirroring an OpenRouter-onboarded
    /// default (`is_default=1`, `runtime_ref` NULL) — the exact pre-state from the
    /// bug report's repro steps (onboard a non-OC provider first, then pair OC).
    fn seed_non_oc_default_agent(conn: &rusqlite::Connection, name: &str, slug: &str) {
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO ai_agents (id, name, slug, role, is_default, is_active, workspace_path, runtime_type, runtime_ref, created_at, updated_at)
             VALUES (?1, ?2, ?3, 'main', 1, 1, ?1, 'openrouter', NULL, ?4, ?4)",
            rusqlite::params![slug, name, slug, now],
        )
        .expect("failed to seed non-OC default agent");
    }

    /// Test 1 (task-required): pre-existing default, OC import lands non-default.
    /// Set up an OpenRouter-onboarded default ("Opal"), then run sync_initial_import
    /// with an OC gateway returning 2 agents (main + a subagent). Assert: OC main
    /// agent lands with is_default=0, role='local' (not 'sub' — must stay pickable
    /// in the agent picker per DefaultAgentPickerModal.svelte's `role !== 'sub'`
    /// filter); the pre-existing default agent is untouched (is_default=1 still);
    /// ALL agents from the batch are inserted (no rollback).
    #[tokio::test]
    #[serial(pane_test_root, keychain_override)]
    async fn test_pre_existing_default_oc_import_lands_non_default() {
        let _kc = KeychainGuard::set();
        let (app, _fs_dir) = setup();

        {
            let conn_state = app.state::<DbState>();
            let conn = conn_state.0.lock().unwrap();
            seed_non_oc_default_agent(&conn, "Opal", "opal");
        }

        let mut server = mockito::Server::new_async().await;
        let body = r#"{"agents":[{"oc_id":"main","name":"Stia","files":[]},{"oc_id":"sub-1","name":"Reed","files":[]}]}"#;
        let _m = server
            .mock("GET", "/v1/sync/initial")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(body)
            .create_async()
            .await;

        let db_state = app.state::<DbState>();
        let imported = sync_initial_import(server.url(), "test-token".to_string(), db_state)
            .await
            .expect("sync_initial_import must not roll back when a default agent already exists (#2640)");

        assert_eq!(
            imported.agent_names.len(),
            2,
            "all agents in the batch must be inserted, no rollback"
        );

        let conn_state = app.state::<DbState>();
        let conn = conn_state.0.lock().unwrap();

        let (oc_main_is_default, oc_main_role): (i64, String) = conn
            .query_row(
                "SELECT is_default, role FROM ai_agents WHERE runtime_type = 'openclaw' AND runtime_ref = 'main'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .expect("OC main agent row must exist");
        assert_eq!(oc_main_is_default, 0, "OC main agent must NOT be default when a default already exists");
        assert_eq!(oc_main_role, "local", "OC main agent must land with role='local', not 'sub', so it stays pickable in the agent picker");

        let (opal_is_default,): (i64,) = conn
            .query_row(
                "SELECT is_default FROM ai_agents WHERE slug = 'opal'",
                [],
                |r| Ok((r.get(0)?,)),
            )
            .expect("pre-existing default agent row must still exist");
        assert_eq!(opal_is_default, 1, "pre-existing default agent must be untouched (is_default=1) — no import path may steal default status");

        let default_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM ai_agents WHERE is_default = 1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(default_count, 1, "exactly one default agent must exist after import");

        drop(conn);
        teardown();
    }

    /// Test 2 (task-required): fresh DB, no default, OC main becomes default.
    /// Empty ai_agents table. Run sync_initial_import. Assert: OC main agent lands
    /// with is_default=1, role='main' — a genuinely fresh OC-first install must
    /// still get a default agent, otherwise every `is_default=1 AND is_active=1`
    /// lookup site breaks.
    #[tokio::test]
    #[serial(pane_test_root, keychain_override)]
    async fn test_fresh_db_no_default_oc_main_becomes_default() {
        let _kc = KeychainGuard::set();
        let (app, _fs_dir) = setup();

        let mut server = mockito::Server::new_async().await;
        let body = r#"{"agents":[{"oc_id":"main","name":"Stia","files":[]}]}"#;
        let _m = server
            .mock("GET", "/v1/sync/initial")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(body)
            .create_async()
            .await;

        let db_state = app.state::<DbState>();
        sync_initial_import(server.url(), "test-token".to_string(), db_state)
            .await
            .expect("sync_initial_import should succeed on a fresh DB");

        let conn_state = app.state::<DbState>();
        let conn = conn_state.0.lock().unwrap();
        let (is_default, role): (i64, String) = conn
            .query_row(
                "SELECT is_default, role FROM ai_agents WHERE runtime_type = 'openclaw' AND runtime_ref = 'main'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .expect("OC main agent row must exist");
        assert_eq!(is_default, 1, "on a fresh DB with no existing default, OC main agent must become the default");
        assert_eq!(role, "main", "the default agent must have role='main'");

        drop(conn);
        teardown();
    }

    /// Test 3 (task-required): slug collision resolves with suffix.
    /// Seed an existing agent with slug 'assistant' (no runtime_ref — a locally
    /// created agent), then import an OC agent named "Assistant" (same base slug).
    /// Assert: the imported agent gets a suffixed slug ('assistant-1'), no error,
    /// and both agents (the pre-existing one and the imported one) end up in the DB.
    #[tokio::test]
    #[serial(pane_test_root, keychain_override)]
    async fn test_slug_collision_resolves_with_suffix() {
        let _kc = KeychainGuard::set();
        let (app, _fs_dir) = setup();

        {
            let conn_state = app.state::<DbState>();
            let conn = conn_state.0.lock().unwrap();
            let now = chrono::Utc::now().to_rfc3339();
            conn.execute(
                "INSERT INTO ai_agents (id, name, slug, role, is_default, is_active, workspace_path, runtime_type, runtime_ref, created_at, updated_at)
                 VALUES ('pre-existing-id', 'Assistant', 'assistant', 'local', 1, 1, 'pre-existing-id', 'openrouter', NULL, ?1, ?1)",
                rusqlite::params![now],
            )
            .expect("failed to seed colliding-slug agent");
        }

        let mut server = mockito::Server::new_async().await;
        let body = r#"{"agents":[{"oc_id":"main","name":"Assistant","files":[]}]}"#;
        let _m = server
            .mock("GET", "/v1/sync/initial")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(body)
            .create_async()
            .await;

        let db_state = app.state::<DbState>();
        let imported = sync_initial_import(server.url(), "test-token".to_string(), db_state)
            .await
            .expect("sync_initial_import must resolve slug collisions with a suffix, not error out (#2640)");
        assert_eq!(imported.agent_names, vec!["Assistant".to_string()]);

        let conn_state = app.state::<DbState>();
        let conn = conn_state.0.lock().unwrap();

        let imported_slug: String = conn
            .query_row(
                "SELECT slug FROM ai_agents WHERE runtime_type = 'openclaw' AND runtime_ref = 'main'",
                [],
                |r| r.get(0),
            )
            .expect("imported OC main agent row must exist");
        assert_eq!(imported_slug, "assistant-1", "imported agent's slug must be suffixed to avoid colliding with the pre-existing 'assistant' slug");

        let agent_count: i64 = conn.query_row("SELECT COUNT(*) FROM ai_agents", [], |r| r.get(0)).unwrap();
        assert_eq!(agent_count, 2, "both the pre-existing agent and the imported agent must be present — no rollback");

        drop(conn);
        teardown();
    }
}


// === D17 (#2647): sync_initial_import writes model in its INSERT ===
//
// Covers the three D17-specific test cases the task called out that aren't already
// exercised by d11_/d13_'s tests above:
//   - ON CONFLICT CASE preservation/self-heal (Reed's Option C, docs/audits/
//     2026-08-21-pr1-on-conflict-scope-check.md §1 Q1) exercised by calling
//     sync_initial_import a second time against rows that already exist.
//   - The model soft-fail path: /v1/agents (fetch_oc_agent_model) fails while
//     /v1/sync/initial succeeds — import must still complete, model must be NULL,
//     and agents_missing_model must name the affected agent for the UI to surface.
#[cfg(test)]
mod d17_sync_initial_import_model_in_insert_tests {
    use super::*;
    use crate::database::DbState;
    use crate::keychain::test_support::KeychainGuard;
    use serial_test::serial;
    use tauri::Manager;
    use tempfile::TempDir;

    /// Same setup pattern as d11_/d13_'s test modules above.
    fn setup() -> (tauri::App<tauri::test::MockRuntime>, TempDir) {
        let fs_dir = TempDir::new().expect("failed to create scratch fs dir");
        std::env::set_var("PANE_TEST_ROOT", fs_dir.path());

        let conn = rusqlite::Connection::open_in_memory().expect("in-memory db");
        crate::migrations::run_migrations(&conn).expect("fresh-install migrations should succeed");

        let app = tauri::test::mock_app();
        app.manage(DbState::new(conn));
        (app, fs_dir)
    }

    fn teardown() {
    }

    /// Test: model soft-fail. /v1/sync/initial succeeds (agents returned) but
    /// /v1/agents (fetch_oc_agent_model's endpoint) returns a 500. Import must NOT
    /// abort: the main agent lands with model=NULL, and agents_missing_model names it
    /// // CI-ALLOW: hardcoded-model — "Stia" is a fixture display name, not a real model string.
    #[tokio::test]
    #[serial(pane_test_root, keychain_override)]
    async fn test_model_fetch_soft_fail_import_still_succeeds_and_names_the_agent() {
        let _kc = KeychainGuard::set();
        let (app, _fs_dir) = setup();

        let mut server = mockito::Server::new_async().await;
        let sync_body = r#"{"agents":[{"oc_id":"main","name":"Stia","files":[]}]}"#;
        let _m_sync = server
            .mock("GET", "/v1/sync/initial")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(sync_body)
            .create_async()
            .await;
        let _m_agents = server
            .mock("GET", "/v1/agents")
            .with_status(500)
            .with_body("gateway internal error")
            .create_async()
            .await;

        let db_state = app.state::<DbState>();
        let imported = sync_initial_import(server.url(), "test-token".to_string(), db_state)
            .await
            .expect("sync_initial_import must NOT abort when /v1/agents (model fetch) soft-fails — only /v1/sync/initial failing is fatal");

        assert_eq!(imported.agent_names, vec!["Stia".to_string()], "import must still complete and report the agent");
        assert_eq!(
            imported.agents_missing_model,
            vec!["Stia".to_string()],
            "the main agent's soft-failed model fetch must be named in agents_missing_model so the UI can surface a real warning, not a silent log-only failure"
        );

        let conn_state = app.state::<DbState>();
        let conn = conn_state.0.lock().unwrap();
        let model: Option<String> = conn
            .query_row(
                "SELECT model FROM ai_agents WHERE runtime_type = 'openclaw' AND runtime_ref = 'main'",
                [],
                |r| r.get(0),
            )
            .expect("OC main agent row must exist despite the model soft-fail");
        assert_eq!(model, None, "model must be NULL when fetch_oc_agent_model soft-fails, not an empty string or placeholder");

        drop(conn);
        teardown();
    }

    /// Test 3-variant (task-required, Reed's recommendation): ON CONFLICT CASE
    /// self-heals a pre-existing NULL model. Seed an ai_agents row with
    /// runtime_type='openclaw', runtime_ref='main', model=NULL (the exact D17-affected
    /// pre-fix state), then call sync_initial_import again (directly, bypassing the
    /// D18 re-pair signal, per the task's instruction to exercise the ON CONFLICT
    /// clause specifically). Assert the CASE clause self-heals: model gets populated
    /// from the fresh fetch_oc_agent_model result via `excluded.model`.
    #[tokio::test]
    #[serial(pane_test_root, keychain_override)]
    async fn test_on_conflict_case_self_heals_null_model() {
        let _kc = KeychainGuard::set();
        let (app, _fs_dir) = setup();

        let now = chrono::Utc::now().to_rfc3339();
        {
            let conn_state = app.state::<DbState>();
            let conn = conn_state.0.lock().unwrap();
            conn.execute(
                "INSERT INTO ai_agents (id, name, slug, role, is_default, is_active, workspace_path, runtime_type, runtime_ref, model, created_at, updated_at)
                 VALUES ('oc-main-id', 'Stia', 'stia', 'main', 1, 1, 'oc-main-id', 'openclaw', 'main', NULL, ?1, ?1)",
                rusqlite::params![now],
            )
            .expect("failed to seed pre-existing D17-affected NULL-model row");
        }

        let mut server = mockito::Server::new_async().await;
        let sync_body = r#"{"agents":[{"oc_id":"main","name":"Stia","files":[]}]}"#;
        let _m_sync = server
            .mock("GET", "/v1/sync/initial")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(sync_body)
            .create_async()
            .await;
        let agents_body = r#"{"agents":[{"oc_agent_id":"main","model":"anthropic/claude-opus-4"}]}"#; // CI-ALLOW: hardcoded-model
        let _m_agents = server
            .mock("GET", "/v1/agents")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(agents_body)
            .create_async()
            .await;

        let db_state = app.state::<DbState>();
        let imported = sync_initial_import(server.url(), "test-token".to_string(), db_state)
            .await
            .expect("sync_initial_import must succeed on a second call against an existing row (ON CONFLICT path)");

        assert!(
            imported.agents_missing_model.is_empty(),
            "model fetch succeeded this time, so agents_missing_model must be empty"
        );

        let conn_state = app.state::<DbState>();
        let conn = conn_state.0.lock().unwrap();
        let model: Option<String> = conn
            .query_row(
                "SELECT model FROM ai_agents WHERE runtime_type = 'openclaw' AND runtime_ref = 'main'",
                [],
                |r| r.get(0),
            )
            .expect("OC main agent row must exist");
        assert_eq!(
            model.as_deref(),
            Some("anthropic/claude-opus-4"), // CI-ALLOW: hardcoded-model
            "ON CONFLICT CASE must self-heal a pre-existing NULL model from excluded.model on re-run"
        );

        let row_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM ai_agents WHERE runtime_type = 'openclaw' AND runtime_ref = 'main'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(row_count, 1, "ON CONFLICT must UPDATE the existing row, not create a duplicate");

        drop(conn);
        teardown();
    }

    /// Integration-level regression guard for D17's `ai_configure_and_save` removal:
    /// calls the REAL command (not a hand-rolled UPDATE) against a mocked `/v1/agents`
    /// endpoint, with a pre-existing OC main agent row seeded with `model = NULL`
    /// (simulating the moment right after `sync_initial_import` created the row without
    /// a model, or before any import has run at all). Asserts:
    ///   1. `ai_configure_and_save` still succeeds (connectivity validation passes).
    ///   2. `fetch_oc_agent_model`'s underlying `/v1/agents` mock IS hit (proves the
    ///      #1446 connectivity-validation call is still made, unconditionally).
    ///   3. `ai_agents.model` for the runtime_ref='main' row is STILL NULL afterward —
    ///      proves the model-UPDATE write is actually gone from the real code path, not
    ///      just from a test that used to hand-simulate it.
    #[tokio::test]
    #[serial(pane_test_root, keychain_override)]
    async fn test_ai_configure_and_save_openclaw_does_not_write_model() {
        let _kc = KeychainGuard::set();
        let (app, _fs_dir) = setup();

        let now = chrono::Utc::now().to_rfc3339();
        {
            let conn_state = app.state::<DbState>();
            let conn = conn_state.0.lock().unwrap();
            conn.execute(
                "INSERT INTO ai_agents (id, name, slug, role, is_default, is_active, workspace_path, runtime_type, runtime_ref, model, created_at, updated_at)
                 VALUES ('oc-main-id', 'Stia', 'stia', 'main', 1, 1, 'oc-main-id', 'openclaw', 'main', NULL, ?1, ?1)",
                rusqlite::params![now],
            )
            .expect("failed to seed pre-existing NULL-model OC main row");
        }

        let mut server = mockito::Server::new_async().await;
        let agents_body = r#"{"agents":[{"oc_agent_id":"main","model":"anthropic/claude-opus-4"}]}"#; // CI-ALLOW: hardcoded-model
        let _m_agents = server
            .mock("GET", "/v1/agents")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(agents_body)
            .create_async()
            .await;

        let db_state = app.state::<DbState>();
        let result = ai_configure_and_save(
            "openclaw".to_string(),
            Some("test-token".to_string()),
            None,
            Some(server.url()),
            None,
            db_state,
        )
        .await;
        assert!(
            result.is_ok(),
            "ai_configure_and_save must still succeed (connectivity validation passes): {:?}",
            result
        );

        let conn_state = app.state::<DbState>();
        let conn = conn_state.0.lock().unwrap();
        let model: Option<String> = conn
            .query_row(
                "SELECT model FROM ai_agents WHERE runtime_type = 'openclaw' AND runtime_ref = 'main'",
                [],
                |r| r.get(0),
            )
            .expect("OC main agent row must still exist");
        assert_eq!(
            model, None,
            "#2647 D17: ai_configure_and_save must NOT write ai_agents.model even though \
             fetch_oc_agent_model succeeded — that write now lives exclusively in \
             sync_initial_import's INSERT/ON CONFLICT CASE"
        );

        drop(conn);
        teardown();
    }

    /// Test 3-variant (task-required): ON CONFLICT CASE preserves a user-chosen
    /// non-NULL model — never clobbers it, even when the fresh gateway fetch returns a
    /// different model. Seed the row with a real, non-NULL model already set (the
    /// user's own choice, or a self-healed value from a prior run), then re-run
    /// sync_initial_import with a DIFFERENT model in the mocked /v1/agents response.
    /// Assert: the existing model is untouched.
    #[tokio::test]
    #[serial(pane_test_root, keychain_override)]
    async fn test_on_conflict_case_preserves_existing_non_null_model() {
        let _kc = KeychainGuard::set();
        let (app, _fs_dir) = setup();

        let now = chrono::Utc::now().to_rfc3339();
        {
            let conn_state = app.state::<DbState>();
            let conn = conn_state.0.lock().unwrap();
            // CI-ALLOW: hardcoded-model — fixture representing the user's already-chosen model.
            conn.execute(
                "INSERT INTO ai_agents (id, name, slug, role, is_default, is_active, workspace_path, runtime_type, runtime_ref, model, created_at, updated_at)
                 VALUES ('oc-main-id', 'Stia', 'stia', 'main', 1, 1, 'oc-main-id', 'openclaw', 'main', 'anthropic/claude-sonnet-4', ?1, ?1)",
                rusqlite::params![now],
            )
            .expect("failed to seed pre-existing non-NULL-model row");
        }

        let mut server = mockito::Server::new_async().await;
        let sync_body = r#"{"agents":[{"oc_id":"main","name":"Stia","files":[]}]}"#;
        let _m_sync = server
            .mock("GET", "/v1/sync/initial")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(sync_body)
            .create_async()
            .await;
        let agents_body = r#"{"agents":[{"oc_agent_id":"main","model":"openai/gpt-5"}]}"#; // CI-ALLOW: hardcoded-model
        let _m_agents = server
            .mock("GET", "/v1/agents")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(agents_body)
            .create_async()
            .await;

        let db_state = app.state::<DbState>();
        sync_initial_import(server.url(), "test-token".to_string(), db_state)
            .await
            .expect("sync_initial_import must succeed on a second call against an existing row (ON CONFLICT path)");

        let conn_state = app.state::<DbState>();
        let conn = conn_state.0.lock().unwrap();
        let model: Option<String> = conn
            .query_row(
                "SELECT model FROM ai_agents WHERE runtime_type = 'openclaw' AND runtime_ref = 'main'",
                [],
                |r| r.get(0),
            )
            .expect("OC main agent row must exist");
        assert_eq!(
            model.as_deref(),
            Some("anthropic/claude-sonnet-4"), // CI-ALLOW: hardcoded-model
            "ON CONFLICT CASE must preserve an existing non-NULL model — never clobber a user's chosen model, even when the gateway fetch returns a different value"
        );

        drop(conn);
        teardown();
    }
}


#[cfg(test)]
mod f5_sync_initial_import_role_self_heal_tests {
    use super::*;
    use crate::database::DbState;
    use crate::keychain::test_support::KeychainGuard;
    use serial_test::serial;
    use tauri::Manager;
    use tempfile::TempDir;

    /// Same setup pattern as d11_/d13_/d17_'s test modules above.
    fn setup() -> (tauri::App<tauri::test::MockRuntime>, TempDir) {
        let fs_dir = TempDir::new().expect("failed to create scratch fs dir");
        std::env::set_var("PANE_TEST_ROOT", fs_dir.path());

        let conn = rusqlite::Connection::open_in_memory().expect("in-memory db");
        crate::migrations::run_migrations(&conn).expect("fresh-install migrations should succeed");

        let app = tauri::test::mock_app();
        app.manage(DbState::new(conn));
        (app, fs_dir)
    }

    fn teardown() {
    }

    /// F5 item 2 regression guard (#2455 venice live-test v2): a subagent row
    /// already corrupted (role='main', simulating pre-fix 1b safety-net damage,
    /// e.g. Matt's real Stia row: `role='main', runtime_ref='coder'`) must have
    /// its role self-healed back to 'sub' the next time sync_initial_import's ON
    /// CONFLICT clause fires for that same oc_id — without needing a fresh
    /// migration/app-upgrade to repair it.
    #[tokio::test]
    #[serial(pane_test_root, keychain_override)]
    async fn test_sync_initial_import_self_heals_corrupted_subagent_role_on_conflict() {
        let _kc = KeychainGuard::set();
        let (app, _fs_dir) = setup();

        let now = chrono::Utc::now().to_rfc3339();
        {
            let conn_state = app.state::<DbState>();
            let conn = conn_state.0.lock().unwrap();
            // Corrupted subagent row: runtime_ref='coder' (an OC subagent, NOT OC's
            // own main agent) with role force-written to 'main' by the pre-fix 1b
            // safety net.
            conn.execute(
                "INSERT INTO ai_agents (id, name, slug, role, is_default, is_active, workspace_path, runtime_type, runtime_ref, created_at, updated_at)
                 VALUES ('coder-id', 'Coder', 'coder', 'main', 0, 1, 'coder-id', 'openclaw', 'coder', ?1, ?1)",
                rusqlite::params![now],
            )
            .expect("failed to seed pre-corrupted subagent row");
        }

        let mut server = mockito::Server::new_async().await;
        let sync_body = r#"{"agents":[{"oc_id":"coder","name":"Coder","files":[]}]}"#;
        let _m_sync = server
            .mock("GET", "/v1/sync/initial")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(sync_body)
            .create_async()
            .await;
        let agents_body = r#"{"agents":[]}"#;
        let _m_agents = server
            .mock("GET", "/v1/agents")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(agents_body)
            .create_async()
            .await;

        let db_state = app.state::<DbState>();
        sync_initial_import(server.url(), "test-token".to_string(), db_state)
            .await
            .expect("sync_initial_import must succeed on the ON CONFLICT path");

        let conn_state = app.state::<DbState>();
        let conn = conn_state.0.lock().unwrap();
        let role: String = conn
            .query_row("SELECT role FROM ai_agents WHERE runtime_ref = 'coder'", [], |r| r.get(0))
            .expect("coder agent row must exist");
        assert_eq!(role, "sub", "a corrupted subagent role must self-heal to 'sub' on the ON CONFLICT re-import path");

        drop(conn);
        teardown();
    }

    /// F5 negative-case guard: the scoped self-heal CASE (`excluded.runtime_ref != 'main'`)
    /// must NEVER touch OC's own main/local row's role, even on a re-run where this
    /// loop's freshly-computed `role` local would (incorrectly) compute 'local' because
    /// `default_agent_exists` is true from the very row being re-imported holding the
    /// seat already. Before this fix, a bare `role = excluded.role` would silently
    /// demote an already-'main' row to 'local' on every re-pair — a real corruption
    /// this test would have caught.
    #[tokio::test]
    #[serial(pane_test_root, keychain_override)]
    async fn test_sync_initial_import_never_overwrites_oc_main_row_role_on_conflict() {
        let _kc = KeychainGuard::set();
        let (app, _fs_dir) = setup();

        let now = chrono::Utc::now().to_rfc3339();
        {
            let conn_state = app.state::<DbState>();
            let conn = conn_state.0.lock().unwrap();
            // OC's own main/local row, already correctly promoted to default with
            // role='main' from a prior pairing.
            conn.execute(
                "INSERT INTO ai_agents (id, name, slug, role, is_default, is_active, workspace_path, runtime_type, runtime_ref, created_at, updated_at)
                 VALUES ('main-id', 'Main Agent', 'main-agent', 'main', 1, 1, 'main-id', 'openclaw', 'main', ?1, ?1)",
                rusqlite::params![now],
            )
            .expect("failed to seed pre-existing OC main row");
        }

        let mut server = mockito::Server::new_async().await;
        let sync_body = r#"{"agents":[{"oc_id":"main","name":"Main Agent","files":[]}]}"#;
        let _m_sync = server
            .mock("GET", "/v1/sync/initial")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(sync_body)
            .create_async()
            .await;
        let agents_body = r#"{"agents":[]}"#;
        let _m_agents = server
            .mock("GET", "/v1/agents")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(agents_body)
            .create_async()
            .await;

        let db_state = app.state::<DbState>();
        sync_initial_import(server.url(), "test-token".to_string(), db_state)
            .await
            .expect("sync_initial_import must succeed on the ON CONFLICT path");

        let conn_state = app.state::<DbState>();
        let conn = conn_state.0.lock().unwrap();
        let role: String = conn
            .query_row("SELECT role FROM ai_agents WHERE runtime_ref = 'main'", [], |r| r.get(0))
            .expect("main agent row must exist");
        assert_eq!(role, "main", "the self-heal CASE must never overwrite OC's own main/local row's role");

        drop(conn);
        teardown();
    }
}

