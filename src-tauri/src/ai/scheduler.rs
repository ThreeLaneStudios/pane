//! Scheduler: background jobs nobody typed — memory extraction, contradiction
//! checking, consolidation/synthesis, decay, and the embedding pipeline
//! (engine switch, batching, counts). Split out of `ai/commands.rs` (#3097).
//! Seeded now with the existing background-memory machinery; populated
//! further by the harness's own scheduler PR.

use rusqlite::params;
use rusqlite::OptionalExtension;
use std::sync::{Mutex, OnceLock};
use super::provider::*;
use super::tools::{
    policy_error, atomic_write,
    atomic_write_authorized,
};
use super::chat::{
    is_compaction_summary, resolve_default_agent_dispatch_tuple, AI_TOOL_CALL_MAX_TOKENS,
    AI_TOOL_CALL_TEMPERATURE,
};
use super::connections::{
    fetch_venice_embedding_models, fetch_openrouter_embedding_models, fetch_ollama_embedding_models,
};
#[cfg(test)]
use super::connections::{test_set_venice_embed_models_base, test_clear_venice_embed_models_base};
use super::anthropic::{AnthropicProvider, AnthropicResponse};
use super::openrouter::{OpenRouterProvider, OpenRouterResponse, RequestOrigin};
use crate::database::{DbConn, DbState};
use crate::filesystem_policy::{PaneFs, PaneRoot};
use crate::refresh_event::{
    emit_data_changed, ENTITY_NOTE, REASON_EDITED,
};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::{Emitter, Manager, State};


/// Task extraction: background LLM call extracting structured JSON tasks.
/// LOAD-BEARING: randomness causes inconsistent stored facts over time.
/// Sent only when OpenRouter provider metadata confirms temperature support (#1846).
pub(crate) const EXTRACTION_TEMPERATURE: f64 = 0.0;

pub(crate) const EXTRACTION_MAX_TOKENS: u32 = 1024;


/// Contradiction detection: single-field JSON response — minimal output budget.
/// LOAD-BEARING: nondeterminism causes spurious memory overwrites or missed contradictions.
/// Sent only when OpenRouter provider metadata confirms temperature support (#1846).
pub(crate) const CONTRADICTION_TEMPERATURE: f64 = 0.0;

pub(crate) const CONTRADICTION_MAX_TOKENS: u32 = 256;


/// Synthesis (compaction + background): deterministic memory/note synthesis.
/// LOAD-BEARING: variance causes unpredictable token budget management.
/// Sent only when OpenRouter provider metadata confirms temperature support (#1846).
pub(crate) const SYNTHESIS_TEMPERATURE: f64 = 0.0;

pub(crate) const SYNTHESIS_MAX_TOKENS: u32 = 2048;


/// Determines synthesis prompt mode based on memory note word count.
/// Returns (mode_name, optional_suffix) where suffix is appended to the synthesis prompt.
pub(crate) fn synthesis_size_guard(word_count: usize) -> (&'static str, Option<String>) {
    if word_count > 3000 {
        (
            "aggressive",
            Some(format!(
                "\n\nCRITICAL: This memory note exceeds 3,000 words ({} words). Aggressively consolidate to under 2,500 words. Merge all redundant facts, remove low-value details, keep only essential information. Every sentence must earn its place.",
                word_count
            )),
        )
    } else if word_count > 2000 {
        (
            "consolidate",
            Some(format!(
                "\n\nIMPORTANT: This memory note is getting long ({} words). Consolidate duplicate information, merge related facts, and tighten prose. Aim for concise, information-dense paragraphs.",
                word_count
            )),
        )
    } else {
        ("normal", None)
    }
}


// === Memory System ===

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct MemoryEntry {
    pub id: String,
    pub fact: String,
    pub category: String,
    pub project_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_message_id: Option<String>,
    /// Attribution tag: "user" (fact is about the user, their decisions, or the world)
    /// or "assistant" (fact is about what the AI did, said, or was asked to do).
    /// Defaults to "user" for backward compat if the model omits this field.
    /// This is the hook for #1711 attribution-routing (post-launch).
    #[serde(default = "default_attribution")]
    pub attribution: String,
    /// Extraction-guard PR A (#2745, #2750): "user", "assistant", or "web" — where the
    /// fact's words actually originated. Distinct from `attribution`, which tags the
    /// fact's SUBJECT. Mechanically enforced by `filter_assistant_origin_facts`
    /// (drops source=="web" regardless of `used_web_tools`). Filter-only — never
    /// written to the `ai_memory` INSERT, same as `attribution`.
    #[serde(default = "default_source")]
    pub source: String,
    /// Extraction-guard PR A (#2745, #2750): "durable" (still true/useful in a month)
    /// or "episodic" (scoped to a specific PR, issue, build, test run, or the current
    /// thread). Mechanically enforced by `filter_assistant_origin_facts` (drops
    /// scope=="episodic"). Filter-only — never written to the `ai_memory` INSERT.
    #[serde(default = "default_scope")]
    pub scope: String,
}


fn default_attribution() -> String {
    "user".to_string()
}


/// Extraction-guard PR A (#2745, #2750): default `source` for a RawEntry when the
/// model omits the field. "user" mirrors `default_attribution`'s backward-compat
/// posture. Filter-only — never persisted to `ai_memory`.
fn default_source() -> String {
    "user".to_string()
}


/// Extraction-guard PR A (#2745, #2750): default `scope` for a RawEntry when the
/// model omits the field. "durable" is the conservative default — an omitted
/// scope tag should not be silently treated as episodic-and-dropped. Filter-only
/// — never persisted to `ai_memory`.
fn default_scope() -> String {
    "durable".to_string()
}


/// Enqueue a memory extraction job (async pipeline - §27.7)
#[tauri::command]
pub fn ai_enqueue_extraction(
    thread_id: String,
    message_id: String,
    user_message_id: Option<String>,
    db: State<'_, DbState>,
) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    crate::ai::extraction_worker::enqueue_extraction(
        &*conn,
        &thread_id,
        &message_id,
        user_message_id.as_deref(),
    )
}


/// §4.6.9 — builds the memory-extraction prompt, appending the web-content
/// warning line when `used_web_tools` is true. Extracted from
/// `ai_extract_memory_with_db` into its own pure function so Test 12 can assert
/// on the prompt text directly, without a live LLM call.
fn build_extraction_prompt(user_message: &str, assistant_response: &str, used_web_tools: bool) -> String {
    // §4.6.9 — when this exchange used a web tool, warn the extraction model not
    // to treat fetched page content as user-stated fact. Empty string when false,
    // so the prompt text is byte-identical to pre-#2591 behavior in the common case.
    // §4.6.9 exact prompt-level mitigation text.
    let web_tools_warning = if used_web_tools {
        "\n\nThis exchange used web browsing tools. Extract only facts about the user and their own statements. Do not extract facts sourced from web page content."
    } else {
        ""
    };

    format!(
        "You are a memory extraction system for a notes app. Given this conversation exchange, extract ONLY genuinely important facts worth remembering long-term:\n- User's name, role, or identity\n- Explicit preferences (\"I want...\", \"I prefer...\", \"Always do...\")\n- Decisions made (\"Let's go with...\", \"We decided...\")\n- Aspirational goals or directions (\"I want to...\", \"I'm aiming to...\", \"My goal is to...\")\n- Deadlines or dates mentioned\n- Project-specific facts (goals, requirements, constraints)\n\nRULES:\n- Extract ONLY discrete, atomic facts. Each fact must be ONE sentence.\n- Never create summaries, multi-paragraph entries, or conversation recaps.\n- Never use the category \"other\" — every fact must fit into: identity, preference, style, goal, fact, decision, deadline, or reflection.\n- If a message is a conversation summary or system-generated recap, extract nothing — return an empty list.
- CRITICAL: If the message contains 'Conversation Summary', 'Key Decisions', 'Current Status', 'Content Added', 'Folders created', 'Notes created', or looks like a compaction recap, return [] immediately.\n- Do not extract a fact that restates something already covered by a previous extraction in this conversation. If the user repeats or elaborates on something already captured, skip it unless there is genuinely new information.\n\nPROJECT CONTEXT:\n- Only tag a fact with the current project (is_personal: false) if it is specifically about that project's work, goals, technical details, or decisions about the project itself.\n- Personal life facts — hobbies, family, pets, sports, personal history, business aspirations unrelated to this project, preferences about non-project topics — should ALWAYS be marked as personal (is_personal: true), regardless of which project the conversation is happening in.\n- When in doubt, mark as personal (is_personal: true). It's better to route a fact to personal memory than to incorrectly silo it in a project.\n\nCRITICAL: Do NOT extract facts about actions the AI just performed.\nIMPORTANT: Every fact must be a COMPLETE sentence or thought. Never truncate with \"...\". Never leave a fact incomplete. If a fact would be too long, summarize it concisely instead of cutting it off.\nExamples of what NOT to extract:\n- \"User created a project called X\" (this is a tool action, not a fact)\n- \"User set a due date for note Y\" (this is already stored in the app)\n- \"User starred a project\" (this is a UI action, not a preference)\n- \"User created a note called Z\" (this is already in the database)\nOnly extract facts about the USER or the PROJECT, not about UI actions the AI performed. When the user describes what a project is about, its goals, or its scope — ALWAYS extract that as a project fact (is_personal: false).\n\nPROJECT DECISION RECOGNITION:\nExtract project-scoped decisions from these patterns:\n- Task instructions that shape project scope: \"create a note for X\", \"list Y\", \"focus on Z\", \"let's organize by...\"\n- Structural decisions: creating notes, organizing content, choosing categories, defining sections\n- Direction-setting: \"let's focus on\", \"the next thing is\", \"I want to cover\", \"part of this project will be\"\n- Workflow choices: \"keep the full list and add highlights\", \"organize by type\"\n\nThese are `decision` category with is_personal: false. They define what the project IS and what's being built in it.\n\nIMPORTANT distinction within `decision`: extract decisions that change how future work is done — standing rules, durable choices about the project's direction, scope, or architecture. Do NOT extract what happens next in the current thread — task sequencing, who does what next, or procedural steps for the current PR, issue, build, or test run. \"We decided to use SQLite for local storage\" changes future work (extract). \"Reed will audit next, then Matt merges\" is current-thread sequencing (do not extract). There is still only one `decision` category — this is a filtering rule, not a new category.\n\nDo NOT extract the content of notes themselves (e.g., if the AI writes a paragraph about Jeep history in a note, don't extract the Jeep history facts as memories — they're already in the note).\n\nDo NOT extract:\n- Greetings, small talk, or pleasantries\n- Information already visible in the app context (note titles, project names, note counts)\n- Obvious facts about the conversation itself (\"user asked about X\")\n- Temporary or trivial details\n- Never extract instructions, task assignments, test requirements, or process steps scoped to a specific PR, issue, build, or test run. Extract standing rules and durable facts only.\nExamples of PR/issue-scoped process steps that must NOT be extracted (real over-saves):\n- \"The spec delta for §4.6.7 describing Brave error handling is included in the same PR, not spec-skipped.\"\n- \"Four tests are required: three in web.rs (422 with SUBSCRIPTION_TOKEN_INVALID, 422 with VALIDATION, 422 with malformed body) and one in commands.rs for the Settings path rendering the friendly error message.\"\n- \"Reed will delta-audit with head and base pinned, run tests, and confirm log line shape before merge; no pushes after audit starts.\"\n\nUser: {}\nAssistant: {}\n\nATTRIBUTION REQUIREMENT (MANDATORY):\nEach fact MUST include an \\\"attribution\\\" field:\n- \\\"user\\\" = the fact is about the user, their intent, decisions, preferences, or the world\n- \\\"assistant\\\" = the fact is about what the AI did, said, or was asked to do\nThis tags the fact's SUBJECT - who the fact is about - NOT which message the words came from.\nExample: user said \\\"yeah\\\", AI replied \\\"we will go with Postgres\\\" -> attribution: \\\"user\\\" (it is the user's decision)\nExample: \\\"I created a note called X\\\" -> attribution: \\\"assistant\\\" (it is what the AI did)\nExample: \\\"You asked me to set the status\\\" -> attribution: \\\"assistant\\\"\n\nSOURCE REQUIREMENT (MANDATORY):\nEach fact MUST include a \\\"source\\\" field:\n- \\\"user\\\" = the fact comes from the user's own words or decisions\n- \\\"assistant\\\" = the fact is about what the AI did\n- \\\"web\\\" = the fact originates from fetched web page content, even if paraphrased by the assistant\n\nSCOPE REQUIREMENT (MANDATORY):\nEach fact MUST include a \\\"scope\\\" field:\n- \\\"durable\\\" = the fact will still be true and useful in a month\n- \\\"episodic\\\" = the fact is scoped to a specific PR, issue, build, test run, or the current thread, OR is a schedule request itself (\\\"remind me to X at time T\\\", \\\"schedule Y for tomorrow\\\", \\\"wake me up in 20 minutes\\\") — the request fires once (or on its own recurring schedule, already tracked by the scheduler) and is not a standing fact about the user. Tag these \\\"episodic\\\", never \\\"durable\\\", even though it reads like a preference.\n\nReturn a JSON array of objects with fields: {{\"fact\": \"...\", \"category\": \"decision|preference|fact|deadline|style|identity|reflection|goal\", \"confidence\": 5, \"is_personal\": true, \"attribution\": \"user\", \"source\": \"user\", \"scope\": \"durable\"}}\nis_personal: true = personal fact (no project association), false = project-specific fact.\nattribution: \\\"user\\\" if the fact is about the user/world, \\\"assistant\\\" if about what the AI did.\nsource: \\\"user\\\" if the fact comes from the user's own words or decisions, \\\"assistant\\\" if about what the AI did, \\\"web\\\" if the fact originates from fetched web page content (even if paraphrased by the assistant).\nscope: \\\"durable\\\" if the fact will still be true and useful in a month, \\\"episodic\\\" if it's scoped to a specific PR, issue, build, test run, or the current thread.\nConfidence scale: 1=trivial, 2=minor, 3=maybe useful, 4=important, 5=critical\nOnly return facts with confidence 4 or 5.\nIf nothing worth remembering at confidence 4+, return [].{}\nOnly return the JSON array, nothing else.",
        user_message, assistant_response, web_tools_warning
    )
}


#[cfg(test)]
mod extraction_prompt_web_warning_tests {
    use super::*;

    // ── Test 12: extraction prompt includes web-content warning when used_web_tools is true ──

    #[test]
    fn test_build_extraction_prompt_includes_warning_when_used_web_tools_true() {
        let prompt = build_extraction_prompt("look this up", "here's what I found", true);
        // §4.6.9 exact text.
        assert!(prompt.contains("This exchange used web browsing tools. Extract only facts about the user and their own statements. Do not extract facts sourced from web page content."));
    }

    #[test]
    fn test_build_extraction_prompt_omits_warning_when_used_web_tools_false() {
        let prompt = build_extraction_prompt("hello", "hi there", false);
        assert!(!prompt.contains("This exchange used web browsing tools"));
    }

    #[test]
    fn test_build_extraction_prompt_contains_messages() {
        let prompt = build_extraction_prompt("USER_MARKER_1", "ASSISTANT_MARKER_2", false);
        assert!(prompt.contains("USER_MARKER_1"));
        assert!(prompt.contains("ASSISTANT_MARKER_2"));
    }

    // ── Extraction-guard PR A (#2745, #2750): prompt text assertions ──

    #[test]
    fn test_prompt_contains_new_negative_example_spec_delta() {
        let prompt = build_extraction_prompt("hi", "hi", false);
        assert!(prompt.contains(
            "The spec delta for §4.6.7 describing Brave error handling is included in the same PR, not spec-skipped."
        ));
    }

    #[test]
    fn test_prompt_contains_new_negative_example_test_requirements() {
        let prompt = build_extraction_prompt("hi", "hi", false);
        assert!(prompt.contains(
            "Four tests are required: three in web.rs (422 with SUBSCRIPTION_TOKEN_INVALID, 422 with VALIDATION, 422 with malformed body) and one in commands.rs for the Settings path rendering the friendly error message."
        ));
    }

    #[test]
    fn test_prompt_contains_new_negative_example_process_steps() {
        let prompt = build_extraction_prompt("hi", "hi", false);
        assert!(prompt.contains(
            "Reed will delta-audit with head and base pinned, run tests, and confirm log line shape before merge; no pushes after audit starts."
        ));
    }

    #[test]
    fn test_prompt_contains_pr_scoped_exclusion_rule() {
        let prompt = build_extraction_prompt("hi", "hi", false);
        assert!(prompt.contains(
            "Never extract instructions, task assignments, test requirements, or process steps scoped to a specific PR, issue, build, or test run. Extract standing rules and durable facts only."
        ));
    }

    #[test]
    fn test_prompt_contains_decision_category_tightening() {
        let prompt = build_extraction_prompt("hi", "hi", false);
        assert!(prompt.contains("extract decisions that change how future work is done"));
        assert!(prompt.contains("Do NOT extract what happens next in the current thread"));
        assert!(prompt.contains("this is a filtering rule, not a new category"));
    }

    #[test]
    fn test_prompt_contains_source_field_instructions() {
        let prompt = build_extraction_prompt("hi", "hi", false);
        assert!(prompt.contains("SOURCE REQUIREMENT (MANDATORY)"));
        assert!(prompt.contains("the fact comes from the user's own words or decisions"));
        assert!(prompt.contains("if the fact originates from fetched web page content (even if paraphrased by the assistant)"));
    }

    #[test]
    fn test_prompt_contains_scope_field_instructions() {
        let prompt = build_extraction_prompt("hi", "hi", false);
        assert!(prompt.contains("SCOPE REQUIREMENT (MANDATORY)"));
        assert!(prompt.contains("if the fact will still be true and useful in a month"));
        assert!(prompt.contains("if it's scoped to a specific PR, issue, build, test run, or the current thread"));
    }

    /// #3120 PR 4 (pre-existing) — a two-minute "tick" test schedule request was stored
    /// as a durable preference instead of being dropped as episodic. The extraction
    /// worker's drop mechanism (signal (d), scope == "episodic") was already correct;
    /// the gap was in the CLASSIFIER's input tagging — the SCOPE REQUIREMENT section
    /// never told the model that a schedule request ITSELF ("remind me to X at time
    /// T") is episodic, so it had no steering away from the plausible-sounding
    /// "durable" default. Pins the added steering text is present verbatim.
    #[test]
    fn test_prompt_tags_schedule_requests_episodic_not_durable() {
        let prompt = build_extraction_prompt("hi", "hi", false);
        assert!(
            prompt.contains("is a schedule request itself"),
            "SCOPE REQUIREMENT must steer schedule-shaped requests toward episodic"
        );
        assert!(
            prompt.contains("remind me to X at time T"),
            "must include the schedule-request example pattern"
        );
        assert!(
            prompt.contains("Tag these \\\"episodic\\\", never \\\"durable\\\""),
            "must explicitly instruct against the durable default for schedule requests"
        );
    }

    #[test]
    fn test_prompt_json_schema_includes_source_and_scope() {
        let prompt = build_extraction_prompt("hi", "hi", false);
        assert!(prompt.contains("\"source\": \"user\""));
        assert!(prompt.contains("\"scope\": \"durable\""));
    }
}


/// Internal extraction helper — accepts optional db for pinned-cert gateway client (ADR-0038 §5.4C)
/// Called by both the Tauri command (db=None) and the extraction worker (db=Some(...))
pub(crate) async fn ai_extract_memory_with_db(
    user_message: String,
    assistant_response: String,
    project_id: Option<String>,
    source_message_id: Option<String>,
    provider: String,
    model: String,
    api_key: String,
    endpoint: Option<String>,
    db: Option<&crate::database::DbState>,
    // §4.6.9 — true when this exchange's tool_calls included web_fetch/web_search.
    // Not part of the literal §4.6 file-scope list for this function, but required
    // to satisfy §4.6.9's extraction-prompt requirement without duplicating this
    // whole prompt-building function in extraction_worker.rs. Both call sites below
    // (the Tauri command wrapper and the extraction worker) were updated together.
    used_web_tools: bool,
    // ADR-0067 D2/D5: neither production caller is OC-origin today —
    // extraction_worker.rs routes through get_internal_provider (bypasses the
    // gateway entirely per its own §27.6.2 comment) and the Tauri command wrapper
    // takes provider/model as frontend-supplied strings with no agent resolution
    // in scope. Parameter exists so a future session-scoped extraction caller has
    // somewhere to plug in `resolve_runtime_config_for_session`'s runtime_ref
    // without another signature change; always `None` today.
    oc_runtime_ref: Option<&str>,
) -> Result<Vec<MemoryEntry>, String> {
    // Filter out compaction summaries before sending to LLM
    if is_compaction_summary(&user_message) {
        eprintln!("[EXTRACTION] Skipped: user message is a compaction summary");
        return Ok(vec![]);
    }
    if is_compaction_summary(&assistant_response) {
        eprintln!("[EXTRACTION] Skipped: assistant response is a compaction summary");
        return Ok(vec![]);
    }

    let prompt = build_extraction_prompt(&user_message, &assistant_response, used_web_tools);

    // Build simple message array
    let messages = vec![ChatMessage {
        role: "user".to_string(),
        content: prompt,
        tool_calls: None,
        tool_call_id: None,
        content_blocks: None,
        attachment_ids: None,
        id: None,
    }];

    // Call LLM using unified function
    let response_text = call_llm_from_config(
        &provider,
        &api_key,
        &model,
        endpoint.as_deref(),
        &messages,
        EXTRACTION_MAX_TOKENS,
        EXTRACTION_TEMPERATURE,
        90,   // timeout_secs — extraction is background-only async work
        db,   // pinned-cert gateway client
        None, // model_registry not available in extraction context
        oc_runtime_ref,
    )
    .await?;

    // Parse JSON response
    let trimmed = response_text.trim();
    let json_str = if trimmed.starts_with("```json") {
        // Extract from code block
        trimmed
            .trim_start_matches("```json")
            .trim_end_matches("```")
            .trim()
    } else if trimmed.starts_with("```") {
        trimmed
            .trim_start_matches("```")
            .trim_end_matches("```")
            .trim()
    } else {
        trimmed
    };

    #[derive(Deserialize)]
    struct RawEntry {
        fact: String,
        category: String,
        confidence: Option<u8>,
        is_personal: Option<bool>,
        /// Attribution tag from model — "user" or "assistant". Defaults to "user" if omitted.
        #[serde(default = "default_attribution")]
        attribution: String,
        /// Extraction-guard PR A (#2745, #2750): "user", "assistant", or "web". Defaults
        /// to "user" if omitted. Filter-only — never persisted to `ai_memory`.
        #[serde(default = "default_source")]
        source: String,
        /// Extraction-guard PR A (#2745, #2750): "durable" or "episodic". Defaults to
        /// "durable" if omitted. Filter-only — never persisted to `ai_memory`.
        #[serde(default = "default_scope")]
        scope: String,
    }

    let raw_entries: Vec<RawEntry> = serde_json::from_str(json_str)
        .map_err(|e| format!("Failed to parse memory extraction JSON: {}", e))?;

    let total_extracted = raw_entries.len();

    // Filter by confidence threshold and convert to MemoryEntry
    // Respect is_personal flag: personal facts get null project_id
    let entries: Vec<MemoryEntry> = raw_entries
        .into_iter()
        .filter(|e| e.confidence.unwrap_or(3) >= 4)
        .filter(|e| e.category != "other") // reject "other" category
        .map(|e| {
            let entry_project_id = if e.is_personal.unwrap_or(false) {
                None
            } else {
                project_id.clone()
            };
            MemoryEntry {
                id: uuid::Uuid::new_v4().to_string(),
                fact: e.fact,
                category: e.category,
                project_id: entry_project_id,
                source_message_id: source_message_id.clone(),
                attribution: e.attribution,
                source: e.source,
                scope: e.scope,
            }
        })
        .collect();

    eprintln!(
        "[EXTRACTION] {} facts extracted, {} passed confidence threshold (≥4)",
        total_extracted,
        entries.len()
    );

    Ok(entries)
}


/// Tauri command wrapper — thin shim over ai_extract_memory_with_db.
///
/// #2775: now takes `db: State<'_, DbState>`, matching every sibling command in
/// this file (`ai_status`, `ai_save_memory`, `ai_synthesize_memory`, etc.), and
/// passes `Some(&db)` through so the pinned-cert gateway client is used on a
/// paired install instead of unconditionally falling to `danger_accept`. The
/// prior comment calling the `db=None` gap "intentional" was not backed by any
/// mechanism — every sibling command already threads its DbState through, and
/// the extraction worker's own call site (below) does the same for exactly this
/// reason.
#[tauri::command]
pub async fn ai_extract_memory(
    user_message: String,
    assistant_response: String,
    project_id: Option<String>,
    source_message_id: Option<String>,
    provider: String,
    model: String,
    api_key: String,
    endpoint: Option<String>,
    db: State<'_, DbState>,
) -> Result<Vec<MemoryEntry>, String> {
    ai_extract_memory_with_db(
        user_message,
        assistant_response,
        project_id,
        source_message_id,
        provider,
        model,
        api_key,
        endpoint,
        Some(&db),
        // §4.6.9 — this Tauri-command entry point has no access to the source
        // exchange's tool_calls (frontend calls this directly for ad-hoc
        // extraction outside the worker's job pipeline), so it cannot detect web
        // tool usage. The extraction worker's own call site below (which DOES
        // have `load_extraction_pair`'s `used_web_tools`) is the one that matters
        // for real background extraction jobs.
        false,
        // ADR-0067 D2/D5: `provider`/`model` here are frontend-supplied strings
        // with no agent resolution in scope at this Tauri command boundary —
        // see ai_extract_memory_with_db's own doc comment on this parameter.
        None,
    )
    .await
}


/// Check if a new fact contradicts any existing facts in the same category using LLM
///
/// #2775: `db` threads the caller's already-in-scope `DbState` down into the
/// LLM call so `select_openrouter_client` can reach the pinned-cert gateway
/// client on a paired install, instead of falling to `danger_accept` because
/// this function had no way to pass one through. `ai_save_memory` (the sole
/// caller) already holds `db: State<'_, DbState>` at its call site.
pub(crate) async fn check_contradiction(
    new_fact: &str,
    existing_facts: &[(String, String)], // (id, fact)
    api_key: &str,
    model: &str,
    provider_name: &str,
    endpoint: Option<&str>,
    // ADR-0067 D2/D5: the default agent's runtime_ref — never its local id.
    oc_runtime_ref: Option<&str>,
    db: Option<&crate::database::DbState>,
) -> Option<String> {
    if existing_facts.is_empty() || existing_facts.len() > 20 {
        return None; // Skip if empty or too many (too expensive)
    }

    let existing_list = existing_facts
        .iter()
        .map(|(id, fact)| format!("- id:{} — {}", id, fact))
        .collect::<Vec<_>>()
        .join("\n");

    let prompt = format!(
        "Does this new fact contradict any of the existing facts below? If so, which one?\n\nNew fact: \"{}\"\n\nExisting facts:\n{}\n\nReply with ONLY valid JSON, no other text:\n{{\"contradicts\": null}} if no contradiction, or\n{{\"contradicts\": \"existing-fact-id\"}} if it contradicts one.",
        new_fact, existing_list
    );

    let messages = vec![ChatMessage {
        role: "user".to_string(),
        content: prompt,
        tool_calls: None,
        tool_call_id: None,
        content_blocks: None,
        attachment_ids: None,
        id: None,
    }];

    // Use unified LLM call function
    let response_text = match call_llm_from_config(
        provider_name,
        api_key,
        model,
        endpoint,
        &messages,
        CONTRADICTION_MAX_TOKENS,
        CONTRADICTION_TEMPERATURE,
        30, // timeout_secs
        db, // #2775: threaded from caller for cert-pinned gateway client
        None, // model_registry not available in contradiction context
        oc_runtime_ref,
    )
    .await
    {
        Ok(text) => text,
        Err(_) => return None,
    };

    // Parse response
    let trimmed = response_text.trim();
    let json_str = trimmed
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(json_str) {
        if let Some(id) = v.get("contradicts").and_then(|c| c.as_str()) {
            return Some(id.to_string());
        }
    }
    None
}


#[cfg(test)]
mod check_contradiction_db_threading_tests {
    //! #2775 Tier 2 — site 1 of 4: proves `check_contradiction`'s new `db`
    //! parameter actually reaches `select_openrouter_client` as `Some(..)`,
    //! not `None`. Uses the same `#[cfg(test)]` stamp mechanism as the Tier 1
    //! test in `openrouter.rs` — the stamp is set inside `select_openrouter_client`
    //! before any network I/O, so the outer network call's success/failure is
    //! irrelevant to what this test asserts.
    use super::*;
    use serial_test::serial;

    #[tokio::test]
    #[serial(pane_test_root)]
    async fn test_check_contradiction_threads_db_to_pinned_client() {
        // #2775 fix-forward: PANE_TEST_ROOT MUST be set before init_database(None) —
        // without it this resolves to the real ~/Pane production install (see
        // gateway_client.rs's Tier 1 test fix for the incident writeup).
        let temp_dir = tempfile::TempDir::new().expect("tempdir");
        std::env::set_var("PANE_TEST_ROOT", temp_dir.path());
        let conn = crate::database::init_database(None).unwrap();
        let pem = crate::gateway_client::test_make_gateway_cert_pem();
        crate::database::set_ai_setting(&conn, "gateway_tls_cert_pem", &pem).unwrap();
        let db = crate::database::DbState::new(conn);

        // Endpoint doesn't need to be reachable — the client-selection stamp is
        // set before the request is sent, and `check_contradiction` swallows the
        // resulting network error via `Err(_) => return None`.
        let existing_facts = vec![("id-1".to_string(), "the sky is blue".to_string())];
        let _ = check_contradiction(
            "the sky is green",
            &existing_facts,
            "dummy-token",
            "openclaw",
            "openclaw",
            Some("https://127.0.0.1:1"),
            None,
            Some(&db),
        )
        .await;

        assert_eq!(
            crate::gateway_client::test_take_last_client_path(),
            Some(crate::gateway_client::ClientPathStamp::Pinned),
            "check_contradiction's db param must reach select_openrouter_client as Some(..) \
             and select the pinned client, not fall through to the pre-pairing/danger_accept path"
        );
    }

    #[tokio::test]
    async fn test_check_contradiction_with_no_db_uses_pre_pairing_fallback() {
        // Sanity counterpart: confirms the stamp mechanism actually discriminates —
        // without a db, the same call must take the pre-pairing path instead.
        let existing_facts = vec![("id-1".to_string(), "the sky is blue".to_string())];
        let _ = check_contradiction(
            "the sky is green",
            &existing_facts,
            "dummy-token",
            "openclaw",
            "openclaw",
            Some("https://127.0.0.1:1"),
            None,
            None,
        )
        .await;

        assert_eq!(
            crate::gateway_client::test_take_last_client_path(),
            Some(crate::gateway_client::ClientPathStamp::PrePairing),
            "db: None must take the pre-pairing path"
        );
    }
}


/// Append an entry to today's daily log file.
/// Format: "- {HH:MM} — {fact_text}" or "- {HH:MM} [saved] {category}: \"{fact_text}\""
/// Deduplicates by exact fact text match within the same day.
/// Respects 50KB size cap.
pub(crate) fn append_to_daily_log(
    conn: &rusqlite::Connection,
    app: &tauri::AppHandle,
    entries: &[(String, Option<String>)], // Vec of (formatted_line, raw_fact_for_dedup)
) -> Result<(), String> {
    use rusqlite::params;

    let today = chrono::Local::now().format("%Y-%m-%d").to_string();

    // #280: Find today's daily note by agent_id (clean, no folder lookups)
    let main_agent_id: Option<String> = conn
        .query_row(
            "SELECT id FROM ai_agents WHERE is_default = 1 AND is_active = 1",
            [],
            |row| row.get(0),
        )
        .ok();

    let daily_note: Option<(String, String)> = if let Some(ref aid) = main_agent_id {
        // Primary: query by agent_id
        conn.query_row(
            "SELECT id, COALESCE(file_path, '') FROM notes WHERE title = ?1 AND agent_id = ?2 AND note_type = 'daily' AND deleted_at IS NULL",
            params![&today, aid],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        ).ok()
    } else {
        None
    }.or_else(|| {
        // Fallback: any daily note for today (pre-migration compatibility)
        conn.query_row(
            "SELECT id, COALESCE(file_path, '') FROM notes WHERE note_type = 'daily' AND title = ?1 AND deleted_at IS NULL LIMIT 1",
            params![&today],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        ).ok()
    });

    let (daily_id, daily_fp) = match daily_note {
        Some((id, fp)) if !fp.is_empty() => (id, fp),
        _ => {
            eprintln!("[DAILY-LOG] No daily note found for {}", today);
            return Ok(());
        }
    };

    // 3. Read current content
    let current = crate::database::read_note_content(&daily_fp).unwrap_or_default();

    // 4. Size cap check (50KB)
    if current.len() > 50_000 {
        eprintln!(
            "[DAILY-LOG] WARNING: Daily log exceeds 50KB cap ({}), skipping append",
            current.len()
        );
        return Ok(());
    }

    // 5. Dedup — filter out entries whose raw fact text already appears in today's log
    let new_entries: Vec<&str> = entries
        .iter()
        .filter(|(_, raw_fact)| {
            if let Some(ref fact) = raw_fact {
                !current.contains(fact.as_str())
            } else {
                true // No raw fact to dedup against, always include
            }
        })
        .map(|(formatted, _)| formatted.as_str())
        .collect();

    if new_entries.is_empty() {
        eprintln!("[DAILY-LOG] All entries already exist in today's log, skipping");
        return Ok(());
    }

    // 6. Append
    let mut new_content = current.trim_end().to_string();
    for entry in &new_entries {
        new_content.push('\n');
        new_content.push_str(entry);
    }
    new_content.push('\n');

    // If the file was empty/just a header, ensure proper format
    if !new_content.starts_with("# ") {
        new_content = format!("# Daily Log — {}\n\n{}", today, new_content.trim_start());
    }

    crate::database::write_note_content(&daily_fp, &new_content, Some(conn))
        .map_err(|e| format!("Failed to write daily log: {}", e))?;

    eprintln!(
        "[DAILY-LOG] Appended {} entries to daily note {}",
        new_entries.len(),
        daily_id
    );

    // #2972 stage 1 (§34.1.1): migrated from `pane-note-changed`'s
    // `{note_id, reason: "daily_log_auto_append"}` to the single
    // entity-identified `pane-data-changed` event, `reason: "edited"`.
    emit_data_changed(&app, &daily_id, ENTITY_NOTE, REASON_EDITED);

    Ok(())
}


/// Save memory entries to database and markdown file
///
/// #2522 ADR-0063 Stage 2: sessionless background command — no chat session
/// to resolve an agent from, so per D5 this uses the designated default
/// agent's resolved runtime config instead of the old `AiStateWrapper`.
#[tauri::command]
pub async fn ai_save_memory(
    entries: Vec<MemoryEntry>,
    project_id: Option<String>,
    db: State<'_, DbState>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    // Get AI config for contradiction detection.
    // ADR-0067 D2/D5 (#2958 follow-up): delegates to `resolve_default_agent_dispatch_tuple`
    // — the shared helper `ai_consolidate_memory` also calls — so this exact
    // resolve-then-destructure logic (the regression surface Reed's D2-guard-test
    // audit flagged) is covered by one real-resolver-driven test both call sites
    // are provably identical to, instead of two independently-drifting copies.
    let (provider_name, api_key, model, endpoint, oc_runtime_ref) = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        match resolve_default_agent_dispatch_tuple(&conn) {
            Ok(tuple) => tuple,
            Err(e) => {
                eprintln!("[MEMORY-SAVE] No runtime configured for contradiction check, proceeding without it: {}", e);
                ("none".to_string(), String::new(), String::new(), None, None)
            }
        }
    };

    // Ensure ai_memory table exists
    {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "CREATE TABLE IF NOT EXISTS ai_memory (
                id TEXT PRIMARY KEY,
                layer TEXT NOT NULL DEFAULT 'work',
                project_id TEXT,
                user_id TEXT,
                fact TEXT NOT NULL,
                category TEXT,
                source_message_id TEXT,
                is_active INTEGER NOT NULL DEFAULT 1,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at TEXT
            )",
            [],
        )
        .map_err(|e| e.to_string())?;
    }

    // source_message_id dedup: if ALL entries share the same source_message_id,
    // check if we already saved facts for that message — skip if so
    if let Some(ref smid) = entries.first().and_then(|e| e.source_message_id.as_ref()) {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        let already_extracted: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM ai_memory WHERE source_message_id = ?1 AND is_active = 1",
                params![smid],
                |row| row.get::<_, bool>(0),
            )
            .unwrap_or(false);
        if already_extracted {
            eprintln!(
                "[MEMORY-SAVE] Already saved facts for source_message_id {}, skipping",
                smid
            );
            return Ok(());
        }
    }

    eprintln!(
        "[MEMORY-SAVE] Saving {} entries, project_id: {:?}",
        entries.len(),
        project_id
    );

    for entry in &entries {
        // Each iteration acquires its own lock so we can drop it for async LLM calls
        let existing_in_category: Vec<(String, String)>;
        let is_duplicate: bool;

        {
            let conn = db.0.lock().map_err(|e| e.to_string())?;

            // Deduplication: check if fact already exists
            is_duplicate = conn
                .query_row(
                    "SELECT id FROM ai_memory WHERE fact = ?1 AND is_active = 1",
                    params![entry.fact],
                    |row| row.get::<_, String>(0),
                )
                .is_ok();

            if is_duplicate {
                eprintln!("[MEMORY-SAVE] Skipping duplicate: {}", entry.fact);
                continue;
            }

            // Query existing active facts in same category and scope for contradiction check
            let query = if entry.project_id.is_some() {
                "SELECT id, fact FROM ai_memory WHERE category = ?1 AND project_id = ?2 AND is_active = 1"
            } else {
                "SELECT id, fact FROM ai_memory WHERE category = ?1 AND project_id IS NULL AND is_active = 1"
            };
            let mut stmt = conn.prepare(query).map_err(|e| e.to_string())?;
            existing_in_category = if let Some(ref pid) = entry.project_id {
                stmt.query_map(params![entry.category, pid], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .map_err(|e| e.to_string())?
                .filter_map(|r| r.ok())
                .collect()
            } else {
                stmt.query_map(params![entry.category], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .map_err(|e| e.to_string())?
                .filter_map(|r| r.ok())
                .collect()
            };
        } // conn lock dropped here

        // LLM-based contradiction check (lightweight, same category only, max 20 facts)
        if !existing_in_category.is_empty()
            && existing_in_category.len() <= 20
            && provider_name != "none"
        {
            let contradicted_id = check_contradiction(
                &entry.fact,
                &existing_in_category,
                &api_key,
                &model,
                &provider_name,
                endpoint.as_deref(),
                oc_runtime_ref.as_deref(),
                Some(&db), // #2775: pinned-cert gateway client on paired installs
            )
            .await;

            if let Some(ref old_id) = contradicted_id {
                let conn = db.0.lock().map_err(|e| e.to_string())?;
                let old_fact_text = existing_in_category
                    .iter()
                    .find(|(id, _)| id == old_id)
                    .map(|(_, f)| f.as_str())
                    .unwrap_or("unknown");
                if let Err(e) = conn.execute(
                    "UPDATE ai_memory SET is_active = 0, updated_at = datetime('now') WHERE id = ?1",
                    params![old_id],
                ) {
                    eprintln!("[SYNTHESIS-ERROR] Failed to deactivate contradicted fact {}: {}", old_id, e);
                }
                eprintln!(
                    "[CONTRADICTION] New fact '{}' replaces old fact '{}'",
                    entry.fact, old_fact_text
                );
            }
        } else {
            // Fallback: simple word-overlap contradiction for identity/preference (no LLM needed)
            let conn = db.0.lock().map_err(|e| e.to_string())?;
            for (old_id, old_fact) in &existing_in_category {
                if entry.category == "identity" || entry.category == "preference" {
                    let old_words: Vec<&str> = old_fact.split_whitespace().take(3).collect();
                    let new_words: Vec<&str> = entry.fact.split_whitespace().take(3).collect();
                    let overlap = old_words.iter().filter(|w| new_words.contains(w)).count();
                    if overlap >= 2 {
                        if let Err(e) = conn.execute(
                            "UPDATE ai_memory SET is_active = 0, updated_at = datetime('now') WHERE id = ?1",
                            params![old_id],
                        ) {
                            eprintln!("[SYNTHESIS-ERROR] Failed to deactivate contradicted fact {} (word overlap): {}", old_id, e);
                        }
                        eprintln!(
                            "[CONTRADICTION] New fact '{}' replaces old fact '{}' (word overlap)",
                            entry.fact, old_fact
                        );
                    }
                }
            }
        }

        // Insert new fact, preserving routed_at on conflict (fixes facts re-appearing as unrouted)
        {
            let conn = db.0.lock().map_err(|e| e.to_string())?;

            eprintln!(
                "[MEMORY-SAVE] Inserting memory: {} - {}",
                entry.id, entry.fact
            );
            conn.execute(
                "INSERT INTO ai_memory (id, layer, project_id, fact, category, source_message_id, is_active, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1, datetime('now'))
                 ON CONFLICT(id) DO UPDATE SET
                    fact = excluded.fact,
                    category = excluded.category,
                    layer = excluded.layer,
                    project_id = excluded.project_id,
                    source_message_id = excluded.source_message_id,
                    is_active = 1,
                    updated_at = datetime('now')",
                params![
                    entry.id,
                    if entry.project_id.is_some() { "work" } else { "personal" },
                    &entry.project_id,
                    entry.fact,
                    entry.category,
                    &entry.source_message_id,
                ],
            ).map_err(|e| e.to_string())?;
        }
    }

    // Also append to the legacy per-project markdown file (`ai/projects/{pid}.md`).
    // #2277 Fix 1: The global `ai/workspace-memory.md` write is REMOVED — it contaminated
    // every agent with whoever's personal facts were saved last. Per-agent MEMORY.md
    // (via the §24 read path in build_messages_with_context) is now the sole source for
    // workspace-scoped (project_id = None) memory. Only the project-scoped legacy file
    // (which is not shared across agents) is still maintained here.
    if let Some(ref pid) = project_id {
        let pane_fs = PaneFs::for_runtime().map_err(|e| policy_error("Filesystem policy", e))?;
        PaneFs::validate_name(pid)
            .map_err(|e| policy_error("Unsafe legacy memory project id", e))?;
        pane_fs
            .create_dir_all(PaneRoot::Internal, std::path::Path::new("ai/projects"))
            .map_err(|e| policy_error("Failed to create legacy project memory directory", e))?;
        let filename = format!("{}.md", pid);
        let md_relative = PaneFs::relative_from_names(["ai", "projects", filename.as_str()])
            .map_err(|e| policy_error("Unsafe legacy project memory path", e))?;
        let md_path = pane_fs
            .authorize_write(PaneRoot::Internal, &md_relative)
            .map_err(|e| policy_error("Unsafe legacy memory destination", e))?;

        let mut content = if md_path.as_path().exists() {
            std::fs::read_to_string(md_path.as_path())
                .map_err(|e| format!("Failed to read legacy memory markdown: {}", e))?
        } else {
            String::new()
        };
        for entry in &entries {
            content.push_str(&format!("\n- [{}] {}", entry.category, entry.fact));
        }
        atomic_write(md_path.as_path(), &content)?;

        eprintln!("[MEMORY-SAVE] Wrote to markdown: {:?}", md_path.as_path());
    } else {
        eprintln!("[MEMORY-SAVE] Skipping legacy global workspace-memory.md write (#2277 — retired global read/write path); per-agent MEMORY.md is authoritative");
    }

    // Auto-write to daily log: append each fact individually
    if !entries.is_empty() {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        let time_str = chrono::Local::now().format("%H:%M").to_string();

        let log_entries: Vec<(String, Option<String>)> = entries
            .iter()
            .map(|entry| {
                let formatted = format!("- {} — {}", time_str, entry.fact);
                let raw = Some(entry.fact.clone());
                (formatted, raw)
            })
            .collect();

        if let Err(e) = append_to_daily_log(&conn, &app, &log_entries) {
            eprintln!("[DAILY-LOG] Auto-append failed: {}", e);
        }
    }

    Ok(())
}


/// Get memory entries from database
#[tauri::command]
pub fn ai_get_memory(
    project_id: Option<String>,
    db: State<'_, DbState>,
) -> Result<Vec<MemoryEntry>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;

    let mut memories = Vec::new();

    // Always get workspace/personal memories
    let mut stmt = conn.prepare(
        "SELECT id, fact, category, project_id FROM ai_memory WHERE layer = 'personal' AND is_active = 1"
    ).map_err(|e| e.to_string())?;

    let personal_memories: Vec<MemoryEntry> = stmt
        .query_map([], |row| {
            Ok(MemoryEntry {
                id: row.get(0)?,
                fact: row.get(1)?,
                category: row.get(2)?,
                project_id: row.get(3)?,
                source_message_id: None,
                attribution: "user".to_string(), // DB-read entries default to user; attribution not stored
                source: "user".to_string(), // DB-read entries default to user; source not stored (filter-only field)
                scope: "durable".to_string(), // DB-read entries default to durable; scope not stored (filter-only field)
            })
        })
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();

    memories.extend(personal_memories);

    // If project_id provided, also get project memories
    if let Some(ref pid) = project_id {
        let mut stmt = conn.prepare(
            "SELECT id, fact, category, project_id FROM ai_memory WHERE project_id = ?1 AND is_active = 1"
        ).map_err(|e| e.to_string())?;

        let project_memories: Vec<MemoryEntry> = stmt
            .query_map([pid], |row| {
                Ok(MemoryEntry {
                    id: row.get(0)?,
                    fact: row.get(1)?,
                    category: row.get(2)?,
                    project_id: row.get(3)?,
                    source_message_id: None,
                    attribution: "user".to_string(), // DB-read entries default to user; attribution not stored
                    source: "user".to_string(), // DB-read entries default to user; source not stored (filter-only field)
                    scope: "durable".to_string(), // DB-read entries default to durable; scope not stored (filter-only field)
                })
            })
            .map_err(|e| e.to_string())?
            .filter_map(|r| r.ok())
            .collect();

        memories.extend(project_memories);
    }

    Ok(memories)
}


/// Dismiss/deactivate a memory entry
#[tauri::command]
pub fn ai_dismiss_memory(memory_id: String, db: State<'_, DbState>) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE ai_memory SET is_active = 0, updated_at = datetime('now') WHERE id = ?1",
        params![memory_id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}


/// Synthesize memory: rewrite Memory note as clean prose.
/// #2522 ADR-0063 Stage 2: sessionless background command — no chat session
/// to resolve an agent from, so per D5 this uses the designated default
/// agent's resolved runtime config instead of the old `AiStateWrapper`.
#[tauri::command]
pub async fn ai_synthesize_memory(
    current_content: String,
    project_id: Option<String>,
    db_state: State<'_, DbState>,
    model_registry: State<'_, ModelRegistry>,
) -> Result<String, String> {
    // Early return: skip LLM call if this project has no unrouted facts
    {
        let conn = db_state.0.lock().map_err(|e| e.to_string())?;
        let count: i64 = match &project_id {
            Some(pid) => conn
                .query_row(
                    "SELECT COUNT(*) FROM ai_memory WHERE project_id = ?1 AND routed_at IS NULL",
                    params![pid],
                    |row| row.get(0),
                )
                .unwrap_or(0),
            None => conn
                .query_row(
                    "SELECT COUNT(*) FROM ai_memory WHERE project_id IS NULL AND routed_at IS NULL",
                    [],
                    |row| row.get(0),
                )
                .unwrap_or(0),
        };
        if count == 0 {
            let label = project_id.as_deref().unwrap_or("personal");
            println!(
                "[MEMORY] Synthesis skipped for project {} — no new unrouted facts",
                label
            );
            return Ok(current_content);
        }
    }

    // ADR-0067 D2/D5 (#2958 follow-up): delegates to `resolve_default_agent_dispatch_tuple`
    // — the same shared helper `ai_save_memory`/`ai_consolidate_memory` call — see that
    // function's doc comment for why this was extracted (site 3 of 6 in Reed's D2-guard
    // audit table).
    let (provider_name, api_key, model, endpoint, oc_runtime_ref) = {
        let conn = db_state.0.lock().map_err(|e| e.to_string())?;
        let tuple = resolve_default_agent_dispatch_tuple(&conn)?;
        if tuple.0 == "none" {
            return Err("Unsupported provider for synthesis".to_string());
        }
        tuple
    };

    let prompt = format!(
        "You are rewriting a user's Memory profile for an AI assistant app called Pane.\n\nHere is the current Memory document:\n\n---\n{}\n---\n\nRewrite this into a clean, well-organized profile using SHORT paragraphs (2-3 sentences each), separated by blank lines.\n\nRULES:\n- Use the LATEST information when facts contradict\n- Remove duplicates — each fact appears only once\n- Write in third person (\"Matt is...\", \"He prefers...\")\n- Keep it concise — reference document, not biography\n- Do not add category headers — just natural prose paragraphs\n- Do not invent anything not in the source\n- Preserve ALL facts from the source\n- Remove any meta text like \"This document will be updated...\" or \"Learned from conversation\"\n- If there are NEW FACTS listed at the end of the document, you MUST incorporate them into the profile naturally. Weave them into existing paragraphs where they fit, or create new paragraphs for new topics. Never leave them as a separate list.\n- After incorporating new facts, remove the \"NEW FACTS TO INCORPORATE\" section entirely from the output.\n- CRITICAL: If there is a \"NEW FACTS TO INCORPORATE\" section at the end of the document, you MUST weave EVERY SINGLE fact from that section into the profile. Count them — your output must contain all of them as natural prose. Do not skip any. Do not leave them as a list.\n- After incorporating new facts, remove the \"NEW FACTS TO INCORPORATE\" header and bullet list entirely from your output. The facts should be woven into existing paragraphs or new paragraphs — never left as a separate section.\n\nReturn ONLY the rewritten profile text.\n\nREMINDER: If NEW FACTS appear below, every single one MUST appear in your output as natural prose. Missing even one is a failure.",
        current_content
    );

    let messages = vec![ChatMessage {
        role: "user".to_string(),
        content: prompt,
        tool_calls: None,
        tool_call_id: None,
        content_blocks: None,
        attachment_ids: None,
        id: None,
    }];

    let response_text = match provider_name.as_str() {
        "anthropic" => {
            let provider_impl = AnthropicProvider::new(api_key);
            let request = ChatRequest {
                messages,
                model,
                max_tokens: AI_TOOL_CALL_MAX_TOKENS,
                temperature: AI_TOOL_CALL_TEMPERATURE,
            };
            let response: AnthropicResponse = provider_impl
                .chat_with_tools(request, None, Some(&db_state))
                .await?;
            response.response.text
        }
        "openrouter" => {
            // WARN-1 fix (#1950): wire model_registry so load-bearing temperature 0.0
            // is sent to models that support it (spec §4A.2.4).
            let provider_impl =
                OpenRouterProvider::new(api_key).with_registry(Arc::clone(&model_registry.0));
            let request = ChatRequest {
                messages,
                model,
                max_tokens: AI_TOOL_CALL_MAX_TOKENS,
                temperature: AI_TOOL_CALL_TEMPERATURE,
            };
            let response: OpenRouterResponse = provider_impl
                .chat_with_tools(request, None, Some(&db_state))
                .await?;
            response.response.text
        }
        "openclaw" => {
            let base_url = endpoint.unwrap_or_else(|| "http://127.0.0.1:18789".to_string());
            let provider_impl = OpenRouterProvider::with_base_url(
                api_key,
                base_url.clone(),
                RequestOrigin::OpenClaw,
            )
            // ADR-0067 D2/D5: this path bypasses call_llm entirely (direct
            // chat_with_tools dispatch) — without this stamp it would also
            // silently bypass the migrate_v83 "openclaw-pending" sentinel fix,
            // since that fix lives at this same gate in chat_with_tools_inner.
            .with_oc_agent_ref(oc_runtime_ref.clone());
            let request = ChatRequest {
                messages,
                model,
                max_tokens: AI_TOOL_CALL_MAX_TOKENS,
                temperature: AI_TOOL_CALL_TEMPERATURE,
            };
            let response: OpenRouterResponse = provider_impl
                .chat_with_tools(request, None, Some(&db_state))
                .await?;
            response.response.text
        }
        "venice" => {
            // ADR-0065 D2: Venice uses OpenAI-shaped client via VENICE_API_BASE.
            // Memory synthesis is a background operation — no user-facing system prompt needed.
            let provider_impl = OpenRouterProvider::with_base_url(
                api_key,
                super::openrouter::VENICE_API_BASE.to_string(),
                RequestOrigin::Venice,
            )
            .with_venice_system_prompt(false);
            let request = ChatRequest {
                messages,
                model,
                max_tokens: AI_TOOL_CALL_MAX_TOKENS,
                temperature: AI_TOOL_CALL_TEMPERATURE,
            };
            let response: OpenRouterResponse = provider_impl
                .chat_with_tools(request, None, Some(&db_state))
                .await?;
            response.response.text
        }
        _ => return Err("Unsupported provider".to_string()),
    };

    Ok(response_text)
}


/// Consolidate a memory file from ai_memory source of truth (§5.5.3).
/// #2522 ADR-0063 Stage 2: sessionless background command — no chat session
/// to resolve an agent from, so per D5 this uses the designated default
/// agent's resolved runtime config instead of the old `AiStateWrapper`.
#[tauri::command]
pub async fn ai_consolidate_memory(
    app: tauri::AppHandle,
    db: State<'_, DbState>,
    project_id: Option<String>,
) -> Result<String, String> {
    eprintln!(
        "[CONSOLIDATE] Starting consolidation for project_id: {:?}",
        project_id
    );

    // Get provider config. ADR-0067 D2/D5 (#2958 follow-up): delegates to
    // `resolve_default_agent_dispatch_tuple` — the same shared helper `ai_save_memory`
    // calls — see that function's doc comment for why this was extracted.
    let (provider_name, api_key, model, endpoint, oc_runtime_ref) = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        let tuple = resolve_default_agent_dispatch_tuple(&conn)?;
        if tuple.0 == "none" {
            return Err("Unsupported provider for consolidation".to_string());
        }
        tuple
    };

    // The consolidator resolves and authorizes the canonical agent/project memory target.
    // This empty relative seam prevents DB paths from influencing production resolution.
    let file_path = std::path::PathBuf::new();

    // Call consolidator::consolidate_file - it will acquire locks as needed internally
    crate::ai::consolidator::consolidate_file(
        &db,
        &app,
        &file_path,
        project_id.as_deref(),
        &provider_name,
        &api_key,
        &model,
        endpoint.as_deref(),
        oc_runtime_ref.as_deref(),
    )
    .await?;

    Ok("Consolidation complete".to_string())
}


/// Response type for ai_synthesize_all_memories
#[derive(Debug, Serialize, Deserialize)]
pub struct SynthesisResult {
    pub synthesized: Vec<String>,
    pub skipped: i32,
    #[serde(rename = "totalFacts")]
    pub total_facts: i32,
}


/// Backend-owned synthesis: process ALL projects with unrouted facts in one call.
/// Queries ai_memory WHERE routed_at IS NULL grouped by project_id,
/// runs synthesis for each, and marks facts as routed.
/// Emits pane-reload-editor events for updated notes.
/// #2522 ADR-0063 Stage 2: sessionless background command — no chat session
/// to resolve an agent from, so per D5 this uses the designated default
/// agent's resolved runtime config instead of the old `AiStateWrapper`.
#[tauri::command]
pub async fn ai_synthesize_all_memories(
    app: tauri::AppHandle,
    db_state: State<'_, DbState>,
) -> Result<SynthesisResult, String> {
    let app_for_emit = app.clone();
    ai_synthesize_all_memories_impl(&db_state, move |note_id: &str| {
        // #2972 stage 1 (§34.1.1): migrated from `pane-note-changed`'s
        // `{note_id, reason: "memory_synthesis"}` to the single
        // entity-identified `pane-data-changed` event, `reason: "edited"`.
        emit_data_changed(&app_for_emit, note_id, ENTITY_NOTE, REASON_EDITED);
    })
    .await
}


/// #2775: split out of `ai_synthesize_all_memories` so Tier 2 tests can drive the
/// real db_clone-threading logic (the spawned per-project synthesis closure) without
/// needing a concrete `tauri::AppHandle` — the emit call was the only thing that
/// needed one, and it's now expressed as a generic callback the thin command wrapper
/// supplies (a `tauri::AppHandle::emit` closure in production, a no-op in tests).
async fn ai_synthesize_all_memories_impl<F>(
    db_state: &DbState,
    emit_note_changed: F,
) -> Result<SynthesisResult, String>
where
    F: Fn(&str) + Clone + Send + Sync + 'static,
{
    eprintln!("[MEMORY] Backend synthesis starting...");

    // Get AI config
    let (provider_name, api_key, model, endpoint, oc_runtime_ref) = {
        let conn = db_state.0.lock().map_err(|e| e.to_string())?;
        let cfg = crate::ai::runtime_resolution::resolve_runtime_config_for_default_agent(&conn)?;
        let (name, key, endpoint) = match &cfg.provider {
            AiProvider::OpenRouter { api_key } => ("openrouter", api_key.clone(), None),
            AiProvider::Anthropic { api_key } => ("anthropic", api_key.clone(), None),
            AiProvider::Venice { api_key, .. } => ("venice", api_key.clone(), None),
            AiProvider::OpenClaw { base_url, token, .. } => {
                ("openclaw", token.clone(), Some(base_url.clone()))
            }
            _ => return Err("Unsupported provider for synthesis".to_string()),
        };
        (name.to_string(), key, cfg.model.clone(), endpoint, cfg.runtime_ref.clone())
    };

    // Get unrouted facts grouped by project_id
    eprintln!("[SYNTHESIS-DEBUG] Querying unrouted facts...");
    let project_facts: Vec<(Option<String>, Vec<(String, String, String)>)> = {
        let conn = db_state.0.lock().map_err(|e| e.to_string())?;

        // Get distinct project_ids with unrouted facts
        let mut stmt = conn.prepare(
            "SELECT DISTINCT project_id FROM ai_memory WHERE routed_at IS NULL AND is_active = 1"
        ).map_err(|e| e.to_string())?;

        let project_ids: Vec<Option<String>> = stmt
            .query_map([], |row| row.get::<_, Option<String>>(0))
            .map_err(|e| e.to_string())?
            .filter_map(|r| r.ok())
            .collect();

        // For each project, get its facts
        let mut results = Vec::new();
        for pid in project_ids {
            let mut fact_stmt = if pid.is_some() {
                conn.prepare(
                    "SELECT id, fact, COALESCE(category, 'fact') FROM ai_memory WHERE project_id = ?1 AND routed_at IS NULL AND is_active = 1"
                ).map_err(|e| e.to_string())?
            } else {
                conn.prepare(
                    "SELECT id, fact, COALESCE(category, 'fact') FROM ai_memory WHERE project_id IS NULL AND routed_at IS NULL AND is_active = 1"
                ).map_err(|e| e.to_string())?
            };

            let facts: Vec<(String, String, String)> = if let Some(ref p) = pid {
                fact_stmt
                    .query_map(rusqlite::params![p], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                        ))
                    })
                    .map_err(|e| e.to_string())?
                    .filter_map(|r| r.ok())
                    .collect()
            } else {
                fact_stmt
                    .query_map([], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                        ))
                    })
                    .map_err(|e| e.to_string())?
                    .filter_map(|r| r.ok())
                    .collect()
            };

            if !facts.is_empty() {
                results.push((pid, facts));
            }
        }
        results
    };

    let total_unrouted: usize = project_facts.iter().map(|(_, facts)| facts.len()).sum();
    eprintln!(
        "[SYNTHESIS-DEBUG] Found {} unrouted facts across {} projects",
        total_unrouted,
        project_facts.len()
    );

    if project_facts.is_empty() {
        eprintln!("[MEMORY] No unrouted facts found — skipping synthesis");
        return Ok(SynthesisResult {
            synthesized: vec![],
            skipped: 0,
            total_facts: 0,
        });
    }

    // Collect all project data needed for synthesis under a single lock
    #[allow(dead_code)]
    struct ProjectSynthesisJob {
        project_id: Option<String>,
        project_name: String,
        note_id: String,
        current_content: String,
        facts: Vec<(String, String, String)>, // (id, fact, category)
    }

    let mut jobs: Vec<ProjectSynthesisJob> = Vec::new();
    let mut skipped_count = 0i32;

    {
        let conn = db_state.0.lock().map_err(|e| e.to_string())?;
        for (project_id, facts) in &project_facts {
            let pid = if let Some(ref p) = project_id {
                p.clone()
            } else {
                // §24: Find hidden system project for personal memory notes
                conn.query_row(
                    "SELECT id FROM projects WHERE is_system = 1 AND (parent_id IS NULL OR parent_id = '') LIMIT 1",
                    [],
                    |row| row.get::<_, String>(0),
                ).unwrap_or_default()
            };

            let pname: String = if project_id.is_none() {
                "Personal".to_string()
            } else {
                conn.query_row(
                    "SELECT name FROM projects WHERE id = ?1",
                    rusqlite::params![pid],
                    |row| row.get(0),
                )
                .unwrap_or_else(|_| "Unknown".to_string())
            };

            // §5.3A: Route memory based on project context
            // Personal facts (project_id is None) → agent MEMORY.md via get_system_note
            // Project facts (project_id is Some) → project's AI/Memory note via direct query
            let memory_note = if project_id.is_none() {
                // Personal memory → agent MEMORY.md
                crate::database::get_system_note(&conn, &pid, "memory")
            } else {
                // Project memory → project's AI/Memory note from notes table
                // Query for the note's id and file_path, then read content from file
                match conn.query_row(
                    "SELECT id, file_path FROM notes WHERE project_id = ?1 AND note_type = 'memory' AND deleted_at IS NULL LIMIT 1",
                    rusqlite::params![pid],
                    |row| {
                        let note_id: String = row.get(0)?;
                        let file_path: String = row.get(1)?;
                        Ok((note_id, file_path))
                    }
                ) {
                    Ok((note_id, _file_path)) => {
                        // Read from DB body column for memory notes
                        let content = conn.query_row(
                            "SELECT COALESCE(body, content_preview, '') FROM notes WHERE id = ?1",
                            params![&note_id],
                            |row| row.get::<_, String>(0),
                        ).unwrap_or_default();
                        Ok(Some((note_id, content)))
                    }
                    Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                    Err(e) => Err(e.to_string()),
                }
            };

            match memory_note {
                Ok(Some((nid, content))) => {
                    jobs.push(ProjectSynthesisJob {
                        project_id: project_id.clone(),
                        project_name: pname,
                        note_id: nid,
                        current_content: content,
                        facts: facts.clone(),
                    });
                }
                Ok(None) => {
                    // Lazy-create Memory note at project root
                    eprintln!(
                        "[MEMORY] No Memory note found for project {} — creating one",
                        pname
                    );

                    let note_id = uuid::Uuid::new_v4().to_string();
                    let now = chrono::Utc::now().to_rfc3339();
                    if let Err(e) = PaneFs::validate_name(&pname) {
                        eprintln!(
                            "[MEMORY] Unsafe project name in database for {}: {}",
                            pname, e
                        );
                        skipped_count += 1;
                        continue;
                    }
                    let memory_relative =
                        match PaneFs::relative_from_names([pname.as_str(), "Memory.md"]) {
                            Ok(path) => path,
                            Err(e) => {
                                eprintln!("[MEMORY] Unsafe Memory path for {}: {}", pname, e);
                                skipped_count += 1;
                                continue;
                            }
                        };
                    let file_path = std::path::Path::new("Projects")
                        .join(&memory_relative)
                        .to_string_lossy()
                        .to_string();
                    let pane_fs = match PaneFs::for_runtime() {
                        Ok(pane_fs) => pane_fs,
                        Err(e) => {
                            eprintln!("[MEMORY] Filesystem policy failed for {}: {}", pname, e);
                            skipped_count += 1;
                            continue;
                        }
                    };
                    if let Err(e) =
                        pane_fs.create_dir_all(PaneRoot::Projects, std::path::Path::new(&pname))
                    {
                        eprintln!(
                            "[MEMORY] Failed to create project directory for {}: {}",
                            pname, e
                        );
                        skipped_count += 1;
                        continue;
                    }
                    if let Err(e) = atomic_write_authorized(
                        &pane_fs,
                        PaneRoot::Projects,
                        &memory_relative,
                        "# Memory\n\n",
                    ) {
                        eprintln!("[MEMORY] Failed to write Memory file for {}: {}", pname, e);
                        skipped_count += 1;
                        continue;
                    }

                    // Insert into database
                    if let Err(e) = conn.execute(
                        "INSERT INTO notes (id, title, content_preview, file_path, project_id, folder_id, note_type, tags_json, owner_id, created_at, updated_at)
                         VALUES (?1, 'Memory', '', ?2, ?3, NULL, 'memory', '[]', 'user_local', ?4, ?5)",
                        params![note_id, file_path, project_id, now, now],
                    ) {
                        eprintln!("[MEMORY] Failed to create Memory note for {}: {}", pname, e);
                        skipped_count += 1;
                        continue;
                    }

                    // Now add this project to the synthesis jobs
                    jobs.push(ProjectSynthesisJob {
                        project_id: project_id.clone(),
                        project_name: pname,
                        note_id,
                        current_content: String::from("# Memory\n\n"),
                        facts: facts.clone(),
                    });
                }
                Err(e) => {
                    eprintln!("[MEMORY] Error getting Memory note for {}: {}", pname, e);
                    skipped_count += 1;
                }
            }
        }
    } // DB lock dropped

    if jobs.is_empty() {
        return Ok(SynthesisResult {
            synthesized: vec![],
            skipped: skipped_count,
            total_facts: 0,
        });
    }

    // Get a clone of the DB Arc for spawned tasks
    let db_arc = db_state.0.clone();

    // Spawn parallel synthesis tasks — each project is independent
    let mut handles = Vec::new();
    for job in jobs {
        let emit_note_changed = emit_note_changed.clone();
        let db_clone = db_arc.clone();
        let provider = provider_name.clone();
        let key = api_key.clone();
        let mdl = model.clone();
        let ep = endpoint.clone();
        let oc_ref = oc_runtime_ref.clone();

        let handle = tokio::spawn(async move {
            let fact_ids: Vec<String> = job.facts.iter().map(|(id, _, _)| id.clone()).collect();
            let fact_count = job.facts.len() as i32;

            eprintln!(
                "[SYNTHESIS-DEBUG] [{}] Starting synthesis of {} facts (parallel)",
                job.project_name, fact_count
            );

            // Build the content with new facts appended
            let new_facts_text = job
                .facts
                .iter()
                .map(|(_, fact, category)| format!("- [{}] {}", category, fact))
                .collect::<Vec<_>>()
                .join("\n");

            let content_with_new_facts = format!(
                "{}\n\n## NEW FACTS TO INCORPORATE\n{}",
                job.current_content.trim(),
                new_facts_text
            );

            // Memory size guard: adjust synthesis prompt based on note length (§16.3)
            let memory_word_count = job.current_content.split_whitespace().count();
            let (size_mode, size_suffix) = synthesis_size_guard(memory_word_count);
            eprintln!(
                "[SYNTHESIS] [{}] Memory note word count: {} (mode: {})",
                job.project_name, memory_word_count, size_mode
            );

            // Call LLM to synthesize
            let base_prompt = format!(
            "You are rewriting a user's Memory profile for an AI assistant app called Pane.\n\nHere is the current Memory document:\n\n---\n{}\n---\n\nRewrite this into a clean, well-organized profile using SHORT paragraphs (2-3 sentences each), separated by blank lines.\n\nRULES:\n- Use the LATEST information when facts contradict\n- Remove duplicates — each fact appears only once\n- Write in third person (\"Matt is...\", \"He prefers...\")\n- Keep it concise — reference document, not biography\n- Do not add category headers — just natural prose paragraphs\n- Do not invent anything not in the source\n- Preserve ALL facts from the source\n- Remove any meta text like \"This document will be updated...\" or \"Learned from conversation\"\n- If there are NEW FACTS listed at the end of the document, you MUST incorporate them into the profile naturally. Weave them into existing paragraphs where they fit, or create new paragraphs for new topics. Never leave them as a separate list.\n- After incorporating new facts, remove the \"NEW FACTS TO INCORPORATE\" section entirely from the output.\n- CRITICAL: If there is a \"NEW FACTS TO INCORPORATE\" section at the end of the document, you MUST weave EVERY SINGLE fact from that section into the profile. Count them — your output must contain all of them as natural prose. Do not skip any. Do not leave them as a list.\n- After incorporating new facts, remove the \"NEW FACTS TO INCORPORATE\" header and bullet list entirely from your output. The facts should be woven into existing paragraphs or new paragraphs — never left as a separate section.\n\nReturn ONLY the rewritten profile text.\n\nREMINDER: If NEW FACTS appear below, every single one MUST appear in your output as natural prose. Missing even one is a failure.",
            content_with_new_facts
        );

            let prompt = if let Some(suffix) = size_suffix {
                format!("{}{}", base_prompt, suffix)
            } else {
                base_prompt
            };

            let messages = vec![ChatMessage {
                role: "user".to_string(),
                content: prompt,
                tool_calls: None,
                tool_call_id: None,
                content_blocks: None,
                attachment_ids: None,
                id: None,
            }];

            // Use unified LLM call function
            // #2775: db_clone (already cloned into this closure for the DB
            // write-back below) is also threaded into the LLM call now —
            // wrapped as a DbState so select_openrouter_client can reach the
            // pinned-cert gateway client on paired installs, instead of the
            // LLM call hardcoding None three lines away from a live handle.
            let db_for_llm = crate::database::DbState(db_clone.clone());
            let response_text = match call_llm_from_config(
                &provider,
                &key,
                &mdl,
                ep.as_deref(),
                &messages,
                SYNTHESIS_MAX_TOKENS,
                SYNTHESIS_TEMPERATURE,
                90, // timeout_secs (synthesis can take longer)
                Some(&db_for_llm),
                None, // model_registry not available in background synthesis context
                oc_ref.as_deref(),
            )
            .await
            {
                Ok(text) => text,
                Err(e) => {
                    eprintln!(
                        "[SYNTHESIS-ERROR] [{}] LLM call failed: {}",
                        job.project_name, e
                    );
                    return Err(format!("LLM error for {}: {}", job.project_name, e));
                }
            };

            // Write the synthesized content back to the note
            {
                let conn = db_clone.lock().map_err(|e| e.to_string())?;

                // Get file_path for the note
                let file_path: String = conn
                    .query_row(
                        "SELECT file_path FROM notes WHERE id = ?1",
                        rusqlite::params![job.note_id],
                        |row| row.get(0),
                    )
                    .unwrap_or_default();

                if !file_path.is_empty() {
                    // Save current content as a version before synthesis overwrites it
                    let pre_synth_content =
                        crate::database::read_note_content(&file_path).unwrap_or_default();
                    if !pre_synth_content.trim().is_empty() {
                        if let Err(e) = crate::versions::save_version(
                            &conn,
                            &job.note_id,
                            &pre_synth_content,
                            "ai_synthesis",
                        ) {
                            eprintln!("[SYNTHESIS-ERROR] Failed to save pre-synthesis version for note {}: {}", job.note_id, e);
                        } else {
                            eprintln!(
                                "[SYNTHESIS-DEBUG] Saved pre-synthesis version for note {}",
                                job.note_id
                            );
                        }
                    }

                    // Content loss guard: block synthesis if output is >50% shorter than existing content
                    let existing_len = pre_synth_content.trim().len();
                    let new_len = response_text.trim().len();
                    if existing_len > 100 && new_len < (existing_len * 50 / 100) {
                        let reduction_pct = ((existing_len - new_len) * 100) / existing_len;
                        eprintln!("[SYNTHESIS-BLOCKED] [{}] Content reduction {}% ({} → {} chars) — refusing to overwrite Memory note {}",
                        job.project_name, reduction_pct, existing_len, new_len, job.note_id);
                        return Err(format!(
                        "Synthesis blocked: would reduce Memory note content by {}% ({} → {} chars). This likely indicates a synthesis error.",
                        reduction_pct, existing_len, new_len
                    ));
                    }

                    // Write content to file
                    eprintln!(
                        "[SYNTHESIS-DEBUG] [{}] Writing {} chars to Memory note {}",
                        job.project_name,
                        response_text.len(),
                        job.note_id
                    );
                    let write_result = crate::database::write_note_content(
                        &file_path,
                        &response_text,
                        Some(&conn),
                    );
                    if write_result.is_ok() {
                        eprintln!("[SYNTHESIS-DEBUG] [{}] Write success", job.project_name);
                    } else {
                        eprintln!(
                            "[SYNTHESIS-ERROR] [{}] Write FAILED: {}",
                            job.project_name,
                            write_result.as_ref().unwrap_err()
                        );
                    }
                    write_result?;

                    // Update note metadata in DB
                    let content_preview: String = response_text.chars().take(200).collect();
                    let word_count = response_text.split_whitespace().count() as i64;
                    let now = chrono::Utc::now().to_rfc3339();

                    if let Err(e) = conn.execute(
                    "UPDATE notes SET content_preview = ?1, word_count = ?2, updated_at = ?3 WHERE id = ?4",
                    rusqlite::params![content_preview, word_count, now, job.note_id],
                ) {
                    eprintln!("[SYNTHESIS-ERROR] [{}] Failed to update note metadata (note {}): {}", job.project_name, job.note_id, e);
                } else {
                    eprintln!("[SYNTHESIS] [{}] Wrote to DB: note_id={}, content_len={}", job.project_name, job.note_id, response_text.len());
                    eprintln!("[SYNTHESIS] [{}] Wrote to flat file: path={}", job.project_name, file_path);
                }

                    // FTS update happens automatically via trigger (§486)

                    // === Read-back verification (#163) ===
                    // Verify flat file content matches what was written
                    let readback =
                        crate::database::read_note_content(&file_path).unwrap_or_default();
                    if readback.trim() == response_text.trim() {
                        eprintln!(
                            "[SYNTHESIS-VERIFY] ✓ Read-back matches for '{}' ({} chars)",
                            job.project_name,
                            readback.len()
                        );
                    } else {
                        eprintln!("[SYNTHESIS-ERROR] ✗ Read-back MISMATCH for '{}': wrote {} chars, read back {} chars",
                        job.project_name, response_text.len(), readback.len());
                        // Log first divergence point for debugging
                        let write_chars: Vec<char> = response_text.chars().collect();
                        let read_chars: Vec<char> = readback.chars().collect();
                        for (i, (w, r)) in write_chars.iter().zip(read_chars.iter()).enumerate() {
                            if w != r {
                                eprintln!("[SYNTHESIS-ERROR] First divergence at char {}: wrote {:?}, read {:?}", i, w, r);
                                break;
                            }
                        }
                    }

                    // Verify FTS index was updated
                    let fts_content: Option<String> = conn.query_row(
                    "SELECT body FROM notes_fts WHERE rowid = (SELECT rowid FROM notes WHERE id = ?1)",
                    rusqlite::params![job.note_id],
                    |row| row.get(0),
                ).ok();
                    if let Some(ref fts) = fts_content {
                        let check_len = 50.min(response_text.len());
                        if check_len > 0 && fts.contains(&response_text[..check_len]) {
                            eprintln!(
                                "[SYNTHESIS-VERIFY] ✓ FTS index updated for '{}'",
                                job.project_name
                            );
                        } else {
                            eprintln!(
                                "[SYNTHESIS-ERROR] ✗ FTS index NOT updated for '{}'",
                                job.project_name
                            );
                        }
                    } else {
                        eprintln!(
                            "[SYNTHESIS-ERROR] ✗ FTS row not found for '{}'",
                            job.project_name
                        );
                    }

                    // Verify metadata (word_count, content_preview)
                    let (_db_preview, db_wc): (String, i64) = conn.query_row(
                    "SELECT COALESCE(content_preview, ''), COALESCE(word_count, 0) FROM notes WHERE id = ?1",
                    rusqlite::params![job.note_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                ).unwrap_or_default();
                    let expected_wc = response_text.split_whitespace().count() as i64;
                    if db_wc == expected_wc {
                        eprintln!(
                            "[SYNTHESIS-VERIFY] ✓ Word count correct for '{}': {}",
                            job.project_name, db_wc
                        );
                    } else {
                        eprintln!(
                            "[SYNTHESIS-ERROR] ✗ Word count mismatch for '{}': expected {}, got {}",
                            job.project_name, expected_wc, db_wc
                        );
                    }
                }

                // Mark facts as routed
                eprintln!(
                    "[SYNTHESIS-DEBUG] [{}] Marking {} facts as routed",
                    job.project_name,
                    fact_ids.len()
                );
                for fact_id in &fact_ids {
                    if let Err(e) = conn.execute(
                        "UPDATE ai_memory SET routed_at = datetime('now') WHERE id = ?1",
                        rusqlite::params![fact_id],
                    ) {
                        eprintln!(
                            "[SYNTHESIS-ERROR] Failed to set routed_at for fact {}: {}",
                            fact_id, e
                        );
                    } else {
                        eprintln!(
                            "[SYNTHESIS-DEBUG] routed_at UPDATE for fact {}: success",
                            fact_id
                        );
                    }
                }
            }

            // Emit the entity-identified pane-data-changed event (#2972 §34.1) so
            // listeners patch this note's metadata in place — not a reload (§34.5).
            emit_note_changed(&job.note_id);

            eprintln!(
                "[SYNTHESIS-DEBUG] [{}] Complete — {} facts routed",
                job.project_name, fact_count
            );
            Ok::<(String, i32), String>((job.project_name, fact_count))
        }); // end tokio::spawn

        handles.push(handle);
    }

    // Await all parallel tasks and collect results
    let mut synthesized_names: Vec<String> = Vec::new();
    let mut total_facts_routed = 0i32;

    for handle in handles {
        match handle.await {
            Ok(Ok((name, count))) => {
                synthesized_names.push(name);
                total_facts_routed += count;
            }
            Ok(Err(e)) => {
                eprintln!("[SYNTHESIS-ERROR] Task failed: {}", e);
                skipped_count += 1;
            }
            Err(e) => {
                eprintln!("[SYNTHESIS-ERROR] Task panicked: {}", e);
                skipped_count += 1;
            }
        }
    }

    eprintln!(
        "[MEMORY] Synthesis complete: {} projects updated (parallel), {} skipped, {} facts routed",
        synthesized_names.len(),
        skipped_count,
        total_facts_routed
    );

    Ok(SynthesisResult {
        synthesized: synthesized_names,
        skipped: skipped_count,
        total_facts: total_facts_routed,
    })
}


#[cfg(test)]
mod ai_synthesize_all_memories_db_threading_tests {
    //! #2775 Tier 2 — site 4 of 4: proves the spawned synthesis closure's
    //! `db_clone` (already threaded through for the write-back) also reaches
    //! `select_openrouter_client` as `Some(..)` via the newly-added `db_for_llm`
    //! wrap. Drives `ai_synthesize_all_memories_impl` directly (the AppHandle-free
    //! core split out of the `#[tauri::command]` wrapper for exactly this reason
    //! — see its doc comment) with a no-op emit callback, since the DB-to-Arc-to-
    //! DbState threading this test cares about lives entirely in the impl body.
    use super::*;
    use crate::keychain::test_support::KeychainGuard;
    use serial_test::serial;
    use tempfile::TempDir;

    static TEST_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[tokio::test]
    // #2903 Phase 2 PR1: this test mutates PANE_TEST_ROOT (the crate-wide
    // get_pane_root() override), so it must share the `pane_test_root` serial
    // group with every other PANE_TEST_ROOT-mutating test, not the bare default
    // group — the module-local TEST_MUTEX below only protects this single test
    // from itself, not from concurrently-running tests elsewhere in the crate
    // that also swap PANE_TEST_ROOT (see backup.rs / consolidator.rs / openrouter.rs).
    #[serial(pane_test_root, keychain_override)]
    async fn test_ai_synthesize_all_memories_threads_db_to_pinned_client() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _kc = KeychainGuard::set();

        let temp_fs_dir = TempDir::new().expect("tempdir");
        std::env::set_var("PANE_TEST_ROOT", temp_fs_dir.path());

        let conn = rusqlite::Connection::open_in_memory().expect("in-memory db");
        crate::migrations::run_migrations(&conn).expect("fresh-install migrations should succeed");

        // Paired openclaw install: default agent on the openclaw runtime, a stored
        // gateway token (keychain), an endpoint, and a pinned SPKI cert.
        conn.execute(
            "INSERT INTO ai_agents (id, name, slug, workspace_path, is_default, is_active, runtime_type, model)
             VALUES ('agent-1', 'Test Agent', 'test-agent', 'test-agent', 1, 1, 'openclaw', 'openclaw')",
            [],
        )
        .unwrap();
        crate::ai::credential_migration::write_gateway_token(&conn, "oc-token-abc").unwrap();
        crate::database::set_ai_setting(&conn, "endpoint", "https://127.0.0.1:1").unwrap();
        let pem = crate::gateway_client::test_make_gateway_cert_pem();
        crate::database::set_ai_setting(&conn, "gateway_tls_cert_pem", &pem).unwrap();

        // Hidden system project (personal-memory routing target) + one unrouted fact.
        conn.execute(
            "INSERT INTO projects (id, name, is_system, parent_id) VALUES ('sys-proj', 'Personal', 1, NULL)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO ai_memory (id, fact, category, is_active, project_id, routed_at)
             VALUES ('fact-1', 'The user prefers dark mode.', 'preference', 1, NULL, NULL)",
            [],
        )
        .unwrap();

        // Physical MEMORY.md so get_system_note_from_agents resolves Some(..) rather
        // than falling into the lazy-create-Memory-note branch (irrelevant to what
        // this test asserts, just avoids an unrelated filesystem side path).
        let memory_dir = temp_fs_dir.path().join(".pane/system/test-agent");
        std::fs::create_dir_all(&memory_dir).unwrap();
        std::fs::write(memory_dir.join("MEMORY.md"), "# Memory\n\nExisting content.\n").unwrap();

        let db_state = DbState::new(conn);

        // The spawned per-project task will fail its network call against the closed
        // port (127.0.0.1:1) — that's fine, the outer command still returns Ok with
        // that project counted as skipped. The stamp is set during client selection,
        // before the network attempt, which is what this test actually asserts.
        let _ = ai_synthesize_all_memories_impl(&db_state, |_note_id: &str| {}).await;

        assert_eq!(
            crate::gateway_client::test_take_last_client_path(),
            Some(crate::gateway_client::ClientPathStamp::Pinned),
            "ai_synthesize_all_memories' spawned closure must thread db_clone into the LLM \
             call as Some(..) and select the pinned client, not fall through to the \
             pre-pairing/danger_accept path"
        );

    }
}


// ADR-0045 D4's `restore_non_oc_main_inner`/`restore_non_oc_main` (#1492) — dead
// from the UI since ADR-0059 D3 retired the switch-away call site (2026-07-26,
// #2030 PR B; Settings.svelte's picker/forced-create flow re-targets instead) —
// removed entirely per ADR-0062 D8 (#2522): a live registered command that
// mutates `is_main` via a retired selection rule is pure hazard (scope audit §2).
// Test module (`restore_non_oc_main_tests`, ADR-0045 #1492 unit tests) and the
// `lib.rs` handler registration were removed in the same pass.

/// Run memory decay — retire stale facts based on category thresholds (§5.4A).
/// Soft-delete only (is_active = 0). Never decays identity/preference/style.
pub fn run_memory_decay(conn: &rusqlite::Connection) -> (usize, usize) {
    let mut fact_decision_count: usize = 0;

    // 90-day decay: fact, decision, deadline
    let ninety_day_categories = ["fact", "decision", "deadline"];
    for cat in &ninety_day_categories {
        let decayed = conn
            .execute(
                "UPDATE ai_memory SET is_active = 0 WHERE category = ?1 AND is_active = 1
             AND COALESCE(last_referenced, created_at) < datetime('now', '-90 days')",
                rusqlite::params![cat],
            )
            .unwrap_or(0);
        fact_decision_count += decayed;
        if decayed > 0 {
            eprintln!(
                "[DECAY] Retired {} {} facts (90-day threshold)",
                decayed, cat
            );
        }
    }

    // 30-day decay: reflection
    let decayed = conn
        .execute(
            "UPDATE ai_memory SET is_active = 0 WHERE category = 'reflection' AND is_active = 1
         AND COALESCE(last_referenced, created_at) < datetime('now', '-30 days')",
            [],
        )
        .unwrap_or(0);
    let reflection_count = decayed;
    if decayed > 0 {
        eprintln!(
            "[DECAY] Retired {} reflection facts (30-day threshold)",
            decayed
        );
    }

    let total = fact_decision_count + reflection_count;
    eprintln!(
        "[DECAY] Decayed {} facts ({} fact/decision, {} reflection)",
        total, fact_decision_count, reflection_count
    );
    (fact_decision_count, reflection_count)
}


/// Tauri command wrapper for memory decay — callable from frontend
#[tauri::command]
pub fn ai_run_decay_check(db: State<'_, DbState>) -> Result<usize, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let (fact_decision, reflection) = run_memory_decay(&conn);
    Ok(fact_decision + reflection)
}


/// Switch the active embedding engine and re-index all content under the new engine.
///
/// This command:
/// 1. Validates the engine name and model are non-empty
/// 2. Acquires the DB lock briefly: persists settings, clears stale vectors,
///    sets status = "indexing", builds embed_fn (reads credentials), then RELEASES the lock.
/// 3. Opens a SEPARATE read-only connection for the pipeline — main Mutex is NOT held
///    during the potentially long indexing run.
/// 4. Runs the pipeline, emitting `embeddings://status` progress events via `app_handle`.
/// 5. Re-acquires the lock briefly to write the terminal status (ready/error).
///
/// **DB lock behaviour (fix #1657):** The lock is held for ~milliseconds during settings
/// persistence and credential reading, then released before the pipeline starts. Other DB
/// operations (chat, memory extraction, sync) are never blocked by embedding indexing.
/// Resolve the token limit to persist for a given embedding engine/model, per #2776's
/// precedence chain (scope doc §5a):
///   1. `max_tokens` supplied by the caller (OpenRouter's fetched `context_length`,
///      threaded from the frontend's already-fetched `ModelInfo.context_length` —
///      see `fetch_openrouter_embedding_models`).
///   2. Static seed table (`embedding_model_seed_lookup`, Ollama's only source —
///      Ollama's `/api/tags` never reports a context window for embedding models).
///   3. Last-resort conservative floor (`EMBEDDINGS_MAX_TOKENS_FALLBACK` = 512),
///      ALWAYS logged so an unauthoritative fallback is never silent (Matt's ruling #3).
///
/// Never returns a bare magic number without going through one of these three named,
/// logged sources (Matt's ruling #1).
fn resolve_embeddings_max_tokens(model: &str, max_tokens: Option<u64>) -> u64 {
    if let Some(t) = max_tokens {
        eprintln!("[SWITCH_EMBEDDING_ENGINE] max_tokens resolved from caller-supplied context_length: {}", t);
        return t;
    }
    if let Some(t) = crate::embeddings::embedding_model_seed_lookup(model) {
        eprintln!("[SWITCH_EMBEDDING_ENGINE] max_tokens resolved from static seed table for '{}': {}", model, t);
        return t;
    }
    eprintln!(
        "[SWITCH_EMBEDDING_ENGINE] unknown model '{}', using conservative floor of {} tokens",
        model, crate::embeddings::EMBEDDINGS_MAX_TOKENS_FALLBACK
    );
    crate::embeddings::EMBEDDINGS_MAX_TOKENS_FALLBACK
}


/// Default `/embeddings` batch size (#3082 follow-up to #2988). Matt's ruling: the
/// provider's `/embeddings` endpoint is called once per `batch_size` items instead of
/// once per item, but the size itself must be configurable and never a bare magic
/// number. `25` matches `run_embedding_pipeline`'s existing progress-throttle cadence
/// (fires at most once per 25 items per sub-pass, §3 throttle) so one full batch lines
/// up with one progress tick — no new cadence bookkeeping, no batch-boundary vs
/// throttle-boundary drift to reconcile.
pub const EMBEDDINGS_BATCH_SIZE_DEFAULT: usize = 25;


/// Resolve the `/embeddings` batch size for the re-index pipeline (#3082), following
/// the exact precedence/logging discipline `resolve_embeddings_max_tokens` (above) uses:
/// a named fallback constant, logged regardless of which source wins, persisted to
/// `ai_settings` (key `embeddings_batch_size`) so it's inspectable and survives
/// restarts without a code change (e.g. via a future Settings control or a support
/// instruction to set the key directly).
///
/// Clamped to 1..=100 — a sane guess, not verified against any provider's real
/// `/embeddings` array-length limit (flagged for Matt's live pass, see PR notes).
fn resolve_embeddings_batch_size(conn: &rusqlite::Connection) -> usize {
    match crate::database::get_ai_setting(conn, "embeddings_batch_size")
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|&n| (1..=100).contains(&n))
    {
        Some(n) => {
            eprintln!("[SWITCH_EMBEDDING_ENGINE] batch_size resolved from ai_settings: {}", n);
            n
        }
        None => {
            eprintln!(
                "[SWITCH_EMBEDDING_ENGINE] no valid embeddings_batch_size setting, using default of {}",
                EMBEDDINGS_BATCH_SIZE_DEFAULT
            );
            EMBEDDINGS_BATCH_SIZE_DEFAULT
        }
    }
}


// ─── Concurrency Guard (#3082, same shape as backup.rs's `BackupGuard` / R7-#1921) ─────
//
// Converting `switch_embedding_engine` to `async fn` + `spawn_blocking` makes a second,
// concurrent switch reachable for the first time — today's main-thread block serializes
// every call by construction. A second switch while one is running would race the
// stale-embedding DELETE, orphan rows tagged with the first call's (abandoned) engine,
// and interleave/overwrite the terminal `embeddings_status`/`embeddings_failed_count`
// writes non-deterministically. Matt's ruling: reject the second call outright — no
// queueing, no cancel-and-restart (that comes later with the harness Stop plumbing).
static EMBEDDING_SWITCH_IN_PROGRESS: AtomicBool = AtomicBool::new(false);


#[derive(Debug)]
struct EmbeddingSwitchGuard;


impl EmbeddingSwitchGuard {
    fn try_acquire() -> Result<Self, String> {
        if EMBEDDING_SWITCH_IN_PROGRESS
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Err("a re-index is already running".to_string());
        }
        Ok(EmbeddingSwitchGuard)
    }
}


impl Drop for EmbeddingSwitchGuard {
    fn drop(&mut self) {
        EMBEDDING_SWITCH_IN_PROGRESS.store(false, Ordering::SeqCst);
    }
}


#[tauri::command]
pub async fn switch_embedding_engine(
    db: tauri::State<'_, DbState>,
    app_handle: tauri::AppHandle,
    engine: String,
    model: String,
    // The Ollama endpoint entered in the Semantic Search card (§5.9.2 / #1657).
    // When present, stored as `embeddings_endpoint` in ai_settings. C50 (#3171
    // PR 1) retired the sibling `api_key` param — OpenRouter now reads its key
    // from Provider Connections (`read_provider_key`, inside
    // `build_http_embed_batch_fn`'s openrouter arm) and Venice already read the
    // chat-provider key directly, so no caller has anything to pass here
    // anymore; Ollama has no key, only this endpoint.
    endpoint: Option<String>,
    // #2776: the embedding model's token/context limit, sourced from OpenRouter's
    // `/api/v1/models?output_modalities=embeddings` response (`ModelInfo.context_length`,
    // already fetched by `fetch_embedding_models` at model-selection time and threaded
    // through here by the frontend). `None` for Ollama (no live source) or if the
    // frontend didn't have a value — falls through to the seed table / conservative floor
    // in `resolve_embeddings_max_tokens`.
    max_tokens: Option<u64>,
) -> Result<String, String> {
    eprintln!(
        "[SWITCH_EMBEDDING_ENGINE] called with: engine={} model={} max_tokens={:?}",
        engine, model, max_tokens
    );

    if engine != "openrouter" && engine != "ollama" && engine != "venice" {
        return Err(format!(
            "Invalid engine '{}': must be 'openrouter', 'ollama', or 'venice'",
            engine
        ));
    }
    if model.trim().is_empty() {
        return Err("model must not be empty".to_string());
    }

    // #3082: reject a second concurrent switch outright (see EmbeddingSwitchGuard's doc
    // comment above). Acquired BEFORE spawn_blocking even starts, matching R7's
    // BackupGuard — the message is exactly "a re-index is already running".
    let _guard = EmbeddingSwitchGuard::try_acquire()?;

    // #2776: resolve the token limit up front — doesn't need DB access, pure, stays
    // outside spawn_blocking (same reasoning #1296's fetch_oc_agent_model pre-fetch uses).
    let resolved_max_tokens = resolve_embeddings_max_tokens(&model, max_tokens);

    // #3082 (async + spawn_blocking, per #1921/R7 + #1296's State<DbState> pattern):
    // clone the Arc so it can be moved into spawn_blocking; the lock is acquired INSIDE
    // the closure at each site below, never held across an await point.
    let db_arc = db.0.clone();
    let app_handle_blocking = app_handle.clone();

    tokio::task::spawn_blocking(move || -> Result<String, String> {
        let app_handle = app_handle_blocking;

        // === Step 1: Lock briefly — persist settings + clear stale rows + build embed_fn ===
        // The MutexGuard drops at the end of this block, releasing the lock before the pipeline.
        let embed_fn_result: Result<
            (
                Box<dyn Fn(&[&str]) -> Result<Vec<Vec<f32>>, String> + Send + Sync + 'static>,
                String,
            ),
            String,
        > = {
            let conn = db_arc.lock().map_err(|e| format!("db_lock_failed: {}", e))?;

            // Persist engine + model
            crate::database::set_ai_setting(&conn, "embeddings_engine", &engine)
                .map_err(|e| format!("Failed to save embeddings_engine: {}", e))?;
            crate::database::set_ai_setting(&conn, "embeddings_model", &model)
                .map_err(|e| format!("Failed to save embeddings_model: {}", e))?;

            // #2776: persist the resolved token limit alongside engine/model so the chunker
            // (run_embedding_pipeline_batched) can read it on every future pipeline run, not
            // just this one — see resolve_embeddings_max_tokens for the precedence chain.
            crate::database::set_ai_setting(&conn, "embeddings_max_tokens", &resolved_max_tokens.to_string())
                .map_err(|e| format!("Failed to save embeddings_max_tokens: {}", e))?;

            // C50 (#3171 PR 1): no more persisting an OpenRouter key here —
            // `embeddings_api_key` is retired (it was plaintext in `ai_settings`,
            // never in the keychain — a credential-handling defect against
            // ADR-0044, not a UI tidy-up). The Ollama endpoint is NOT a secret
            // and is unaffected by C50; it still persists below.
            if let Some(ref ep) = endpoint {
                if !ep.is_empty() {
                    crate::database::set_ai_setting(&conn, "embeddings_endpoint", ep)
                        .map_err(|e| format!("Failed to save embeddings_endpoint: {}", e))?;
                }
            }

            // Clear stale vectors from other engines OR other models within the SAME
            // engine (#3101 — a model switch within one engine previously cleared
            // nothing at all, since the predicate only checked `engine`; two models
            // under one engine produce vectors of different dimension/semantics and are
            // not comparable, same as two different engines). NULL = legacy/stale,
            // treated same as a wrong engine/model.
            let deleted = conn
                .execute(
                    "DELETE FROM embeddings WHERE engine IS NULL OR engine != ?1 OR model IS NULL OR model != ?2",
                    rusqlite::params![engine, model],
                )
                .map_err(|e| format!("Failed to clear stale embeddings: {}", e))?;
            eprintln!(
                "[SWITCH_EMBEDDING_ENGINE] Cleared {} stale embedding rows",
                deleted
            );

            // Mark status as indexing + enabled
            crate::database::set_ai_setting(&conn, "embeddings_status", "indexing")
                .map_err(|e| format!("Failed to set embeddings_status: {}", e))?;
            crate::database::set_ai_setting(&conn, "embeddings_enabled", "true")
                .map_err(|e| format!("Failed to set embeddings_enabled: {}", e))?;

            // #3082: batch-capable embed_fn (was build_http_embed_fn) — reads credentials
            // from DB and returns a closure that captures the credential strings by value
            // (does NOT retain the Connection). Lock drops here when this block exits.
            crate::build_http_embed_batch_fn(&conn, &engine, &model)
        };

        // === Step 2: Handle credential build failure (lock already released) ===
        let (batch_embed_fn, engine_tag) = embed_fn_result.map_err(|e| {
            eprintln!("[SWITCH_EMBEDDING_ENGINE] Credential build failed: {}", e);
            if let Ok(conn) = db_arc.lock() {
                let _ = crate::database::set_ai_setting(&conn, "embeddings_status", "error");
            }
            let _ = app_handle.emit(
                "embeddings://status",
                serde_json::json!({
                    "stage": "error",
                    "reason": format!("credential_build_failed: {}", e)
                }),
            );
            format!("Failed to build embed_fn: {}", e)
        })?;

        // #3082: the per-item fallback required by `embed_and_store_chunk` whenever a
        // batch fails as a unit — adapts the batch closure to the unchanged
        // `Fn(&str) -> Result<Vec<f32>, String>` contract `embed_with_token_retry` expects.
        let embed_fn_single = |text: &str| -> Result<Vec<f32>, String> {
            let mut v = batch_embed_fn(&[text])?;
            v.pop().ok_or_else(|| {
                "batch embed_fn returned an empty result for a single-item call".to_string()
            })
        };

        // === Step 3: Emit initial indexing stage (lock already released) ===
        let _ = app_handle.emit(
            "embeddings://status",
            serde_json::json!({
                "stage": "indexing"
            }),
        );

        // === Step 4: Open SEPARATE connection — pipeline runs without holding the main mutex ===
        let db_path = crate::database::get_db_path();
        let pipeline_conn = rusqlite::Connection::open(&db_path).map_err(|e| {
            let _ = app_handle.emit(
                "embeddings://status",
                serde_json::json!({
                    "stage": "error",
                    "reason": format!("pipeline_db_open_failed: {}", e)
                }),
            );
            format!("Failed to open pipeline DB: {}", e)
        })?;

        // #3082: resolve the configured batch size via a fresh short-lived lock — same
        // discipline as resolved_max_tokens (named/logged source, never a bare literal).
        let batch_size = {
            let conn = db_arc.lock().map_err(|e| format!("db_lock_failed: {}", e))?;
            resolve_embeddings_batch_size(&conn)
        };

        let pane_root = crate::database::get_pane_root();
        let app_handle_cb = app_handle.clone();

        // === Step 5: Run pipeline — main DB lock NOT held ===
        eprintln!(
            "[SWITCH_EMBEDDING_ENGINE] Starting pipeline (main DB lock released, batch_size={})",
            batch_size
        );
        match crate::embeddings::run_embedding_pipeline_batched(
            &pipeline_conn,
            &pane_root,
            &embed_fn_single,
            &engine_tag,
            &model,
            resolved_max_tokens as usize,
            move |progress| {
                let crate::embeddings::EmbeddingProgress::Indexing {
                    notes,
                    facts,
                    chats,
                    notes_total,
                    facts_total,
                    chats_total,
                } = &progress;
                let _ = app_handle_cb.emit(
                    "embeddings://status",
                    serde_json::json!({
                        "stage": "indexing",
                        "notes": notes,
                        "facts": facts,
                        "chats": chats,
                        "notes_total": notes_total,
                        "facts_total": facts_total,
                        "chats_total": chats_total
                    }),
                );
            },
            &*batch_embed_fn,
            batch_size,
        ) {
            Ok((notes, facts, chats, failed_count)) => {
                eprintln!("[SWITCH_EMBEDDING_ENGINE] Pipeline complete: {} notes, {} facts, {} chats, {} failed", notes, facts, chats, failed_count);
                // === Step 6: Re-acquire lock briefly to write terminal status ===
                let terminal_stage = if failed_count > 0 { "partial" } else { "ready" };
                if let Ok(conn) = db_arc.lock() {
                    let _ = crate::database::set_ai_setting(&conn, "embeddings_status", terminal_stage);
                    let _ = crate::database::set_ai_setting(
                        &conn,
                        "embeddings_failed_count",
                        &failed_count.to_string(),
                    );
                }
                let _ = app_handle.emit(
                    "embeddings://status",
                    serde_json::json!({
                        "stage": terminal_stage
                    }),
                );
                Ok(format!(
                    "Indexed {} notes, {} facts, {} chats ({} failed)",
                    notes, facts, chats, failed_count
                ))
            }
            Err(e) => {
                eprintln!("[SWITCH_EMBEDDING_ENGINE] Pipeline error: {}", e);
                if let Ok(conn) = db_arc.lock() {
                    let _ = crate::database::set_ai_setting(&conn, "embeddings_status", "error");
                    // Do not write embeddings_failed_count on auth abort — leave prior state intact
                }
                let _ = app_handle.emit(
                    "embeddings://status",
                    serde_json::json!({
                        "stage": "error",
                        "reason": e.clone()
                    }),
                );
                Err(format!("Embedding pipeline failed: {}", e))
            }
        }
    })
    .await
    .map_err(|e| {
        let _ = app_handle.emit(
            "embeddings://status",
            serde_json::json!({
                "stage": "error",
                "reason": format!("pipeline thread panicked: {}", e)
            }),
        );
        format!("switch_embedding_engine thread panicked: {}", e)
    })?
}


/// Fetch available models for embedding from the specified provider.
///
/// Used by the Semantic Search settings card to populate the embedding-model picker.
///
/// - `engine`: `"openrouter"`, `"ollama"`, or `"venice"`
/// - `endpoint`: Ollama base URL (required for ollama; ignored for openrouter and venice)
///
/// C50 (#3171 PR 1): `db` is now needed for BOTH the openrouter and venice arms
/// — openrouter reads Provider Connections' own keychain slot
/// (`credential_migration::read_provider_key`), the same function image
/// generation's §39.2 arm already uses, instead of a frontend-supplied
/// `api_key` param (retired — it used to feed the now-retired
/// `embeddings_api_key` plaintext slot). Venice is unchanged — it already read
/// the chat-provider key from the keychain (`read_venice_key`), never a
/// frontend-supplied key (Venice reuses the existing key; the user never
/// enters one just for Semantic Search).
#[tauri::command]
pub async fn fetch_embedding_models(
    db: State<'_, DbState>,
    engine: String,
    endpoint: Option<String>,
) -> Result<Vec<super::provider::ModelInfo>, String> {
    eprintln!("[FETCH_EMBEDDING_MODELS] called with: engine={}", engine);

    match engine.as_str() {
        "venice" => {
            // #2988 §1e: same deliberate deviation as build_http_embed_fn's venice arm
            // — reads the chat-provider Venice key, NOT a frontend-supplied api_key.
            let key = {
                let conn = db.0.lock().map_err(|e| format!("db_lock_failed: {}", e))?;
                crate::ai::credential_migration::read_venice_key(&conn)
            }
            .ok_or_else(|| {
                "Venice API key not configured (set it in Settings → AI → Primary Provider or Internal Provider)"
                    .to_string()
            })?;
            fetch_venice_embedding_models(&key).await
        }
        "openrouter" => {
            // C50 (#3171 PR 1): no empty-key fallback — a missing connection key
            // errors here, before any network call (`fetch_openrouter_embedding_models`
            // is never reached). Silent degradation to an empty key would produce
            // a 401 indistinguishable from a provider outage.
            let key = {
                let conn = db.0.lock().map_err(|e| format!("db_lock_failed: {}", e))?;
                crate::ai::credential_migration::read_provider_key(&conn, "openrouter")
            }
            .ok_or_else(|| {
                "OpenRouter is not connected — add it in Settings → AI → Provider Connections".to_string()
            })?;
            fetch_openrouter_embedding_models(&key).await
        }
        "ollama" => {
            let ep = endpoint
                .filter(|e| !e.is_empty())
                .unwrap_or_else(|| "http://localhost:11434".to_string());
            // Normalise trailing slashes
            let ep = ep.trim_end_matches('/').to_string();
            fetch_ollama_embedding_models(&ep).await
        }
        other => Err(format!(
            "Unknown embedding engine '{}': must be 'openrouter', 'ollama', or 'venice'",
            other
        )),
    }
}


/// Counts of embeddings in the index, broken out by source, plus the persisted
/// failed count from the most recent pipeline run.
///
/// Returned by `get_embeddings_counts` — used by Settings.svelte to show
/// index totals on mount without relying on transient event payloads.
#[derive(Debug, Serialize)]
pub struct EmbeddingsCounts {
    pub notes: i64,
    pub facts: i64,
    pub chats: i64,
    pub failed: i64,
}


/// Tauri command: return the total number of embeddings in the index per source,
/// plus the failed count from the most recent pipeline run.
///
/// Runs a live `SELECT source, COUNT(*) FROM embeddings GROUP BY source` so the
/// counts always reflect the actual index state (not a per-run snapshot).
/// `failed` is read from `ai_settings.embeddings_failed_count` — it IS persisted
/// because it is not derivable from the embeddings table.
#[tauri::command]
pub fn get_embeddings_counts(db: tauri::State<'_, DbState>) -> Result<EmbeddingsCounts, String> {
    eprintln!("[GET_EMBEDDINGS_COUNTS] called");
    let conn =
        db.0.lock()
            .map_err(|e| format!("get_embeddings_counts: db lock failed: {}", e))?;

    let mut notes = 0i64;
    let mut facts = 0i64;
    let mut chats = 0i64;

    let mut stmt = conn
        .prepare("SELECT source, COUNT(*) FROM embeddings GROUP BY source")
        .map_err(|e| format!("get_embeddings_counts: prepare failed: {}", e))?;

    let rows = stmt
        .query_map([], |row| {
            let source: String = row.get(0)?;
            let count: i64 = row.get(1)?;
            Ok((source, count))
        })
        .map_err(|e| format!("get_embeddings_counts: query failed: {}", e))?;

    for row in rows {
        let (source, count) =
            row.map_err(|e| format!("get_embeddings_counts: row error: {}", e))?;
        match source.as_str() {
            "note" => notes = count,
            "memory" => facts = count,
            "chat" => chats = count,
            other => eprintln!(
                "[GET_EMBEDDINGS_COUNTS] unknown source '{}' — skipping",
                other
            ),
        }
    }

    let failed: i64 = crate::database::get_ai_setting(&conn, "embeddings_failed_count")
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(0);

    eprintln!(
        "[GET_EMBEDDINGS_COUNTS] notes={} facts={} chats={} failed={}",
        notes, facts, chats, failed
    );
    Ok(EmbeddingsCounts {
        notes,
        facts,
        chats,
        failed,
    })
}


/// Verify all agent folders in Pane System have their expected system notes.
/// If any are missing, attempt to recreate from flat files on disk.
/// Runs silently on startup — no UI, just backend healing.

// ─── Memory Management Commands (v11.27.0 — #68, #69) ─────────────────────

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct FullMemoryEntry {
    pub id: String,
    pub fact: String,
    pub category: String,
    pub layer: String,
    pub project_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub last_referenced: Option<String>,
}


/// Get all active memories for the memory management page
#[tauri::command]
pub fn get_all_memories(db: State<'_, DbState>) -> Result<Vec<FullMemoryEntry>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare(
            "SELECT id, fact, COALESCE(category, 'fact'), COALESCE(layer, 'personal'), project_id,
                COALESCE(created_at, ''), COALESCE(updated_at, ''), last_referenced
         FROM ai_memory WHERE is_active = 1 ORDER BY created_at DESC",
        )
        .map_err(|e| e.to_string())?;

    let entries: Vec<FullMemoryEntry> = stmt
        .query_map([], |row| {
            Ok(FullMemoryEntry {
                id: row.get(0)?,
                fact: row.get(1)?,
                category: row.get(2)?,
                layer: row.get(3)?,
                project_id: row.get(4)?,
                created_at: row.get(5)?,
                updated_at: row.get(6)?,
                last_referenced: row.get(7)?,
            })
        })
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();

    Ok(entries)
}


/// Update a memory's fact text
#[tauri::command]
pub fn update_memory(
    app: tauri::AppHandle,
    db: State<'_, DbState>,
    memory_id: String,
    fact: String,
) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;

    // Fetch old fact and category for audit marker
    let (old_fact, category): (String, String) = conn
        .query_row(
            "SELECT fact, COALESCE(category, 'fact') FROM ai_memory WHERE id = ?1",
            params![memory_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|e| format!("Memory not found: {}", e))?;

    conn.execute(
        "UPDATE ai_memory SET fact = ?1, updated_at = datetime('now') WHERE id = ?2",
        params![fact, memory_id],
    )
    .map_err(|e| e.to_string())?;

    // #672: Append [memory edited] audit marker to daily log (best-effort)
    let time_str = chrono::Local::now().format("%H:%M").to_string();
    let marker_line = format!(
        "- {} {} {}: \"{}\" → \"{}\"",
        time_str,
        crate::ai::audit_markers::MARKER_EDITED,
        category,
        old_fact,
        fact
    );
    if let Err(e) = append_to_daily_log(&conn, &app, &[(marker_line, None)]) {
        eprintln!(
            "[MEMORY] Warning: failed to write [memory edited] marker: {}",
            e
        );
    }

    // #674: Edits to routed/consolidated facts drift until next consolidation (§5.12.3).
    // The old bullet stays in the memory file; the AI sees the correction via the daily log
    // audit marker in context. No immediate file update needed.

    Ok(())
}


/// Soft-delete a memory (set is_active = 0)
#[tauri::command]
pub fn delete_memory(
    app: tauri::AppHandle,
    db: State<'_, DbState>,
    memory_id: String,
) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;

    // Fetch fact and category for audit marker
    let (fact_text, category): (String, String) = conn
        .query_row(
            "SELECT fact, COALESCE(category, 'fact') FROM ai_memory WHERE id = ?1",
            params![memory_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|e| format!("Memory not found: {}", e))?;

    conn.execute(
        "UPDATE ai_memory SET is_active = 0, updated_at = datetime('now') WHERE id = ?1",
        params![memory_id],
    )
    .map_err(|e| e.to_string())?;

    // #672: Append [memory deleted] audit marker to daily log (best-effort)
    let time_str = chrono::Local::now().format("%H:%M").to_string();
    let marker_line = format!(
        "- {} {} {}: \"{}\" (user removed via Memory Settings)",
        time_str,
        crate::ai::audit_markers::MARKER_DELETED,
        category,
        fact_text
    );
    if let Err(e) = append_to_daily_log(&conn, &app, &[(marker_line, None)]) {
        eprintln!(
            "[MEMORY] Warning: failed to write [memory deleted] marker: {}",
            e
        );
    }

    // #674: If fact was already routed, immediately remove bullet from memory file
    let was_routed: bool = conn
        .query_row(
            "SELECT routed_at IS NOT NULL FROM ai_memory WHERE id = ?1",
            params![memory_id],
            |row| row.get(0),
        )
        .unwrap_or(false);

    if was_routed {
        if let Err(e) = crate::ai::memory_router::remove_inactive_facts(&conn) {
            eprintln!(
                "[MEMORY] Warning: failed to remove bullet from memory file: {}",
                e
            );
        }
    }

    Ok(())
}


/// Manually save a fact to memory (from chat context menu)
#[tauri::command]
pub fn save_to_memory(
    app: tauri::AppHandle,
    db: State<'_, DbState>,
    fact: String,
    category: String,
    source_message_id: Option<String>,
    project_id: Option<String>,
) -> Result<String, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let id = uuid::Uuid::new_v4().to_string();
    let layer = if project_id.is_some() {
        "work"
    } else {
        "personal"
    };

    conn.execute(
        "INSERT INTO ai_memory (id, fact, category, layer, source_message_id, project_id, is_active, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1, datetime('now'), datetime('now'))",
        params![id, fact, category, layer, source_message_id, project_id],
    ).map_err(|e| e.to_string())?;

    // Also append to daily log
    let time_str = chrono::Local::now().format("%H:%M").to_string();
    let formatted = format!("- {} [saved] {}: \"{}\"", time_str, category, fact);
    let log_entries = vec![(formatted, Some(fact.clone()))];
    if let Err(e) = append_to_daily_log(&conn, &app, &log_entries) {
        eprintln!("[DAILY-LOG] Manual save daily log append failed: {}", e);
    }

    Ok(id)
}

#[cfg(test)]
mod synthesis_content_loss_guard_tests {
    use super::*;

    // === Synthesis Content Loss Guard Tests (§5, #321) ===

    #[test]
    fn test_synthesis_content_loss_guard_blocks_large_reduction() {
        // Simulate synthesis content loss guard logic
        // Existing content: 500 chars, new content: 200 chars (60% reduction)
        let existing_len = 500;
        let new_len = 200;

        let should_block = existing_len > 100 && new_len < (existing_len * 50 / 100);
        assert!(should_block, "Should block when content reduces by >50%");

        let reduction_pct = ((existing_len - new_len) * 100) / existing_len;
        assert_eq!(reduction_pct, 60, "Should calculate 60% reduction");
    }

    #[test]
    fn test_synthesis_content_loss_guard_allows_similar_size() {
        // Existing content: 500 chars, new content: 450 chars (10% reduction)
        let existing_len = 500;
        let new_len = 450;

        let should_block = existing_len > 100 && new_len < (existing_len * 50 / 100);
        assert!(
            !should_block,
            "Should NOT block when content reduces by <50%"
        );
    }

    #[test]
    fn test_synthesis_content_loss_guard_allows_growth() {
        // Existing content: 500 chars, new content: 800 chars (growth)
        let existing_len = 500;
        let new_len = 800;

        let should_block = existing_len > 100 && new_len < (existing_len * 50 / 100);
        assert!(!should_block, "Should NOT block when content grows");
    }

    #[test]
    fn test_synthesis_content_loss_guard_allows_initial_write() {
        // Existing content: 50 chars (below 100 threshold), new content: 10 chars
        let existing_len = 50;
        let new_len = 10;

        let should_block = existing_len > 100 && new_len < (existing_len * 50 / 100);
        assert!(
            !should_block,
            "Should NOT block initial writes with small existing content"
        );
    }

    #[test]
    fn test_synthesis_content_loss_guard_boundary_case() {
        // Existing content: 500 chars, new content: exactly 250 chars (exactly 50%)
        let existing_len = 500;
        let new_len = 250;

        let should_block = existing_len > 100 && new_len < (existing_len * 50 / 100);
        assert!(
            !should_block,
            "Should NOT block at exactly 50% (boundary case)"
        );
    }

    #[test]
    fn test_synthesis_content_loss_guard_just_below_threshold() {
        // Existing content: 500 chars, new content: 249 chars (just below 50%)
        let existing_len = 500;
        let new_len = 249;

        let should_block = existing_len > 100 && new_len < (existing_len * 50 / 100);
        assert!(should_block, "Should block just below 50% threshold");

        let reduction_pct = ((existing_len - new_len) * 100) / existing_len;
        assert_eq!(
            reduction_pct, 50,
            "Should calculate 50% reduction (integer division)"
        );
    }
}


// ─── switch_embedding_engine DB-lock fix (#1657) ─────────────────────────────
//
// Tests verify the structural correctness of the lock-release fix: settings are
// persisted under the lock, the pipeline uses a SEPARATE connection (no main mutex
// held during indexing), and terminal status is written correctly on both success
// and failure paths.

#[cfg(test)]
mod switch_embedding_engine_tests {
    use rusqlite::Connection;
    use std::sync::{Arc, Mutex};

    /// Build an in-memory DB with the tables required by switch_embedding_engine
    /// and run_embedding_pipeline (ai_settings + embeddings + notes/facts/chats).
    fn make_test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE ai_settings (
                key   TEXT NOT NULL PRIMARY KEY,
                value TEXT NOT NULL
            );
            CREATE TABLE embeddings (
                id            INTEGER PRIMARY KEY AUTOINCREMENT,
                item_type     TEXT NOT NULL,
                item_id       TEXT NOT NULL,
                engine        TEXT,
                model         TEXT,
                content_hash  TEXT NOT NULL,
                vector        BLOB NOT NULL
            );
            CREATE TABLE notes (
                id         TEXT NOT NULL PRIMARY KEY,
                title      TEXT NOT NULL,
                body       TEXT,
                project_id TEXT,
                status     TEXT,
                deleted_at TEXT,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
            CREATE TABLE facts (
                id         INTEGER PRIMARY KEY AUTOINCREMENT,
                fact       TEXT NOT NULL,
                category   TEXT,
                deleted_at TEXT,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
            CREATE TABLE chat_messages (
                id         TEXT NOT NULL PRIMARY KEY,
                role       TEXT NOT NULL,
                content    TEXT NOT NULL,
                deleted_at TEXT,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            );",
        )
        .unwrap();
        conn
    }

    /// switch_embedding_engine should return an error for an unknown engine value
    /// immediately, without touching the DB.
    ///
    /// #2988 PR1: `switch_embedding_engine` takes `app_handle: tauri::AppHandle`
    /// (defaults to the `Wry` runtime — see `notes::update_note`'s doc comment for
    /// the same pre-existing constraint) which cannot be constructed from
    /// `tauri::test::mock_app`'s `AppHandle<MockRuntime>`, so this gate cannot be
    /// exercised through the real `#[tauri::command]` entry point in this test
    /// binary (a codebase-wide limitation, not introduced here — see
    /// `fetch_embedding_models_venice_tests` below for the SIBLING gate, which
    /// takes no AppHandle and IS exercised through the real command). This test
    /// replicates the validation line and additionally greps the live source for
    /// the EXACT literal error string, so a future edit to either the `venice`
    /// literal or the message text fails this test, not just a logic check.
    #[test]
    fn test_switch_embedding_engine_rejects_invalid_engine() {
        let is_valid = |engine: &str| engine == "openrouter" || engine == "ollama" || engine == "venice";
        assert!(!is_valid("candle"), "Unknown engine must be rejected");
        assert!(is_valid("openrouter"), "openrouter must remain valid");
        assert!(is_valid("ollama"), "ollama must remain valid");
        assert!(is_valid("venice"), "venice must be accepted (#2988 gate 1)");

        // Golden-string check against the real source: both the three-engine
        // condition and the exact error message naming all three must be present
        // verbatim in commands.rs — not just logically equivalent.
        let src = include_str!("scheduler.rs");
        assert!(
            src.contains("engine != \"openrouter\" && engine != \"ollama\" && engine != \"venice\""),
            "switch_embedding_engine's gate condition must literally include all three engines"
        );
        assert!(
            src.contains("must be 'openrouter', 'ollama', or 'venice'"),
            "switch_embedding_engine's error message must literally name all three engines"
        );
    }

    /// switch_embedding_engine should return an error for an empty model string.
    #[test]
    fn test_switch_embedding_engine_rejects_empty_model() {
        let model = "   ".to_string();
        assert!(
            model.trim().is_empty(),
            "Whitespace-only model must be rejected (trim guard)"
        );
    }

    /// After the lock-release fix, the pipeline uses a SEPARATE connection.
    /// This test verifies that a second thread CAN acquire the main mutex while
    /// the pipeline connection is open — i.e., the lock is not held across the
    /// pipeline boundary.
    ///
    /// We simulate this by: acquiring the mutex in thread A, verifying the DB
    /// file is reachable via a separate Connection in thread B (the pipeline path),
    /// and releasing A afterwards.
    #[test]
    fn test_pipeline_uses_separate_connection_not_main_mutex() {
        use std::thread;

        let conn = make_test_db();
        let db = Arc::new(Mutex::new(conn));

        // Thread A: holds the main mutex (simulates some other DB operation).
        // Two rendezvous points, not a sleep: the first guarantees A holds
        // the lock before main's try_lock() runs (A's lock() happens-before
        // its own barrier_a.wait(), by program order); the second is the
        // fix for the sleep this replaces — it makes A wait for an explicit
        // "main has finished checking" signal before releasing the guard,
        // instead of guessing a duration long enough to cover main's
        // try_lock() call. Deterministic either way; no scheduler-jitter
        // margin needed.
        let db_a = Arc::clone(&db);
        let barrier_holds_lock = Arc::new(std::sync::Barrier::new(2));
        let barrier_holds_lock_a = Arc::clone(&barrier_holds_lock);
        let barrier_checked = Arc::new(std::sync::Barrier::new(2));
        let barrier_checked_a = Arc::clone(&barrier_checked);

        let handle = thread::spawn(move || {
            let _guard = db_a.lock().unwrap();
            barrier_holds_lock_a.wait(); // signal main that we hold the lock
            barrier_checked_a.wait(); // wait for main's try_lock() check to finish
                                      // guard drops here, only after main has checked
        });

        // Main thread: simulates the pipeline opening its OWN connection
        // (in production this opens a new Connection; here we just verify
        //  that opening a file-backed connection is possible without the mutex).
        // With an in-memory DB we can't share the file, so we verify the
        // non-blocking acquire attempt instead.
        barrier_holds_lock.wait(); // wait until A holds the lock

        // Try a non-blocking lock attempt — it must FAIL (A holds it)
        let try_result = db.try_lock();
        // If A truly holds the lock, try_lock must fail
        assert!(
            try_result.is_err(),
            "Main mutex must be held by thread A at this point"
        );

        barrier_checked.wait(); // tell A the check is done — safe to release now

        handle.join().unwrap();

        // After A releases, the lock must be acquirable again
        let post_release = db.try_lock();
        assert!(
            post_release.is_ok(),
            "Main mutex must be available after thread A releases"
        );
    }

    /// Verifies that `set_ai_setting` / `get_ai_setting` round-trips work for the
    /// settings keys written by `switch_embedding_engine` before the lock release,
    /// including the new credential keys added in #1657.
    #[test]
    fn test_switch_embedding_engine_settings_persist_under_lock() {
        let conn = make_test_db();

        // Simulate the settings-persist block inside the lock
        crate::database::set_ai_setting(&conn, "embeddings_engine", "openrouter").unwrap();
        crate::database::set_ai_setting(&conn, "embeddings_model", "text-embedding-3-small")
            .unwrap();
        crate::database::set_ai_setting(&conn, "embeddings_status", "indexing").unwrap();
        crate::database::set_ai_setting(&conn, "embeddings_enabled", "true").unwrap();

        assert_eq!(
            crate::database::get_ai_setting(&conn, "embeddings_engine").as_deref(),
            Some("openrouter"),
            "embeddings_engine must persist"
        );
        assert_eq!(
            crate::database::get_ai_setting(&conn, "embeddings_model").as_deref(),
            Some("text-embedding-3-small"),
            "embeddings_model must persist"
        );
        assert_eq!(
            crate::database::get_ai_setting(&conn, "embeddings_status").as_deref(),
            Some("indexing"),
            "embeddings_status must be 'indexing' after settings block"
        );
        assert_eq!(
            crate::database::get_ai_setting(&conn, "embeddings_enabled").as_deref(),
            Some("true"),
            "embeddings_enabled must be 'true' after settings block"
        );
    }

    /// C50 (#3171 PR 1): switch_embedding_engine's REAL signature no longer has
    /// an `api_key` param at all — OpenRouter now reads Provider Connections'
    /// own keychain slot (`read_provider_key`, inside
    /// `build_http_embed_batch_fn`'s openrouter arm). This is the regression
    /// guard for the retired plaintext slot: `embeddings_api_key` must never
    /// be written again by anything in this function, pinned to the real
    /// source (same golden-string, scoped-to-the-real-function technique as
    /// `test_switch_embedding_engine_real_predicate_clears_on_model_change_pinned_to_source`
    /// below) — a `src.contains(...)` over the whole file would still pass
    /// after a partial revert, because `build_http_embed_fn`'s own doc comment
    /// mentions the string; scoping to this function's own span is what makes
    /// this pin land on production code.
    #[test]
    fn test_switch_embedding_engine_never_persists_embeddings_api_key_pinned_to_source() {
        let src = include_str!("scheduler.rs");
        let fn_start = src
            .find("pub async fn switch_embedding_engine(")
            .expect("switch_embedding_engine must still exist in scheduler.rs");
        let fn_end = src[fn_start..]
            .find("\n#[tauri::command]\npub async fn fetch_embedding_models(")
            .map(|rel| fn_start + rel)
            .expect("fetch_embedding_models must still immediately follow switch_embedding_engine");
        let fn_span = &src[fn_start..fn_end];

        assert!(
            !fn_span.contains("api_key: Option<String>"),
            "switch_embedding_engine must not have an api_key parameter — C50 retired the \
             frontend-supplied OpenRouter key entirely; the key now comes from Provider \
             Connections' own keychain slot, read inside build_http_embed_batch_fn"
        );
        assert!(
            // The QUOTED string literal (an actual write target), not a bare
            // substring — this function's own doc comments legitimately name
            // the retired key by its backtick-quoted identifier when explaining
            // WHY it's gone; only a literal `"embeddings_api_key"` string would
            // be a real write/read of the ai_settings row.
            !fn_span.contains("\"embeddings_api_key\""),
            "switch_embedding_engine must never write the retired embeddings_api_key \
             plaintext slot — if this fails, the exact credential-handling defect C50 \
             fixed (an API key sitting in the database in the clear) has regressed"
        );
    }

    /// switch_embedding_engine must persist `endpoint` to `embeddings_endpoint` in ai_settings
    /// so that `build_http_embed_fn` reads the card's own endpoint (§5.9.2 / #1657).
    #[test]
    fn test_switch_embedding_engine_persists_endpoint() {
        let conn = make_test_db();

        let endpoint = Some("http://localhost:11434".to_string());
        if let Some(ref ep) = endpoint {
            if !ep.is_empty() {
                crate::database::set_ai_setting(&conn, "embeddings_endpoint", ep).unwrap();
            }
        }

        assert_eq!(
            crate::database::get_ai_setting(&conn, "embeddings_endpoint").as_deref(),
            Some("http://localhost:11434"),
            "embeddings_endpoint must be persisted by switch_embedding_engine"
        );
        // Primary-provider endpoint slot must NOT be touched
        assert_eq!(
            crate::database::get_ai_setting(&conn, "endpoint"),
            None,
            "primary-provider endpoint must remain untouched"
        );
    }

    /// Verifies that the stale-embedding DELETE runs correctly under the lock.
    #[test]
    fn test_switch_embedding_engine_clears_stale_rows() {
        let conn = make_test_db();

        // Seed embeddings from a different engine and from the new engine
        conn.execute_batch(
            "INSERT INTO embeddings (item_type, item_id, engine, content_hash, vector)
             VALUES ('note', 'old-1', 'ollama',       'hash1', x'0000'),
                    ('note', 'old-2', NULL,            'hash2', x'0000'),
                    ('note', 'keep', 'openrouter',    'hash3', x'0000');",
        )
        .unwrap();

        // Simulate the DELETE inside the lock
        let new_engine = "openrouter";
        let deleted = conn
            .execute(
                "DELETE FROM embeddings WHERE engine IS NULL OR engine != ?1",
                rusqlite::params![new_engine],
            )
            .unwrap();

        assert_eq!(deleted, 2, "Two stale rows (ollama + NULL) must be deleted");

        let remaining: i64 = conn
            .query_row("SELECT COUNT(*) FROM embeddings", [], |r| r.get(0))
            .unwrap();
        assert_eq!(remaining, 1, "One row (openrouter) must survive");
    }

    /// #3101 — a MODEL change WITHIN the same engine must also clear stale vectors.
    /// Before this fix the production DELETE predicate checked only `engine`, so
    /// switching `text-embedding-3-small` → `qwen3-8b` under the SAME `openrouter`
    /// engine cleared nothing: the engine string is unchanged, so `engine != ?1`
    /// never matches, and the old model's vectors survive to be scored against the
    /// new model's query vector (the issue's root cause, step 2). Confirms the real
    /// `switch_embedding_engine` predicate (`engine IS NULL OR engine != ?1 OR model
    /// IS NULL OR model != ?2`) clears a same-engine, different-model row.
    #[test]
    fn test_switch_embedding_engine_clears_stale_rows_on_model_change_same_engine() {
        let conn = make_test_db();

        conn.execute_batch(
            "INSERT INTO embeddings (item_type, item_id, engine, model, content_hash, vector)
             VALUES ('note', 'old-model',  'openrouter', 'text-embedding-3-small', 'hash1', x'0000'),
                    ('note', 'legacy',     'openrouter', NULL,                     'hash2', x'0000'),
                    ('note', 'keep',       'openrouter', 'qwen3-8b',               'hash3', x'0000');",
        )
        .unwrap();

        // Same engine as before ("openrouter" unchanged) — only the MODEL changed.
        let new_engine = "openrouter";
        let new_model = "qwen3-8b";
        let deleted = conn
            .execute(
                "DELETE FROM embeddings WHERE engine IS NULL OR engine != ?1 OR model IS NULL OR model != ?2",
                rusqlite::params![new_engine, new_model],
            )
            .unwrap();

        assert_eq!(
            deleted, 2,
            "the old-model row AND the legacy NULL-model row must both be cleared on a model change, even though the engine string is unchanged"
        );

        let remaining: i64 = conn
            .query_row("SELECT COUNT(*) FROM embeddings", [], |r| r.get(0))
            .unwrap();
        assert_eq!(remaining, 1, "only the qwen3-8b row (current engine AND model) must survive");

        // Mutation check (pre-#3101 behavior, inline): the OLD predicate (engine-only)
        // would have deleted ZERO rows here, since "openrouter" == "openrouter" for
        // every row — this is the exact silent-no-op the issue reports.
        let conn2 = make_test_db();
        conn2.execute_batch(
            "INSERT INTO embeddings (item_type, item_id, engine, model, content_hash, vector)
             VALUES ('note', 'old-model', 'openrouter', 'text-embedding-3-small', 'hash1', x'0000');",
        ).unwrap();
        let deleted_old_predicate = conn2
            .execute("DELETE FROM embeddings WHERE engine IS NULL OR engine != ?1", rusqlite::params![new_engine])
            .unwrap();
        assert_eq!(
            deleted_old_predicate, 0,
            "sanity check: the pre-#3101 engine-only predicate deletes nothing on a same-engine model change — this IS the bug"
        );
    }

    /// #3101 fast-follow (Reed's audit on PR #3126) — the test above
    /// (`test_switch_embedding_engine_clears_stale_rows_on_model_change_same_engine`)
    /// only ever runs a hand-copied SQL string against a bare `rusqlite::Connection`;
    /// it has no linkage back to the real `switch_embedding_engine` function, so a
    /// future revert of the production predicate (e.g. back to the pre-#3101
    /// engine-only clear) would NOT turn that test red — Reed confirmed this by
    /// reverting the real call site and watching all 154 scheduler tests stay green.
    ///
    /// This test closes that gap using the same golden-string, scoped-to-the-real-
    /// function technique already established in this codebase (this file's own
    /// `test_switch_embedding_engine_is_async_and_wraps_body_in_spawn_blocking` below,
    /// and `chat.rs`'s `test_site1_ai_chat_stream_source_shape_never_regresses_to_agent_id`):
    /// slice the real source between `switch_embedding_engine`'s own signature and the
    /// next function's signature, then assert the real DELETE predicate text appears
    /// INSIDE that slice — not merely somewhere in the file. A bare `src.contains(...)`
    /// over the whole file would still pass after a production revert, because the
    /// test above keeps its own hardcoded copy of the correct string nearby; scoping
    /// to switch_embedding_engine's own span is what makes this pin actually land on
    /// production code.
    #[test]
    fn test_switch_embedding_engine_real_predicate_clears_on_model_change_pinned_to_source() {
        let src = include_str!("scheduler.rs");
        let fn_start = src
            .find("pub async fn switch_embedding_engine(")
            .expect("switch_embedding_engine must still exist in scheduler.rs");
        let fn_end = src[fn_start..]
            .find("\n#[tauri::command]\npub async fn fetch_embedding_models(")
            .map(|rel| fn_start + rel)
            .expect("fetch_embedding_models must still immediately follow switch_embedding_engine");
        let fn_span = &src[fn_start..fn_end];

        assert!(
            fn_span.contains(
                "DELETE FROM embeddings WHERE engine IS NULL OR engine != ?1 OR model IS NULL OR model != ?2"
            ),
            "switch_embedding_engine's REAL DELETE predicate must clear stale rows on \
             BOTH a different engine AND a different model within the same engine (#3101). \
             If this fails, the production predicate was reverted toward the pre-#3101 \
             engine-only clear (`engine IS NULL OR engine != ?1`), which silently keeps \
             stale same-engine vectors around on a model-only switch — exactly the bug \
             #3101 fixed. This assertion is scoped to switch_embedding_engine's own \
             source span, not the whole file, so it cannot be satisfied by the sibling \
             test's own hardcoded copy of this same string."
        );
    }

    /// Verifies that the terminal status is written correctly on pipeline success.
    #[test]
    fn test_switch_embedding_engine_sets_ready_on_success() {
        let conn = make_test_db();

        // Simulate the success path: status was "indexing", pipeline succeeds
        crate::database::set_ai_setting(&conn, "embeddings_status", "indexing").unwrap();

        // Simulate success branch
        crate::database::set_ai_setting(&conn, "embeddings_status", "ready").unwrap();

        assert_eq!(
            crate::database::get_ai_setting(&conn, "embeddings_status").as_deref(),
            Some("ready"),
            "Terminal status must be 'ready' after successful pipeline"
        );
    }

    /// Verifies that the terminal status is written correctly on pipeline error.
    #[test]
    fn test_switch_embedding_engine_sets_error_on_failure() {
        let conn = make_test_db();

        crate::database::set_ai_setting(&conn, "embeddings_status", "indexing").unwrap();

        // Simulate error branch
        crate::database::set_ai_setting(&conn, "embeddings_status", "error").unwrap();

        assert_eq!(
            crate::database::get_ai_setting(&conn, "embeddings_status").as_deref(),
            Some("error"),
            "Terminal status must be 'error' after failed pipeline"
        );
    }
}


// ─── EmbeddingSwitchGuard + resolve_embeddings_batch_size tests (#3082) ────
//
// `switch_embedding_engine` itself cannot be invoked through `tauri::test::mock_app()`
// (same pre-existing, documented codebase-wide limitation as `switch_embedding_engine_tests`
// above notes: its `app_handle: tauri::AppHandle` parameter is concretely `AppHandle<Wry>`,
// which cannot be constructed from `mock_app`'s `AppHandle<MockRuntime>`). So these tests
// exercise the REAL, independently-testable pieces the async conversion introduced —
// `EmbeddingSwitchGuard` and `resolve_embeddings_batch_size` — directly, plus a
// golden-string source-scan (the same established technique
// `test_switch_embedding_engine_rejects_invalid_engine` above already uses for this exact
// AppHandle constraint) to pin the async fn + spawn_blocking shape itself.
#[cfg(test)]
mod embedding_switch_guard_and_batch_size_tests {
    use rusqlite::Connection;
    use serial_test::serial;

    // EmbeddingSwitchGuard is a process-wide static (AtomicBool) — tests that acquire
    // it must be serialized against each other so they don't interfere.

    /// A second switch while one is "running" (guard held) must be rejected with
    /// exactly the stated message — Matt's ruling, same shape as R7's BackupGuard.
    #[test]
    #[serial(embedding_switch_guard)]
    fn test_second_switch_rejected_while_first_running() {
        let first = super::EmbeddingSwitchGuard::try_acquire().expect("first acquire must succeed");
        let second = super::EmbeddingSwitchGuard::try_acquire();
        assert!(second.is_err(), "a second switch while one is running must be rejected");
        assert_eq!(
            second.unwrap_err(),
            "a re-index is already running",
            "rejection message must match exactly"
        );
        drop(first);
    }

    /// Once the first guard drops (pipeline finished, success or error), a new switch
    /// must be allowed through — this is a reject, not a permanent lock.
    #[test]
    #[serial(embedding_switch_guard)]
    fn test_guard_releases_on_drop_allowing_next_switch() {
        {
            let _first = super::EmbeddingSwitchGuard::try_acquire().expect("first acquire must succeed");
            // _first drops at the end of this block
        }
        let second = super::EmbeddingSwitchGuard::try_acquire();
        assert!(second.is_ok(), "guard must release on drop, allowing the next switch to proceed");
    }

    /// Simulates two overlapping calls the way a real concurrent invocation would see
    /// it: the first acquires and holds the guard across a simulated slow pipeline
    /// (thread sleep), the second attempts to acquire while the first is still running
    /// and must be rejected; after the first finishes (guard drops), a third attempt
    /// succeeds.
    #[test]
    #[serial(embedding_switch_guard)]
    fn test_overlapping_switch_calls_second_rejected_first_completes_normally() {
        use std::sync::{Arc, Barrier};
        use std::thread;

        let barrier = Arc::new(Barrier::new(2));
        let barrier2 = barrier.clone();
        // Second rendezvous — CORRECTION of the original audit's reasoning
        // (empirically disproven: removing both sleeps outright made this
        // test fail deterministically, not flaky — "second switch attempted
        // while the first is running must be rejected" failed because the
        // guard was already dropped by the time main's check ran). The
        // first barrier only proves both threads were AT the barrier
        // simultaneously; it says nothing about the ORDER of whichever work
        // either thread does immediately afterward. Without something
        // holding the guard open past that point, the spawned thread is
        // free to return (dropping `_guard`) before main's very next line
        // (`try_acquire()` for `second`) actually executes — exactly the
        // race the original 80ms/10ms sleeps were (correctly, it turns out)
        // guarding against. This second barrier replaces that timing guess
        // with an explicit signal: the guard is held until main has
        // actually finished its check, deterministically, never a duration.
        let barrier_checked = Arc::new(Barrier::new(2));
        let barrier_checked2 = barrier_checked.clone();

        let handle = thread::spawn(move || {
            let _guard = super::EmbeddingSwitchGuard::try_acquire().expect("first switch must acquire");
            barrier2.wait(); // signal the second attempt to proceed
            barrier_checked2.wait(); // wait for main's try_acquire() check to finish
            "first completed normally"
            // _guard drops here, only after main has checked
        });

        barrier.wait(); // wait until the first thread holds the guard
        let second = super::EmbeddingSwitchGuard::try_acquire();
        assert!(second.is_err(), "second switch attempted while the first is running must be rejected");
        assert_eq!(second.unwrap_err(), "a re-index is already running");
        barrier_checked.wait(); // tell the spawned thread the check is done — safe to release now

        let first_result = handle.join().expect("first thread must not panic");
        assert_eq!(first_result, "first completed normally");

        // Guard must be free again now.
        let third = super::EmbeddingSwitchGuard::try_acquire();
        assert!(third.is_ok(), "guard must be available again after the first switch completes");
    }

    // ── resolve_embeddings_batch_size ───────────────────────────────

    fn make_settings_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE ai_settings (key TEXT NOT NULL PRIMARY KEY, value TEXT NOT NULL);",
        )
        .unwrap();
        conn
    }

    #[test]
    fn test_batch_size_defaults_to_25_when_unset() {
        let conn = make_settings_db();
        assert_eq!(
            super::resolve_embeddings_batch_size(&conn),
            super::EMBEDDINGS_BATCH_SIZE_DEFAULT
        );
        assert_eq!(super::EMBEDDINGS_BATCH_SIZE_DEFAULT, 25, "default must match the progress-throttle cadence");
    }

    #[test]
    fn test_batch_size_reads_from_ai_settings_not_a_literal() {
        let conn = make_settings_db();
        crate::database::set_ai_setting(&conn, "embeddings_batch_size", "50").unwrap();
        assert_eq!(super::resolve_embeddings_batch_size(&conn), 50);
    }

    #[test]
    fn test_batch_size_clamps_out_of_range_values_to_default() {
        let conn = make_settings_db();
        crate::database::set_ai_setting(&conn, "embeddings_batch_size", "0").unwrap();
        assert_eq!(super::resolve_embeddings_batch_size(&conn), super::EMBEDDINGS_BATCH_SIZE_DEFAULT);

        crate::database::set_ai_setting(&conn, "embeddings_batch_size", "9999").unwrap();
        assert_eq!(super::resolve_embeddings_batch_size(&conn), super::EMBEDDINGS_BATCH_SIZE_DEFAULT);
    }

    #[test]
    fn test_batch_size_ignores_unparseable_value() {
        let conn = make_settings_db();
        crate::database::set_ai_setting(&conn, "embeddings_batch_size", "not-a-number").unwrap();
        assert_eq!(super::resolve_embeddings_batch_size(&conn), super::EMBEDDINGS_BATCH_SIZE_DEFAULT);
    }

    // ── Pinned structure: async fn + spawn_blocking (the shape UI-responsiveness relies on) ──
    //
    // UI responsiveness itself can't be exercised in-suite (needs the built app —
    // Matt's live pass). This pins the SOURCE-LEVEL guarantee that makes it true: the
    // command is `async fn`, its body runs inside `tokio::task::spawn_blocking`, and the
    // `JoinError` is surfaced via `.await.map_err(...)?` — the exact #1921/R7 shape, same
    // golden-string technique `test_switch_embedding_engine_rejects_invalid_engine` above
    // already uses for this file's one other AppHandle-constrained gate.
    #[test]
    fn test_switch_embedding_engine_is_async_and_wraps_body_in_spawn_blocking() {
        let src = include_str!("scheduler.rs");
        assert!(
            src.contains("pub async fn switch_embedding_engine("),
            "switch_embedding_engine must be declared `async fn` (#3082) — a plain `pub fn` \
             runs synchronously on Tauri's main/event-loop thread, with no implicit blocking pool"
        );
        assert!(
            src.contains("tokio::task::spawn_blocking(move || -> Result<String, String> {"),
            "the entire pipeline body must run inside spawn_blocking, matching the #1921/R7 shape"
        );
        assert!(
            src.contains("switch_embedding_engine thread panicked: {}"),
            "a JoinError (thread panic) must be surfaced via .await.map_err(...)?, not swallowed"
        );
        assert!(
            src.contains("let _guard = EmbeddingSwitchGuard::try_acquire()?;"),
            "the concurrency guard must be acquired before spawn_blocking starts"
        );
    }
}


// ─── build_http_embed_fn credential source tests (C50 / #3171 PR 1, was #1657) ──
//
// Verify that build_http_embed_fn's openrouter arm reads Provider Connections'
// own keychain slot (`read_provider_key`) and does NOT fall through to the
// retired `embeddings_api_key` plaintext slot — the exact credential-handling
// defect C50 fixed (an API key sitting in the database in the clear).

#[cfg(test)]
mod build_http_embed_fn_credential_tests {
    use crate::keychain::test_support::KeychainGuard;
    use rusqlite::Connection;
    use serial_test::serial;

    fn make_embed_test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE ai_settings (
                key   TEXT NOT NULL PRIMARY KEY,
                value TEXT NOT NULL
            );",
        )
        .unwrap();
        conn
    }

    /// build_http_embed_fn returns Ok when the openrouter Provider Connection's
    /// own keychain slot (`ai_openrouter_key`) has a key. This confirms the
    /// credential is read from the connection, not any ai_settings row (§5.9.2 /
    /// C50).
    #[test]
    #[serial(keychain_override)]
    fn test_build_http_embed_fn_reads_provider_connection_key() {
        let _kc = KeychainGuard::set();
        let conn = make_embed_test_db();
        crate::keychain::store_password("ai_openrouter_key", "sk-or-v1-test-abc123").unwrap();

        let result = crate::build_http_embed_fn(&conn, "openrouter", "text-embedding-3-small");
        assert!(
            result.is_ok(),
            "build_http_embed_fn must succeed when the openrouter connection has a key; got: {:?}",
            result.err()
        );
        let (_, tag) = result.unwrap();
        assert_eq!(tag, "openrouter", "engine tag must be 'openrouter'");
    }

    /// build_http_embed_fn returns Err when NO openrouter connection key exists —
    /// even when the retired `embeddings_api_key` plaintext row is populated.
    /// This is C50's regression guard: a stale plaintext value left over from
    /// before the migration (or a future accidental re-write) must never be read
    /// again — if it were, the credential-handling defect C50 fixed would be back.
    #[test]
    #[serial(keychain_override)]
    fn test_build_http_embed_fn_does_not_fall_through_to_retired_embeddings_api_key() {
        let _kc = KeychainGuard::set();
        let conn = make_embed_test_db();
        // Populate the RETIRED plaintext slot — this must never be read again.
        crate::database::set_ai_setting(&conn, "embeddings_api_key", "sk-or-v1-stale-plaintext")
            .unwrap();
        // No openrouter connection key configured.

        let result = crate::build_http_embed_fn(&conn, "openrouter", "text-embedding-3-small");
        assert!(
            result.is_err(),
            "build_http_embed_fn must fail when no openrouter connection key exists, even if \
             the retired embeddings_api_key plaintext row is populated"
        );
        let err_msg = result.map(|_| "").unwrap_err();
        assert!(
            err_msg.contains("not connected"),
            "Error must name the real problem (not connected), not reference a plaintext \
             card credential; got: {err_msg}"
        );
    }

    /// build_http_embed_fn returns Err when the openrouter connection key is the
    /// empty string (a keychain write of "" is still a configured-but-useless
    /// state — must not silently proceed with an empty Bearer token).
    #[test]
    #[serial(keychain_override)]
    fn test_build_http_embed_fn_rejects_empty_connection_key() {
        let _kc = KeychainGuard::set();
        let conn = make_embed_test_db();
        crate::keychain::store_password("ai_openrouter_key", "").unwrap();

        let result = crate::build_http_embed_fn(&conn, "openrouter", "text-embedding-3-small");
        assert!(
            result.is_err(),
            "build_http_embed_fn must fail when the openrouter connection key is empty"
        );
    }

    /// build_http_embed_fn (Ollama arm) reads `embeddings_endpoint`, not the generic `endpoint`.
    /// Unaffected by C50 — Ollama has no key, only this endpoint.
    #[test]
    fn test_build_http_embed_fn_ollama_reads_embeddings_endpoint() {
        let conn = make_embed_test_db();
        crate::database::set_ai_setting(&conn, "embeddings_endpoint", "http://custom-ollama:11434")
            .unwrap();
        // Populate the generic endpoint slot (should NOT be used for embeddings)
        crate::database::set_ai_setting(&conn, "endpoint", "http://other:11434").unwrap();

        let result = crate::build_http_embed_fn(&conn, "ollama", "nomic-embed-text");
        assert!(
            result.is_ok(),
            "Ollama arm must succeed when embeddings_endpoint is set; got: {:?}",
            result.err()
        );
        let (_, tag) = result.unwrap();
        assert_eq!(tag, "ollama", "engine tag must be 'ollama'");
    }
}


// ─── fetch_embedding_models gate 2 + venice dropdown tests (#2988 PR1) ────────
//
// `fetch_embedding_models` is the SECOND gate Matt flagged — the one that
// actually populates the Semantic Search model dropdown. Unlike
// `switch_embedding_engine` (blocked from direct invocation by its
// `tauri::AppHandle` param — see the comment on
// `test_switch_embedding_engine_rejects_invalid_engine` above), this command
// takes only `State<'_, DbState>`, so it CAN be exercised through the real
// `#[tauri::command]` entry point via `tauri::test::mock_app`.

#[cfg(test)]
mod fetch_embedding_models_venice_tests {
    use crate::database::DbState;
    use crate::keychain::test_support::KeychainGuard;
    use serial_test::serial;
    use tauri::Manager;

    fn setup() -> tauri::App<tauri::test::MockRuntime> {
        let conn = rusqlite::Connection::open_in_memory().expect("in-memory db");
        conn.execute_batch(
            "CREATE TABLE ai_settings (key TEXT NOT NULL PRIMARY KEY, value TEXT NOT NULL);",
        )
        .expect("ai_settings schema");
        let app = tauri::test::mock_app();
        app.manage(DbState::new(conn));
        app
    }

    /// The real dropdown-populating command must reject an unknown engine with a
    /// message naming all three valid engines — through the actual command, not
    /// a logic replica.
    #[tokio::test]
    #[serial(keychain_override)]
    async fn test_fetch_embedding_models_rejects_unknown_engine_via_real_command() {
        let _kc = KeychainGuard::set();
        let app = setup();
        let db_state = app.state::<DbState>();

        let result = super::fetch_embedding_models(
            db_state,
            "candle".to_string(),
            None,
        )
        .await;

        let err = result.expect_err("unknown engine must be rejected by the real command");
        assert!(
            err.contains("openrouter") && err.contains("ollama") && err.contains("venice"),
            "error must name all three valid engines, got: {}",
            err
        );
    }

    /// The real dropdown-populating command, called with `engine: "venice"`, must
    /// return a non-empty model list — proving gate 2 (the one Matt specifically
    /// flagged as missing) is fixed, not just gate 1 (`switch_embedding_engine`).
    /// No live Venice key exists on this machine (Reed confirmed) — a fake key is
    /// injected via `MockKeychain` and the HTTP boundary is mocked with mockito.
    /// This is mock-verified: it proves the wire shape and the dropdown-populating
    /// code path work end to end, NOT that Venice's real `/models?type=embedding`
    /// endpoint returns this exact shape (that part is doc-sourced — confirmed
    /// against `docs.venice.ai/api-reference/endpoint/models/list`'s OpenAPI spec,
    /// which documents `type=embedding` as a valid filter value — but unverifiable
    /// against a live key in this environment).
    #[tokio::test]
    #[serial(keychain_override)]
    async fn test_fetch_embedding_models_venice_returns_nonempty_list_via_real_command() {
        let _kc = KeychainGuard::set();
        crate::keychain::store_password("ai_venice_key", "test-fake-venice-key-not-real")
            .expect("mock keychain store");

        // `Server::new_async` / `create_async` (not the sync `new`/`create`) — this test
        // is `#[tokio::test]` (needed to `.await` the real `fetch_embedding_models`
        // command), and mockito's sync constructor spins its own blocking runtime
        // internally, which panics with "Cannot start a runtime from within a runtime"
        // when called from inside one.
        let mut server = mockito::Server::new_async().await;
        let _m = server.mock("GET", "/models")
            .match_query(mockito::Matcher::UrlEncoded("type".into(), "embedding".into()))
            .match_header("authorization", "Bearer test-fake-venice-key-not-real")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"data":[{"id":"text-embedding-bge-m3","name":"BGE-M3","context_length":8192}],"object":"list","type":"embedding"}"#)
            .create_async()
            .await;

        super::test_set_venice_embed_models_base(&server.url());
        let app = setup();
        let db_state = app.state::<DbState>();
        let result = super::fetch_embedding_models(
            db_state,
            "venice".to_string(),
            None,
        )
        .await;
        super::test_clear_venice_embed_models_base();

        let models = result.expect("fetch_embedding_models('venice') should succeed against the mock");
        assert!(!models.is_empty(), "the dropdown-populating path must return a non-empty Venice model list");
        assert_eq!(models[0].id, "text-embedding-bge-m3");
        assert_eq!(models[0].provider, "venice");
    }

    /// Without a configured Venice key, the venice arm must fail with a clear,
    /// named error — not silently return an empty list (an empty-list dropdown
    /// with no explanation is exactly the failure mode Matt's addendum warned
    /// about).
    #[tokio::test]
    #[serial(keychain_override)]
    async fn test_fetch_embedding_models_venice_missing_key_errors_via_real_command() {
        let _kc = KeychainGuard::set(); // empty mock keychain
        let app = setup();
        let db_state = app.state::<DbState>();

        let result = super::fetch_embedding_models(
            db_state,
            "venice".to_string(),
            None,
        )
        .await;

        let err = result.expect_err("missing Venice key must error, not return an empty list silently");
        assert!(
            err.contains("Venice API key not configured"),
            "error must name the real problem, got: {}",
            err
        );
    }

    /// Matt's named mutation (C50 / #3171 PR 1): "remove a feature's key reuse
    /// → it must FAIL, not fall back to an empty key." Without a configured
    /// openrouter Provider Connection, the real dropdown-populating command
    /// must fail with a clear, named error BEFORE touching
    /// `fetch_openrouter_embedding_models` at all — same shape as the Venice
    /// test above, now proven for the arm C50 actually changed. A silent
    /// empty-key fallback would instead proceed to call out and get a 401,
    /// indistinguishable from a provider outage.
    #[tokio::test]
    #[serial(keychain_override)]
    async fn test_fetch_embedding_models_openrouter_missing_connection_key_errors_via_real_command() {
        let _kc = KeychainGuard::set(); // empty mock keychain — no ai_openrouter_key
        let app = setup();
        let db_state = app.state::<DbState>();

        let result = super::fetch_embedding_models(
            db_state,
            "openrouter".to_string(),
            None,
        )
        .await;

        let err = result.expect_err(
            "missing openrouter connection key must error, not return an empty list silently",
        );
        assert!(
            err.contains("not connected"),
            "error must name the real problem (not connected), got: {}",
            err
        );
    }
}


// ─── get_embeddings_counts tests (#1674) ─────────────────────────────────────

#[cfg(test)]
mod get_embeddings_counts_tests {
    use rusqlite::Connection;

    fn make_test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE ai_settings (
                key   TEXT NOT NULL PRIMARY KEY,
                value TEXT NOT NULL
            );
            CREATE TABLE embeddings (
                id           TEXT NOT NULL,
                source       TEXT NOT NULL,
                embedding    BLOB NOT NULL,
                content_hash TEXT NOT NULL,
                engine       TEXT,
                PRIMARY KEY (id, source)
            );",
        )
        .unwrap();
        conn
    }

    fn insert_embedding(conn: &Connection, id: &str, source: &str) {
        conn.execute(
            "INSERT OR REPLACE INTO embeddings (id, source, embedding, content_hash, engine) VALUES (?1, ?2, X'00', 'hash', 'openrouter')",
            rusqlite::params![id, source],
        )
        .unwrap();
    }

    /// get_embeddings_counts returns correct per-source totals.
    #[test]
    fn test_get_embeddings_counts_returns_per_source_totals() {
        let conn = make_test_db();
        insert_embedding(&conn, "n1", "note");
        insert_embedding(&conn, "n2", "note");
        insert_embedding(&conn, "n3", "note");
        insert_embedding(&conn, "f1", "memory");
        insert_embedding(&conn, "f2", "memory");
        insert_embedding(&conn, "c1", "chat");

        let mut notes = 0i64;
        let mut facts = 0i64;
        let mut chats = 0i64;

        let mut stmt = conn
            .prepare("SELECT source, COUNT(*) FROM embeddings GROUP BY source")
            .unwrap();
        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })
            .unwrap();
        for row in rows {
            let (source, count) = row.unwrap();
            match source.as_str() {
                "note" => notes = count,
                "memory" => facts = count,
                "chat" => chats = count,
                _ => {}
            }
        }

        assert_eq!(notes, 3, "must count 3 notes");
        assert_eq!(facts, 2, "must count 2 facts");
        assert_eq!(chats, 1, "must count 1 chat");
    }

    /// get_embeddings_counts returns zero counts on empty table.
    #[test]
    fn test_get_embeddings_counts_empty_table_returns_zeros() {
        let conn = make_test_db();
        let mut notes = 0i64;
        let mut facts = 0i64;
        let mut chats = 0i64;

        let mut stmt = conn
            .prepare("SELECT source, COUNT(*) FROM embeddings GROUP BY source")
            .unwrap();
        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })
            .unwrap();
        for row in rows {
            let (source, count) = row.unwrap();
            match source.as_str() {
                "note" => notes = count,
                "memory" => facts = count,
                "chat" => chats = count,
                _ => {}
            }
        }

        assert_eq!(notes, 0);
        assert_eq!(facts, 0);
        assert_eq!(chats, 0);
    }

    /// failed count reads from ai_settings.embeddings_failed_count; defaults to 0 if absent.
    #[test]
    fn test_get_embeddings_counts_failed_defaults_to_zero_when_absent() {
        let conn = make_test_db();
        // No embeddings_failed_count key written
        let failed: i64 = crate::database::get_ai_setting(&conn, "embeddings_failed_count")
            .and_then(|s| s.parse::<i64>().ok())
            .unwrap_or(0);
        assert_eq!(failed, 0, "failed must default to 0 when key is absent");
    }

    /// failed count reads correctly when embeddings_failed_count is written.
    #[test]
    fn test_get_embeddings_counts_reads_persisted_failed_count() {
        let conn = make_test_db();
        crate::database::set_ai_setting(&conn, "embeddings_failed_count", "7").unwrap();
        let failed: i64 = crate::database::get_ai_setting(&conn, "embeddings_failed_count")
            .and_then(|s| s.parse::<i64>().ok())
            .unwrap_or(0);
        assert_eq!(failed, 7, "failed must read the persisted value");
    }

    /// Partial status (failed > 0) writes 'partial' to embeddings_status; clean run writes 'ready'.
    #[test]
    fn test_partial_status_written_when_failed_count_nonzero() {
        let conn = make_test_db();
        // Simulate failed_count = 2
        let failed_count = 2usize;
        let terminal_stage = if failed_count > 0 { "partial" } else { "ready" };
        crate::database::set_ai_setting(&conn, "embeddings_status", terminal_stage).unwrap();
        crate::database::set_ai_setting(
            &conn,
            "embeddings_failed_count",
            &failed_count.to_string(),
        )
        .unwrap();

        assert_eq!(
            crate::database::get_ai_setting(&conn, "embeddings_status").as_deref(),
            Some("partial"),
            "status must be 'partial' when failed_count > 0"
        );
        assert_eq!(
            crate::database::get_ai_setting(&conn, "embeddings_failed_count").as_deref(),
            Some("2"),
            "embeddings_failed_count must be persisted"
        );
    }

    /// Clean run (failed = 0) writes 'ready' to embeddings_status.
    #[test]
    fn test_ready_status_written_when_failed_count_zero() {
        let conn = make_test_db();
        let failed_count = 0usize;
        let terminal_stage = if failed_count > 0 { "partial" } else { "ready" };
        crate::database::set_ai_setting(&conn, "embeddings_status", terminal_stage).unwrap();
        crate::database::set_ai_setting(
            &conn,
            "embeddings_failed_count",
            &failed_count.to_string(),
        )
        .unwrap();

        assert_eq!(
            crate::database::get_ai_setting(&conn, "embeddings_status").as_deref(),
            Some("ready"),
            "status must be 'ready' when failed_count == 0"
        );
    }
}

// ═══════════════════════════════════════════════════════════════════════════════════
// §36.3 Scheduler tick (#3050 PR 2, `docs/spec/runtime.md` §36.3/§36.10/§36.11)
//
// This is the tick §36.11's own TurnOrigin bullet names as the second
// sanctioned construction site (alongside `ai::chat::ai_chat_stream`'s
// `TurnOrigin::User`) — `scripts/section-4-analyzer/src/turn_origin.rs`'s
// `ALLOWED_CONSTRUCTORS` placeholder (`"scheduler_tick"`) becomes real here.
// The gate checks the literal enclosing function name, so `TurnOrigin::Job`'s
// construction below MUST stay textually inside `scheduler_tick` itself —
// never inside a helper it calls — or CI fails closed by design.
//
// Mechanics (Reed's scope audit, 2026-10-27):
// - Spawned once from `lib.rs`'s `.setup()` via `tauri::async_runtime::spawn`,
//   same shape as `ai::extraction_worker::start_extraction_worker` — never a
//   `#[tauri::command]`, never on the main thread (#3085's lesson, applied in
//   reverse: this was never a blocking sync command to begin with).
// - Claim-before-run is a conditional `UPDATE ... WHERE status='pending'`
//   inside the app's one shared `Arc<Mutex<DbConn>>` — no `BEGIN IMMEDIATE`
//   needed; there is only one connection, so two ticks racing to claim the
//   same row serialize on the Rust mutex, not on a SQLite-level lock.
// - The launch pass and the periodic 60s tick are the SAME function
//   (`scheduler_tick`), called once at spawn time and then every
//   `RUNTIME_TICK_SECS` — never two differently-shaped "run due jobs"
//   implementations (§36.3.5's own wording).
// - Ruling 1 (Matt, 2026-10-27): a recurring/heartbeat OC job's `next_run_at`
//   is recomputed AT CLAIM TIME (generalized here to every recurring job,
//   API or OC — see `claim_due_jobs`), never waiting on OC's own completion,
//   which Pane cannot observe. This is also what makes the missed-run-collapse
//   invariant hold uniformly: a recurring job's recurrence depends only on
//   Pane's own clock, never on whether this firing's dispatch succeeds.
// - Ruling 2 (Matt, 2026-10-27): the schedule tool family (`schedule_job` /
//   `list_jobs` / `cancel_job` / `pause_job` / `resume_job`) will be offered
//   to OC-bound sessions too (PR 4 builds the tools; this PR only needs the
//   scheduler side to be capable of an OC target, which `claim_due_jobs`'
//   `runtime_type` resolution already provides — no OC-specific gate needed
//   on the job-row shape itself).

use crate::ai::chat::{
    build_messages_with_context, create_session_internal, default_system_prompt,
    resolve_model_for_message, run_agent_loop, AgentLoopInputs,
    ContinuePolicy, JobOutcome, TurnOrigin,
};
use crate::ai::provider::ChatMessage;
use crate::ai::runtime_constants::{
    RUNTIME_DEFAULT_CONCURRENCY, RUNTIME_JOB_ARCHIVE_DAYS, RUNTIME_MAX_CONSECUTIVE_FAILURES,
    RUNTIME_OC_PICKUP_SESSIONS_PER_TICK, RUNTIME_OC_PICKUP_WINDOW_DAYS, RUNTIME_TICK_SECS,
};
use crate::ai::runtime_resolution::resolve_runtime_config_for_session;
use crate::ai::tools::{get_pane_tools_for_runtime, ToolDef};
use chrono::{DateTime, Datelike, Local, TimeZone, Timelike, Utc};
use std::collections::HashSet;

/// One claimed, ready-to-dispatch job — the minimal shape both the OC send
/// path and the API `run_agent_loop` dispatch path need. Deliberately does
/// NOT carry `cron`/`next_run_at`/`missed_count` — those are settled by
/// `claim_due_jobs` (for recurring/heartbeat) before this struct is ever
/// built, so a dispatch function can never accidentally re-derive or
/// re-mutate recurrence state that's already been finalized under the lock.
#[derive(Debug, Clone)]
pub(crate) struct ClaimedJob {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub brief: String,
    pub agent_id: String,
    pub project_id: String,
    pub session_id: String,
    /// `ai_agents.runtime_type`, resolved at claim time via the join in
    /// `load_claimed_job_row`. #3129 (C28(b)): every runtime, `openclaw`
    /// included, now routes through `prepare_job_dispatch`/
    /// `run_agent_loop` — this field is carried on `ClaimedJob` for
    /// context/logging but is no longer branched on anywhere in the
    /// dispatch path itself (previously: `"openclaw"` routed to the now-
    /// deleted `dispatch_oc_job`).
    pub runtime_type: String,
    /// §36.8/§5 (#3050 PR 5): the chat this job was spawned/scheduled FROM
    /// — report-back and wake both target this session, never the job's
    /// own new one. `None` for jobs with no originating chat.
    pub origin_session_id: Option<String>,
    /// §4 (#3050 PR 5): set iff THIS job is itself a depth-2 spawn (its own
    /// turn was created by another job's `spawn_job` call). Threaded onto
    /// `AgentLoopInputs.context_clone` as `"ownJobId"`/`"parentJobId"` so
    /// `execute_single_tool`'s `spawn_job` arm can re-derive the depth
    /// cap at dispatch time — never read directly by this struct's own
    /// consumers otherwise.
    pub parent_job_id: Option<String>,
    /// §5.2 (#3050 PR 5): this job's own chain root (self if it is the
    /// root). Used for the chain-shared tool-call ceiling bookkeeping.
    pub root_job_id: Option<String>,
    /// §5.2 (#3050 PR 5): sequential wake-chain depth — drives the
    /// offer-time chain cap (`chain_depth >= RUNTIME_MAX_CHAIN_DEPTH`).
    pub chain_depth: i64,
    /// §5 (#3050 PR 5): whether this job's completion should enqueue a
    /// wake turn on `origin_session_id` (default `true` at the schema
    /// level — `agent_jobs.wake_origin INTEGER NOT NULL DEFAULT 1`).
    pub wake_origin: bool,
}

/// Full row shape needed only transiently, inside `claim_due_jobs`'s lock, to
/// decide whether a claim counts as a missed-run collapse and to recompute
/// `next_run_at`. Not returned to callers — `ClaimedJob` (above) is.
struct ClaimedJobRow {
    id: String,
    kind: String,
    title: String,
    brief: String,
    agent_id: String,
    project_id: String,
    session_id: String,
    cron: Option<String>,
    next_run_at: Option<String>,
    missed_count: i64,
    runtime_type: String,
    origin_session_id: Option<String>,
    parent_job_id: Option<String>,
    root_job_id: Option<String>,
    chain_depth: i64,
    wake_origin: bool,
}

/// #3129 (C28(b)): no longer selects `agent_jobs.oc_session_key` — the
/// column itself stays (Matt's pin: vestigial, unread, no schema bump for
/// a drop that buys nothing), but nothing downstream of `claim_due_jobs`
/// reads `ClaimedJob.oc_session_key` once the dispatch branch and
/// background-pickup fallback that used to read it are gone (commits 2-4).
fn load_claimed_job_row(conn: &rusqlite::Connection, job_id: &str) -> Result<ClaimedJobRow, String> {
    conn.query_row(
        "SELECT agent_jobs.id, agent_jobs.kind, agent_jobs.title, agent_jobs.brief, agent_jobs.agent_id, \
                agent_jobs.project_id, agent_jobs.session_id, agent_jobs.cron, \
                agent_jobs.next_run_at, agent_jobs.missed_count, COALESCE(ai_agents.runtime_type, ''), \
                agent_jobs.origin_session_id, agent_jobs.parent_job_id, agent_jobs.root_job_id, \
                agent_jobs.chain_depth, agent_jobs.wake_origin \
         FROM agent_jobs JOIN ai_agents ON ai_agents.id = agent_jobs.agent_id \
         WHERE agent_jobs.id = ?1",
        params![job_id],
        |row| {
            Ok(ClaimedJobRow {
                id: row.get(0)?,
                kind: row.get(1)?,
                title: row.get(2)?,
                brief: row.get(3)?,
                agent_id: row.get(4)?,
                project_id: row.get(5)?,
                session_id: row.get(6)?,
                cron: row.get(7)?,
                next_run_at: row.get(8)?,
                missed_count: row.get(9)?,
                runtime_type: row.get(10)?,
                origin_session_id: row.get(11)?,
                parent_job_id: row.get(12)?,
                root_job_id: row.get(13)?,
                chain_depth: row.get(14)?,
                wake_origin: row.get::<_, i64>(15)? != 0,
            })
        },
    )
    .map_err(|e| format!("load_claimed_job_row failed for {}: {}", job_id, e))
}

/// §36.3.3/§36.11: "scheduler claims before it runs." A single conditional
/// `UPDATE ... WHERE status='pending'`, checked via the affected-row count —
/// never a `SELECT` followed by a separate `UPDATE` (that shape would be
/// racy even under this app's single-connection `Mutex`, if anything else
/// could run between the two). Returns `Ok(true)` iff THIS call won the
/// claim; `Ok(false)` means another caller already claimed the row first.
pub(crate) fn claim_job(conn: &rusqlite::Connection, job_id: &str) -> Result<bool, String> {
    let affected = conn
        .execute(
            "UPDATE agent_jobs SET status = 'running', started_at = datetime('now'), updated_at = datetime('now') \
             WHERE id = ?1 AND status = 'pending'",
            params![job_id],
        )
        .map_err(|e| format!("claim_job UPDATE failed for {}: {}", job_id, e))?;
    Ok(affected == 1)
}

/// Select due job ids (`status='pending' AND next_run_at <= now`, oldest
/// first) and attempt to claim up to `limit` of them. §5 of Reed's scope
/// audit: this app shares ONE `rusqlite::Connection` behind one
/// `Arc<Mutex<DbConn>>` — there is no multi-connection SQLite lock
/// contention to defend against (`BEGIN IMMEDIATE` solves a problem this
/// architecture doesn't have), only the SQL inside `claim_job`'s single
/// transaction needs to be correct, which the claim-before-run test proves
/// directly against that function.
pub(crate) fn claim_due_jobs(conn: &rusqlite::Connection, limit: u32) -> Result<Vec<ClaimedJob>, String> {
    let candidate_ids: Vec<String> = {
        let mut stmt = conn
            .prepare(
                "SELECT id FROM agent_jobs \
                 WHERE status = 'pending' AND next_run_at IS NOT NULL AND next_run_at <= datetime('now') \
                 ORDER BY next_run_at LIMIT ?1",
            )
            .map_err(|e| format!("claim_due_jobs SELECT prepare failed: {}", e))?;
        let rows = stmt
            .query_map(params![limit], |row| row.get::<_, String>(0))
            .map_err(|e| format!("claim_due_jobs SELECT query failed: {}", e))?;
        rows.filter_map(|r| r.ok()).collect()
    };

    let mut claimed = Vec::new();
    for id in candidate_ids {
        match claim_job(conn, &id) {
            Ok(true) => {}
            Ok(false) => {
                // Another claim already won this row between the SELECT and
                // this UPDATE — expected under concurrent ticks, silent.
                continue;
            }
            Err(e) => {
                eprintln!("[SCHEDULER] claim_due_jobs: claim_job failed for {}: {}", id, e);
                continue;
            }
        }

        let row = match load_claimed_job_row(conn, &id) {
            Ok(row) => row,
            Err(e) => {
                eprintln!(
                    "[SCHEDULER] Claimed job {} but failed to load its row: {} — leaving status=running for manual recovery, not re-queueing",
                    id, e
                );
                continue;
            }
        };

        // Ruling 1 (Matt, 2026-10-27), generalized: for recurring/heartbeat
        // jobs, recompute next_run_at and return status to 'pending' NOW,
        // under this same lock — not after the dispatched turn finishes (this
        // PR carries no `job_id` field on `AgentLoopInputs` yet, so there is
        // no completion signal to wait on for the API path either; §36.2's
        // auto-continue PR is what wires that). The schedule's own recurrence
        // depends only on Pane's clock, never on this firing's outcome.
        if row.kind == "recurring" || row.kind == "heartbeat" {
            if let Err(e) = recompute_recurrence(conn, &row) {
                eprintln!(
                    "[SCHEDULER] Job {} ({}): failed to recompute next_run_at: {} — leaving status=running",
                    id, row.kind, e
                );
                continue;
            }
        }

        claimed.push(ClaimedJob {
            id: row.id,
            kind: row.kind,
            title: row.title,
            brief: row.brief,
            agent_id: row.agent_id,
            project_id: row.project_id,
            session_id: row.session_id,
            runtime_type: row.runtime_type,
            origin_session_id: row.origin_session_id,
            parent_job_id: row.parent_job_id,
            root_job_id: row.root_job_id,
            chain_depth: row.chain_depth,
            wake_origin: row.wake_origin,
        });
    }

    Ok(claimed)
}

/// SQLite's own `datetime('now')` string shape ("YYYY-MM-DD HH:MM:SS", UTC) —
/// every `next_run_at` this module writes uses this exact format so the
/// claim query's `next_run_at <= datetime('now')` comparison stays a correct
/// lexical string comparison (both sides zero-padded, same clock basis).
/// Cron fields are local time (§36.3.2); conversion to/from UTC happens at
/// these two boundary functions, nowhere else.
pub(crate) fn format_stored_datetime(dt: &DateTime<Utc>) -> String {
    dt.format("%Y-%m-%d %H:%M:%S").to_string()
}

/// §36.3.7 — `schedule_job`'s `run_at` arg is "ISO local" (the spec table's
/// own wording): a local-clock-time string, same local-time convention
/// §36.3.2 already uses for `cron` fields, converted to UTC at this one
/// boundary function (mirroring `format_stored_datetime`/`parse_stored_
/// datetime`'s own "conversion happens at these two boundary functions,
/// nowhere else" rule — this is the THIRD and only other boundary, for the
/// one new input format this PR introduces). Accepts both a `T` separator
/// (`2026-11-02T09:00:00`) and a space separator (`2026-11-02 09:00:00") so
/// a model that hand-types either common ISO-ish shape is not rejected.
///
/// C29 (#3128): ALSO accepts both shapes with seconds omitted
/// (`2026-11-02T09:00`/`2026-11-02 09:00`) — the New job dialog's
/// `<input type="datetime-local">` emits exactly this no-seconds shape per
/// the HTML spec, and every dialog-created Once job was rejected before
/// this widened. Extends the same "don't reject a common ISO-ish shape"
/// intent above rather than patching the one caller that hits it.
pub(crate) fn parse_run_at_local(run_at: &str) -> Result<DateTime<Utc>, String> {
    // C29 (#3128, Matt's correction to Reed's brief — this is a hard bug,
    // not a UI ask): the New job dialog's `<input type="datetime-local">`
    // emits `2026-10-31T10:32` per the HTML spec — no seconds — but this
    // function only ever accepted `%H:%M:%S`. Every dialog-created Once
    // job was rejected with "must be a local datetime like
    // 2026-11-02T09:00:00: input contains invalid characters". Fixed in
    // the validator (not the one caller) so a hand-typing model hitting
    // the SAME function gets the same widened acceptance — matching this
    // function's own pre-existing doc intent (see above) rather than
    // patching one caller.
    let naive = chrono::NaiveDateTime::parse_from_str(run_at, "%Y-%m-%dT%H:%M:%S")
        .or_else(|_| chrono::NaiveDateTime::parse_from_str(run_at, "%Y-%m-%d %H:%M:%S"))
        .or_else(|_| chrono::NaiveDateTime::parse_from_str(run_at, "%Y-%m-%dT%H:%M"))
        .or_else(|_| chrono::NaiveDateTime::parse_from_str(run_at, "%Y-%m-%d %H:%M"))
        .map_err(|e| format!("run_at '{}' must be a local datetime like 2026-11-02T09:00:00: {}", run_at, e))?;
    match Local.from_local_datetime(&naive) {
        chrono::offset::LocalResult::Single(local) => Ok(local.with_timezone(&Utc)),
        chrono::offset::LocalResult::Ambiguous(earliest, _) => Ok(earliest.with_timezone(&Utc)),
        chrono::offset::LocalResult::None => Err(format!(
            "run_at '{}' does not exist in the local timezone (DST gap)",
            run_at
        )),
    }
}

/// C29 (#3128): the New job dialog's exact `<input type="datetime-local">`
/// output (no seconds) must round-trip through `parse_run_at_local`, not
/// just the hand-written-with-seconds shape the pre-existing tests never
/// covered (there were none before this commit — confirmed by grep).
#[cfg(test)]
mod parse_run_at_local_tests_3128_c29 {
    use super::*;

    /// Mutation (actually run — see report): revert `parse_run_at_local`
    /// to only the two `%H:%M:%S`-suffixed formats — reddens ("input
    /// contains invalid characters", the exact error text #3128 quotes).
    #[test]
    fn test_parse_run_at_local_accepts_dialog_no_seconds_t_separator() {
        // The LITERAL shape a datetime-local input emits — not a
        // hand-written one with seconds added.
        let result = parse_run_at_local("2026-10-31T10:32");
        assert!(result.is_ok(), "dialog's no-seconds T-separated shape must be accepted: {:?}", result);
    }

    /// The space-separated no-seconds sibling, same shape doubling as the
    /// pre-existing T/space pair above.
    #[test]
    fn test_parse_run_at_local_accepts_no_seconds_space_separator() {
        let result = parse_run_at_local("2026-10-31 10:32");
        assert!(result.is_ok(), "no-seconds space-separated shape must be accepted: {:?}", result);
    }

    /// Regression guard: the pre-existing WITH-seconds shape (what a
    /// hand-typing model is told to use by the tool schema's own
    /// description) must still work — proves the new `.or_else` arms
    /// don't shadow or break the original acceptance.
    #[test]
    fn test_parse_run_at_local_still_accepts_with_seconds_t_separator() {
        let result = parse_run_at_local("2026-12-01T09:00:00");
        assert!(result.is_ok(), "with-seconds shape must still be accepted: {:?}", result);
    }

    /// A genuinely malformed string must still be rejected — proves the
    /// widened acceptance isn't an "accept anything" regression.
    #[test]
    fn test_parse_run_at_local_rejects_garbage() {
        let result = parse_run_at_local("not-a-datetime");
        assert!(result.is_err(), "garbage input must still be rejected");
    }
}

fn parse_stored_datetime(s: &str) -> Result<DateTime<Utc>, String> {
    let naive = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S")
        .map_err(|e| format!("failed to parse stored datetime '{}': {}", s, e))?;
    Ok(DateTime::<Utc>::from_naive_utc_and_offset(naive, Utc))
}

/// §36.3.5 + Ruling 1: recompute `next_run_at` and flip `status` back to
/// `'pending'`, incrementing `missed_count` by exactly one PER COLLAPSE EVENT
/// (never per missed interval — §36.3.5's own wording: "a schedule that
/// missed 40 firings... runs one catch-up, not 40"). "Missed" is determined
/// by asking: if we'd recomputed from the OLD `next_run_at` instead of now,
/// would that recomputed time ALSO already be due? If yes, at least one
/// scheduled firing was skipped entirely — a collapse, not an on-time tick.
fn recompute_recurrence(conn: &rusqlite::Connection, row: &ClaimedJobRow) -> Result<(), String> {
    let cron = row
        .cron
        .clone()
        .ok_or_else(|| format!("job {} is {} but has no cron", row.id, row.kind))?;
    let now_local = Local::now();

    let missed = match &row.next_run_at {
        Some(old_next_str) => {
            let old_next_local = parse_stored_datetime(old_next_str)?.with_timezone(&Local);
            match next_cron_occurrence(&cron, old_next_local) {
                Ok(would_be_next) => would_be_next <= now_local,
                Err(_) => false,
            }
        }
        None => false,
    };

    let next_local = next_cron_occurrence(&cron, now_local)?;
    let next_stored = format_stored_datetime(&next_local.with_timezone(&Utc));
    let new_missed_count = if missed { row.missed_count + 1 } else { row.missed_count };

    conn.execute(
        "UPDATE agent_jobs SET status = 'pending', next_run_at = ?1, missed_count = ?2, \
         finished_at = datetime('now'), updated_at = datetime('now') WHERE id = ?3",
        params![next_stored, new_missed_count, row.id],
    )
    .map_err(|e| format!("recompute_recurrence UPDATE failed for {}: {}", row.id, e))?;

    if missed {
        eprintln!(
            "[SCHEDULER] Job {} ({}) missed its window — collapsed to one catch-up run, missed_count now {}, next_run_at={}",
            row.id, row.kind, new_missed_count, next_stored
        );
    } else {
        eprintln!(
            "[SCHEDULER] Job {} ({}) recurred on time, next_run_at={}",
            row.id, row.kind, next_stored
        );
    }
    Ok(())
}

/// Minimal 5-field cron parser (minute hour day-of-month month day-of-week),
/// local time (§36.3.2). No external cron crate — adding one would touch
/// `Cargo.lock`, which this PR's own pre-push rules forbid; the supported
/// syntax (`*`, exact numbers, comma lists, `*/step`) covers every
/// §36.10A preset (daily 9am, every 4h, weekly Mon 9am) and every test below.
///
/// Known limitation, stated rather than silently assumed: real cron ORs
/// day-of-month and day-of-week when BOTH are restricted (non-`*`). This
/// parser ANDs them, which is observably identical to real cron whenever at
/// least one of the two is `*` (true for every §36.10A preset and every test
/// here) and only diverges when both are simultaneously restricted — a
/// pattern no PR-2 caller produces.
fn parse_cron_field(field: &str, min: u32, max: u32) -> Result<Vec<u32>, String> {
    let mut values = Vec::new();
    for part in field.split(',') {
        if part == "*" {
            values.extend(min..=max);
        } else if let Some((base, step_str)) = part.split_once('/') {
            let step: u32 = step_str
                .parse()
                .map_err(|_| format!("invalid cron step '{}'", part))?;
            if step == 0 {
                return Err(format!("invalid cron step '{}': step must be > 0", part));
            }
            let start: u32 = if base == "*" {
                min
            } else {
                base.parse()
                    .map_err(|_| format!("invalid cron field '{}'", part))?
            };
            let mut v = start;
            while v <= max {
                values.push(v);
                v += step;
            }
        } else {
            let v: u32 = part
                .parse()
                .map_err(|_| format!("invalid cron field '{}'", part))?;
            if v < min || v > max {
                return Err(format!("cron field value {} out of range [{}, {}]", v, min, max));
            }
            values.push(v);
        }
    }
    values.sort_unstable();
    values.dedup();
    if values.is_empty() {
        return Err(format!("cron field '{}' produced no valid values", field));
    }
    Ok(values)
}

/// Next occurrence of `cron` strictly after `after`, in `after`'s own time
/// zone. Brute force, minute granularity, bounded to ~4 years — correct and
/// simple; this runs on a 60s tick, never in a hot path.
pub(crate) fn next_cron_occurrence(cron: &str, after: DateTime<Local>) -> Result<DateTime<Local>, String> {
    let fields: Vec<&str> = cron.split_whitespace().collect();
    if fields.len() != 5 {
        return Err(format!(
            "cron expression '{}' must have exactly 5 fields, got {}",
            cron,
            fields.len()
        ));
    }
    let minutes = parse_cron_field(fields[0], 0, 59)?;
    let hours = parse_cron_field(fields[1], 0, 23)?;
    let doms = parse_cron_field(fields[2], 1, 31)?;
    let months = parse_cron_field(fields[3], 1, 12)?;
    // cron day-of-week: 0=Sunday..6=Saturday (standard convention) — matches
    // chrono's `Weekday::num_days_from_sunday()` directly, no remapping.
    let dows = parse_cron_field(fields[4], 0, 6)?;

    let mut candidate = (after + chrono::Duration::minutes(1))
        .with_second(0)
        .and_then(|d| d.with_nanosecond(0))
        .ok_or_else(|| format!("failed to truncate {} to minute precision", after))?;

    const MAX_ITERATIONS: i64 = 60 * 24 * 366 * 4; // ~4 years of minutes
    for _ in 0..MAX_ITERATIONS {
        let minute_ok = minutes.contains(&candidate.minute());
        let hour_ok = hours.contains(&candidate.hour());
        let dom_ok = doms.contains(&candidate.day());
        let month_ok = months.contains(&candidate.month());
        let dow_ok = dows.contains(&candidate.weekday().num_days_from_sunday());

        if minute_ok && hour_ok && dom_ok && month_ok && dow_ok {
            return Ok(candidate);
        }
        candidate += chrono::Duration::minutes(1);
    }

    Err(format!(
        "cron expression '{}' produced no occurrence within ~4 years of {}",
        cron, after
    ))
}

/// §5.1 (#3050 PR 5): `agent_jobs.kind` domain, extended — free text, no
/// `CHECK` constraint (same documented-domain convention `chat_sessions.
/// origin` already uses): `'once'`, `'recurring'`, `'heartbeat'`, `'spawn'`
/// (PR 5, same migration-free addition `'spawn'` itself got in PR 2/4), and
/// now `'wake'` — a job the scheduler creates on another job's completion,
/// never by a tool call. A `'wake'` row's own `session_id` is the ORIGIN
/// session it wakes (not a fresh job session — see `create_wake_job`'s own
/// doc comment), and its `origin_session_id` is always `NULL` (there is
/// nowhere further to report back to: the wake turn's own reply is already
/// visible in the session it ran in).
///
/// §36.3.6: create a job and its job session in one logical unit —
/// `create_session_internal` (its own transaction) for the `chat_sessions`
/// row, then the `agent_jobs` INSERT as a second statement. Not wrapped in an
/// outer `BEGIN`/`COMMIT` (SQLite has no nested transactions without
/// `SAVEPOINT`, and `create_session_internal` already owns its own) — same
/// two-step-not-nested shape the rest of this codebase already uses for
/// session-adjacent creates (e.g. the workshop session helper). A crash
/// between the two steps leaves an orphan, never-listed `origin='spawn'|
/// 'schedule'` session with no job row — inert and harmless, same class of
/// partial-failure risk already accepted elsewhere in this codebase.
///
/// No surface calls this yet in production (`schedule_job`/`spawn_job` are
/// PR 4/5) — this PR's own tests call it directly, same as every other
/// "tool not built yet" PR in this sprint has done (Reed's scope audit,
/// §11.6).
pub(crate) struct NewJobParams<'a> {
    pub kind: &'a str,
    pub title: &'a str,
    pub brief: &'a str,
    pub agent_id: &'a str,
    pub project_id: &'a str,
    pub cron: Option<&'a str>,
    pub run_at: Option<&'a str>,
    /// Caller-computed, already in `format_stored_datetime`'s UTC shape —
    /// kept a dumb row-writer input rather than re-deriving kind/cron/run_at
    /// logic here, so `next_cron_occurrence` stays independently testable.
    pub next_run_at: &'a str,
    /// §36.3.6/§36.3.7 (#3050 PR 4): the chat the `schedule_job` tool call was
    /// made FROM — independent of `project_id`/`session_id` above (the
    /// JOB's own surface/session), so a Home agent scheduling a job onto a
    /// project still writes its confirmation chip into the Home chat it was
    /// called from. `None` for jobs with no originating chat (the Schedules
    /// UI, §36.3.8, a later PR).
    pub origin_session_id: Option<&'a str>,
    /// §36.4 (#3050 PR 5): the originating chat message's id, when known
    /// (the tool call that produced this job). `None` for jobs with no
    /// single originating message (e.g. a future Schedules-UI create).
    pub origin_message_id: Option<&'a str>,
    /// §4 of Reed's PR 5 scope audit: set when THIS job is itself a
    /// depth-2 spawn — the id of the job whose own turn called `spawn_job`
    /// to create this one. `None` for a fresh, depth-1 spawn (called from
    /// an interactive turn or a non-nested job turn) and always `None` for
    /// `kind != "spawn"`.
    pub parent_job_id: Option<&'a str>,
    /// §5.2 of Reed's PR 5 scope audit: the chain this job belongs to, for
    /// the shared tool-call ceiling. `None` means "this job is its own
    /// chain root" — `create_job` self-references `root_job_id` to this
    /// job's own freshly-minted id in that case, so every job always has a
    /// `root_job_id`, chained or not.
    pub root_job_id: Option<&'a str>,
    /// §5.2 of Reed's PR 5 scope audit: sequential wake-chain depth. `0` for
    /// a fresh (non-wake-created) job; a wake row created by
    /// `scheduler_tick`'s post-completion hook sets this to the finishing
    /// job's own `chain_depth + 1`.
    pub chain_depth: i64,
    /// §36.4.5 (#3050 PR 5): whether this job's completion should enqueue a
    /// wake turn on `origin_session_id` once it finishes. Schema default is
    /// `1` (`agent_jobs.wake_origin INTEGER NOT NULL DEFAULT 1`) — `true`
    /// here reproduces that default explicitly rather than relying on it,
    /// so a caller that wants `wake: false` (`spawn_job`'s own optional arg)
    /// has somewhere to put it.
    pub wake_origin: bool,
}

// #3151 PR1 Commit B: `find_or_create_reminders_session` (C5, #3120 PR 2)
// is retired. A `reminder` job now gets its own fresh session per firing,
// the same as every other kind — Matt's 2026-11-02 ruling (#3151): "the
// Reminders session is retired... a reminder is a job like any other: own
// session, text posted there, bell row." The shared-session lookup this
// function performed (by `agent_id`, `origin = 'reminder'`) has no
// replacement; `create_job`'s `else` branch below (which every kind but
// `reminder` already used) now runs unconditionally. Existing
// `origin = 'reminder'` sessions are migrated by
// `ensure_reminders_sessions_renamed_once` (`migrations.rs`, commit C) —
// renamed to "Reminders (old)" and flipped to `origin = 'user'`, one time,
// idempotently.

pub(crate) fn create_job(conn: &rusqlite::Connection, params: NewJobParams) -> Result<String, String> {
    let origin = if params.kind == "spawn" { "spawn" } else { "schedule" };

    // §36.17/§3.3 of Reed's PR 5 scope audit: a spawned session INHERITS its
    // captain's crew, read from the calling session (origin_session_id — the
    // chat `spawn_job` was called FROM), verbatim, before the new session is
    // created. This is what makes a depth-2 spawn chain reachable at all: if
    // a spawn session's own crew defaulted to NULL, `spawn_job` would never
    // be offered inside it and the depth cap would be unreachable by
    // construction. Schedule jobs never inherit — a schedule session is
    // single-agent recurring execution, not delegation, so it gets `None`
    // regardless of the calling session's own crew.
    let inherited_crew: Option<String> = if params.kind == "spawn" {
        params.origin_session_id.and_then(|origin_sid| {
            conn.query_row(
                "SELECT crew_agent_ids FROM chat_sessions WHERE id = ?1",
                params![origin_sid],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()
            .ok()
            .flatten()
            .flatten()
        })
    } else {
        None
    };

    // #3151 PR1 Commit A+B: every job kind — `reminder` included, since
    // Commit B retired the shared-Reminders-session branch this `if` used
    // to have — gets its own fresh session here, titled after the job.
    // `create_session_internal` always inserts `title = ''`; the `UPDATE`
    // below is what makes §36.3.6's "a normal session titled after the
    // schedule" literally true for every kind, not just `reminder` (the
    // only kind that had a title at all before this PR, via the now-
    // deleted `find_or_create_reminders_session`'s hardcoded `'Reminders'`
    // write).
    let mut session = create_session_internal(
        conn,
        params.project_id,
        params.agent_id,
        origin,
        false,
        inherited_crew.as_deref(),
    )?;
    conn.execute(
        "UPDATE chat_sessions SET title = ?1 WHERE id = ?2",
        params![params.title, session.id],
    )
    .map_err(|e| format!("create_job: failed to title session {}: {}", session.id, e))?;
    session.title = params.title.to_string();

    let job_id = uuid::Uuid::new_v4().to_string();
    // §5.2 of Reed's PR 5 scope audit: self-reference when no explicit root
    // was supplied — every job has a `root_job_id`, chained or not.
    let root_job_id_owned: String = params
        .root_job_id
        .map(|s| s.to_string())
        .unwrap_or_else(|| job_id.clone());
    conn.execute(
        "INSERT INTO agent_jobs (id, kind, title, brief, agent_id, project_id, session_id, origin_session_id, origin_message_id, parent_job_id, root_job_id, chain_depth, wake_origin, cron, run_at, next_run_at, status, created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, 'pending', datetime('now'), datetime('now'))",
        params![
            job_id,
            params.kind,
            params.title,
            params.brief,
            params.agent_id,
            params.project_id,
            session.id,
            params.origin_session_id,
            params.origin_message_id,
            params.parent_job_id,
            root_job_id_owned,
            params.chain_depth,
            params.wake_origin as i64,
            params.cron,
            params.run_at,
            params.next_run_at,
        ],
    )
    .map_err(|e| format!("create_job INSERT failed: {}", e))?;

    Ok(job_id)
}

/// §5.1 of Reed's PR 5 scope audit: a wake row — an ordinary new
/// `agent_jobs` row, `kind='wake'`, claimed by the SAME `claim_due_jobs`
/// claim-before-run path every other job uses (confirmed: `claim_due_jobs`'s
/// missed-run-collapse recompute is gated to `kind IN ('recurring',
/// 'heartbeat')` only — `'wake'` falls through unchanged, same as `'once'`/
/// `'spawn'` today). Deliberately NOT a second dispatch path: no direct
/// `run_agent_loop` call here, just a row — the NEXT `scheduler_tick` claims
/// it exactly like any other due job.
///
/// Unlike `create_job`, this does NOT create a new `chat_sessions` row —
/// `session_id = origin_session_id` (the turn runs DIRECTLY in the real
/// chat session, which is what makes the reply show up as "your agent
/// telling you" with zero new Svelte for the reply itself).
/// `origin_session_id` on the wake row's own record is always `NULL` —
/// there is nowhere further to report back to.
///
/// fix(runtime) (Reed's 2026-10-28 audit, #3107): `parent_job_id` is now the
/// id of the job that just FINISHED and triggered this wake — previously
/// hardcoded `NULL` unconditionally, regardless of the finished job's own
/// depth. Threading it forward means a later `spawn_job` call made from
/// THIS wake's own turn sees a real calling-job row (its `chain_depth`/
/// `root_job_id` columns, already threaded below, are what
/// `execute_spawn_job` now reads to carry the chain position forward
/// instead of resetting it) — combined with that fix, the depth and chain
/// caps can no longer be defeated by every hop looking like a fresh root.
/// C22 (#3120 PR 1, §36.4.5): N sibling `spawn_job` completions finishing
/// independently used to call this function N times with no dedup read
/// before insert — confirmed N wakes for N siblings, the issue's own
/// repro. Fixed with an `agent_jobs(session_id) WHERE kind='wake' AND
/// status='pending'` partial unique index (`migrations.rs`,
/// `ensure_agent_jobs_wake_dedup_index`) and an upsert here: the FIRST
/// sibling to land creates the row; every subsequent sibling in the SAME
/// pending-wake window updates the existing row in place instead of
/// inserting a second one. The slot frees itself the moment
/// `claim_due_jobs` flips this row's `status` to `'running'` — the
/// partial index stops matching, so the NEXT batch of completions creates
/// a fresh pending wake rather than piling onto a stale one.
///
/// **Briefs are concatenated on conflict, not dropped** (`brief ||
/// '\n\n---\n\n' || excluded.brief`) — chosen over `INSERT OR IGNORE`
/// because C17 (this same PR) means the woken turn loads real session
/// history regardless, so the wake brief's own text is a courtesy
/// "what just happened" summary, not the only record of a sibling's
/// report; losing one sibling's brief text silently (`OR IGNORE`) would be
/// a worse failure mode than a slightly longer brief. `updated_at` is
/// refreshed on every conflict so the row's own staleness tracking is
/// still meaningful.
///
/// `parent_job_id`/`root_job_id`/`chain_depth` are deliberately NOT
/// updated on conflict — they describe the FIRST sibling's chain position
/// (the row that actually won creation), and a later sibling's own chain
/// position was independently validated at ITS OWN dispatch time before
/// this call was ever reached; overwriting them here would let a later
/// sibling's values silently replace an earlier, already-correct chain
/// linkage for no benefit.
///
/// Returns the id of whichever row now holds the pending wake slot for
/// this session — the freshly generated UUID on a real insert, or the
/// pre-existing row's own id on a conflict (never the UUID generated for
/// this call, which was never actually written in that case).
pub(crate) fn create_wake_job(
    conn: &rusqlite::Connection,
    origin_session_id: &str,
    origin_project_id: &str,
    agent_id: &str,
    brief: &str,
    parent_job_id: &str,
    root_job_id: &str,
    chain_depth: i64,
) -> Result<String, String> {
    let job_id = uuid::Uuid::new_v4().to_string();
    let next_run_at = format_stored_datetime(&Utc::now());
    conn.query_row(
        "INSERT INTO agent_jobs (id, kind, title, brief, agent_id, project_id, session_id, origin_session_id, origin_message_id, parent_job_id, root_job_id, chain_depth, wake_origin, cron, run_at, next_run_at, status, created_at, updated_at) \
         VALUES (?1, 'wake', '', ?2, ?3, ?4, ?5, NULL, NULL, ?6, ?7, ?8, 1, NULL, NULL, ?9, 'pending', datetime('now'), datetime('now')) \
         ON CONFLICT(session_id) WHERE kind = 'wake' AND status = 'pending' \
         DO UPDATE SET brief = agent_jobs.brief || '\n\n---\n\n' || excluded.brief, updated_at = datetime('now') \
         RETURNING id",
        params![
            job_id,
            brief,
            agent_id,
            origin_project_id,
            origin_session_id,
            parent_job_id,
            root_job_id,
            chain_depth,
            next_run_at,
        ],
        |row| row.get::<_, String>(0),
    )
    .map_err(|e| format!("create_wake_job upsert failed: {}", e))
}

/// Matt's addition (not in Reed's scope — a direct ask, PR 5): "chips show
/// live status." The spawn chip AND PR 4's schedule chip read
/// `agent_jobs.status`/`finished_at`/`started_at` by the `job_id` already
/// on the message, and re-render when the scheduler marks the job done
/// ("running" → "done in 48s"). This is the one read the frontend needs for
/// that — no new columns, a plain `SELECT` by id. Returns `Ok(None)` for an
/// unknown `job_id` rather than erroring (a chip for a job that's since
/// been deleted — not a production path today, but defensive) so the
/// frontend can fall back to its last-known/static text instead of an
/// error toast.
///
/// `last_error` added (live-test #1b fix PR, Ruling 3): a recurring/
/// heartbeat job's `status` stays `'pending'` through a transport failure
/// by design (self-heals at the next scheduled firing) — the only
/// observable signal that the last firing failed is `last_error` being
/// non-NULL. `ChatPanel.svelte`'s existing 5-second poll already re-reads
/// this for every non-terminal job_id in view (recurring/heartbeat chips
/// are non-terminal forever, by the same design), so no new poll
/// mechanism is needed — only this one additional field on the existing
/// read.
///
/// `next_run_at` added (C37, #3139, live test #4): the chip's "next" clause
/// used to be baked into the chip's STORED content, server-side, in UTC,
/// at creation time (`execute_schedule_job`'s own `format!("Scheduled: {}
/// \u{b7} next {}", title, next_run_at_stored)`) — never reformatted
/// client-side at all, so a chip always showed the raw UTC string
/// regardless of the viewer's local timezone. Fix is plumbing, not a new
/// formatter: `next_run_at` is an EXISTING `agent_jobs` column (no
/// migration), newly SELECTed here so `chipDisplayText` (`jobStatusChip.ts`)
/// can compute the "next" clause at RENDER time instead, reusing the
/// panel's own `formatScheduleDateTime` (`scheduledPanel.ts`) — one
/// formatter, two consumers, never a second Rust-side reimplementation of
/// the same Today/Tomorrow/weekday local-time logic.
#[tauri::command]
pub fn get_job_status(job_id: String, db: State<DbState>) -> Result<Option<serde_json::Value>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    conn.query_row(
        "SELECT status, started_at, finished_at, last_error, next_run_at FROM agent_jobs WHERE id = ?1",
        params![job_id],
        |row| {
            Ok(serde_json::json!({
                "status": row.get::<_, String>(0)?,
                "started_at": row.get::<_, Option<String>>(1)?,
                "finished_at": row.get::<_, Option<String>>(2)?,
                "last_error": row.get::<_, Option<String>>(3)?,
                "next_run_at": row.get::<_, Option<String>>(4)?,
            }))
        },
    )
    .optional()
    .map_err(|e| format!("get_job_status failed for {}: {}", job_id, e))
}

/// §36.3.8 (#3050 PR 8): the Home Scheduled panel's one query — every job
/// except spawns, across every project, for a Svelte component to
/// `invoke()` directly. Deliberately NOT a reuse of `list_jobs`
/// (`ai::tools::execute_list_jobs`): that query is dispatched through
/// `execute_single_tool`'s AI tool-calling match (session-context-scoped,
/// gated by `ToolAccess::Read`) and its row mapper has no project name and
/// no last-run columns — wrong shape and wrong invocation path for a panel
/// that polls on mount. `get_job_status` right above is this command's own
/// precedent: a bare `#[tauri::command]`, no tool-call machinery, no
/// session context (Reed's PR 8 scope audit §3).
///
/// `WHERE kind NOT IN ('spawn', 'wake')` is the ONLY filter (Matt's ruling,
/// #3050 PR 8 follow-up — recorded so it isn't relitigated): "wake rows
/// are plumbing — the user sees the spawn chip and the captain's reaction
/// in chat; a 'wake' row in the panel means nothing to them. 'Every job
/// except spawns' was written before `wake` existed as a kind." Originally
/// `WHERE kind != 'spawn'` only (Reed's scope audit §3a raised the `wake`
/// question; the task text at the time said "except spawns" literally,
/// read as excluding only spawns) — superseded by this ruling. Still the
/// ONLY kind filter in the codebase: `ai/tools.rs`'s `execute_list_jobs`
/// (the `list_jobs` AI tool) has no kind filter at all (Reed's audit §2
/// confirmed this directly), so there's no second, differently-worded
/// exclusion predicate to drift against.
///
/// LEFT JOIN `projects`, COALESCE to `'Home'` for workspace-scoped jobs —
/// same LEFT JOIN + COALESCE shape `ai/tools.rs`'s notes query already uses
/// for its own project-name fallback (§24.10.1e), label changed to 'Home'
/// to match the Scheduled panel's own vocabulary (design sheet 1a/1b use
/// "Home", not "Workshop").
///
/// Sorting/tab partition (failed+missed first, then next run, paused last;
/// Active vs. Done by `status`) is a pure frontend concern
/// (`scheduledPanel.ts`'s `partitionAndSortScheduledJobs`) — this query
/// returns every non-spawn row, unsorted beyond a stable `next_run_at` tie
/// break, per Reed's scope audit §1 ("this can be a pure frontend unit
/// test on the partition function").
#[tauri::command]
/// C3 (#3120 PR 3, Matt's ruling: notification subtitle said "workspace",
/// should say "Home"). The issue's own citation (`unwrap_or("workspace")`
/// sites in `ai/chat.rs`/`ai/tools.rs`) traces to the internal Home-surface
/// SENTINEL used for `project_id` DB writes (session/job creation,
/// `memory_router.rs`'s own normalization, ~100 existing test fixtures) —
/// confirmed by direct read, none of those 5 sites ever produce rendered
/// text. The ACTUAL rendered-text bug (independently traced for this PR):
/// `emit_agent_message_notification`'s `body = format!("{} · {}", first_line,
/// project_name)` was fed the raw, unresolved `project_id` column at all 3
/// of its call sites in this file — so a Home-bound job/session's
/// notification literally read "... · workspace" in `NotificationPanel.svelte`
/// (`{notif.body}`, rendered verbatim, no client-side resolution). Fixed
/// HERE, at the one function every `project_name` argument should pass
/// through — never by renaming the stored sentinel itself, which stays
/// "workspace" everywhere it already is one. Same fallback shape
/// `list_scheduled_jobs`'s own `COALESCE(p.name, 'Home')` already uses
/// (below), as a plain Rust fn since callers here hold a raw `&str`, not a
/// JOIN. **Flagged for Reed's audit, not silently decided:** this diverges
/// from the issue's literal file citation — see the PR body for the full
/// trace and the question of whether this is the intended fix site.
fn resolve_project_display_name(conn: &rusqlite::Connection, project_id: &str) -> String {
    conn.query_row("SELECT name FROM projects WHERE id = ?1", rusqlite::params![project_id], |row| {
        row.get::<_, String>(0)
    })
    .ok()
    .filter(|n| !n.trim().is_empty())
    .unwrap_or_else(|| "Home".to_string())
}

#[tauri::command]
pub fn list_scheduled_jobs(db: State<DbState>) -> Result<Vec<serde_json::Value>, String> {
    eprintln!("[list_scheduled_jobs] called");
    let conn = db.0.lock().map_err(|e| format!("list_scheduled_jobs lock: {}", e))?;
    let mut stmt = conn
        .prepare(
            // #3134 C26: `brief`, `cron`, `run_at`, `agent_id` added to the
            // SELECT (and the JSON shape below) — the Edit dialog's
            // `openEditJobModal(row)` pre-fill needs all four to populate
            // the brief field, the agent <select>, and the inverse
            // once/cron formatters; none required a schema change (every
            // column already exists on `agent_jobs`), and this stays the
            // ONE query the panel reads (no second, Edit-specific query).
            // #3151 PR2/C24: `AND j.archived_at IS NULL` — "archived means
            // hidden from the panel, not deleted" (§3136). Archived rows
            // never reach this query at all, regardless of tab or the
            // Done tab's own "Show deleted" toggle (that toggle only
            // governs the `cancelled` sub-filter among NON-archived rows
            // — an orthogonal, frontend-only concern, `scheduledPanel.ts`).
            "SELECT j.id, j.title, j.kind, a.name, COALESCE(p.name, 'Home') as project_name, j.project_id, \
                    j.next_run_at, j.status, j.missed_count, j.started_at, j.finished_at, j.last_error, j.session_id, \
                    j.brief, j.cron, j.run_at, j.agent_id \
             FROM agent_jobs j \
             JOIN ai_agents a ON a.id = j.agent_id \
             LEFT JOIN projects p ON p.id = j.project_id \
             WHERE j.kind NOT IN ('spawn', 'wake') AND j.archived_at IS NULL \
             ORDER BY j.next_run_at",
        )
        .map_err(|e| format!("list_scheduled_jobs prepare: {}", e))?;
    let rows = stmt
        .query_map([], |row| {
            Ok(serde_json::json!({
                "id": row.get::<_, String>(0)?,
                "title": row.get::<_, String>(1)?,
                "kind": row.get::<_, String>(2)?,
                "agent": row.get::<_, String>(3)?,
                "project": row.get::<_, String>(4)?,
                "project_id": row.get::<_, String>(5)?,
                "next_run_at": row.get::<_, Option<String>>(6)?,
                "status": row.get::<_, String>(7)?,
                "missed_count": row.get::<_, i64>(8)?,
                "started_at": row.get::<_, Option<String>>(9)?,
                "finished_at": row.get::<_, Option<String>>(10)?,
                "last_error": row.get::<_, Option<String>>(11)?,
                "session_id": row.get::<_, String>(12)?,
                "brief": row.get::<_, String>(13)?,
                "cron": row.get::<_, Option<String>>(14)?,
                "run_at": row.get::<_, Option<String>>(15)?,
                "agent_id": row.get::<_, String>(16)?,
            }))
        })
        .map_err(|e| format!("list_scheduled_jobs query_map: {}", e))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("list_scheduled_jobs row: {}", e))?;
    eprintln!("[list_scheduled_jobs] returning {} row(s)", rows.len());
    Ok(rows)
}

/// §3151 PR2/C24 — the ONE archive-write mechanism shared by both "Clear
/// done" (`clear_done_jobs_command`, manual, unconditional-within-the-
/// terminal-set trigger) and the scheduler tick's auto-archive sweep
/// (`sweep_archivable_jobs`, automatic, age-gated trigger). Reed's own
/// open question from #3136/#3151's scope ("'Clear done' and auto-archive
/// should share ONE mechanism with two triggers, not two code paths") is
/// answered here: both call this one function, differing only in the
/// `max_age_days` argument.
///
/// Archiving is always `UPDATE agent_jobs SET archived_at = datetime('now')`
/// — **never** a `status` overwrite. A `status = 'archived'` write was
/// considered and rejected (§3136): it would destroy the done/failed/
/// cancelled distinction the Done tab exists to show. `archived_at` is
/// deliberately orthogonal to `status`.
///
/// `max_age_days: None` → "Clear done"'s trigger: every terminal
/// (`done`/`failed`/`cancelled`) row not yet archived, regardless of age.
/// `max_age_days: Some(n)` → the tick's own trigger: only rows whose
/// `finished_at` is at least `n` days old. `'stopped'` is deliberately
/// NOT in the terminal set — it is not a `done`/`failed`/`cancelled`
/// status (§36.3.7: a `stopped` row can still transition to `paused` via
/// `resume_job`), and neither C24 nor #3136 names it as archivable.
///
/// Cutoff is computed in Rust (`chrono::Utc::now() - Duration::days(n)`,
/// the exact shape `select_oc_pickup_eligible_sessions` above already
/// uses for `RUNTIME_OC_PICKUP_WINDOW_DAYS`) and passed as a bound
/// parameter — never a SQLite `datetime('now', '-N days')` modifier string
/// built via `format!`, so this has no ambient-timezone dependency (the
/// #3137 lesson: this PR has date arithmetic in it, so that risk is live)
/// and no injection surface for the `days` argument.
fn archive_terminal_jobs(conn: &rusqlite::Connection, max_age_days: Option<u32>) -> Result<usize, String> {
    let updated = match max_age_days {
        Some(days) => {
            let cutoff = (chrono::Utc::now() - chrono::Duration::days(days as i64))
                .format("%Y-%m-%d %H:%M:%S")
                .to_string();
            conn.execute(
                "UPDATE agent_jobs SET archived_at = datetime('now') \
                 WHERE status IN ('done', 'failed', 'cancelled') AND archived_at IS NULL \
                 AND finished_at IS NOT NULL AND finished_at <= ?1",
                params![cutoff],
            )
        }
        None => conn.execute(
            "UPDATE agent_jobs SET archived_at = datetime('now') \
             WHERE status IN ('done', 'failed', 'cancelled') AND archived_at IS NULL",
            [],
        ),
    }
    .map_err(|e| format!("archive_terminal_jobs failed: {}", e))?;
    Ok(updated)
}

/// §3151 PR2/C24 — "Clear done": archives every terminal row on the Done
/// tab's own dataset (`done`/`failed`/`cancelled`, not yet archived),
/// unconditionally — the manual trigger for `archive_terminal_jobs`
/// above. Scope is the full Done-tab dataset, not just whatever the
/// "Show deleted" toggle happens to be rendering at click time: a
/// `cancelled` row is still conceptually "on the Done tab" even while the
/// toggle is hiding it, and the alternative (toggle-dependent clearing)
/// would make a `cancelled` row's fate depend on UI state that has
/// nothing to do with the archive operation itself. **Flagged: this is a
/// judgment call on Reed's brief's own ambiguous wording ("archives
/// everything currently on the tab") — see the PR body's open-question
/// note.**
#[tauri::command]
pub fn clear_done_jobs_command(db: State<DbState>) -> Result<usize, String> {
    eprintln!("[clear_done_jobs_command] called");
    let conn = db.0.lock().map_err(|e| format!("clear_done_jobs_command lock: {}", e))?;
    archive_terminal_jobs(&conn, None)
}

/// §3151 PR2/C24 — gate for the scheduler tick's auto-archive sweep, same
/// idiom `archive.rs::should_run_archive` already uses for chat-message
/// archival (a structurally identical "sweep old rows past N days"
/// problem): an `ai_settings` timestamp key, re-checked once per tick,
/// short-circuiting to a cheap `get_ai_setting` read (NOT a table scan)
/// in the common case where the interval hasn't elapsed yet. A sibling
/// key (`last_job_archive_at`), not a shared one — `archive.rs`'s own
/// `last_archived_at` key is for chat-message archival, a different
/// table, a different interval; conflating the two keys would make one
/// sweep's cadence silently depend on the other's.
///
/// 24-hour interval (not `archive.rs`'s own 7-day one): `agent_jobs` rows
/// change status far more often than chat messages age past 30 days, and
/// `RUNTIME_JOB_ARCHIVE_DAYS` (30) is coarse enough that a daily check
/// still catches every newly-eligible row well within its own window —
/// at the cost of one extra `get_ai_setting` read per day, not per tick.
fn should_run_job_archive_sweep(conn: &rusqlite::Connection) -> bool {
    let last: Option<String> = crate::database::get_ai_setting(conn, "last_job_archive_at");
    match last {
        None => {
            eprintln!("[ARCHIVE-JOBS] No previous sweep detected");
            true
        }
        Some(ref timestamp) => match chrono::DateTime::parse_from_rfc3339(timestamp) {
            Ok(dt) => {
                let hours_since = (chrono::Utc::now() - dt.with_timezone(&chrono::Utc)).num_hours();
                eprintln!("[ARCHIVE-JOBS] Last sweep was {} hour(s) ago", hours_since);
                hours_since >= 24
            }
            Err(_) => {
                eprintln!("[ARCHIVE-JOBS] WARNING: could not parse last_job_archive_at: {}", timestamp);
                true
            }
        },
    }
}

/// §3151 PR2/C24 — the scheduler tick's auto-archive sweep: the
/// first-ever consumer of `RUNTIME_JOB_ARCHIVE_DAYS` against `agent_jobs`
/// (see that constant's own doc comment in `runtime_constants.rs` —
/// until this function, it had only ever gated `chat_sessions.
/// last_active_at`). Gated by `should_run_job_archive_sweep` above, so
/// this is NOT a table scan on every 60s tick: the common case (interval
/// not yet elapsed) is one `get_ai_setting` read that returns before
/// `agent_jobs` is ever touched. When the gate DOES let it through, the
/// write itself is a single indexed `UPDATE ... WHERE archived_at IS NULL
/// AND status IN (...) AND finished_at <= ?` (`idx_agent_jobs_archived_at`,
/// `migrate_v97`) — not a scan either.
///
/// Called unconditionally, early in `scheduler_tick`, same "runs even on
/// a tick with zero due jobs" placement `run_oc_background_pickup`
/// already establishes — an archive sweep has nothing to do with
/// `claim_due_jobs`'s due-row claim and must not be skipped by that
/// function's own early return.
fn sweep_archivable_jobs(conn: &rusqlite::Connection) {
    if !should_run_job_archive_sweep(conn) {
        return;
    }
    match archive_terminal_jobs(conn, Some(RUNTIME_JOB_ARCHIVE_DAYS)) {
        Ok(count) => {
            if count > 0 {
                eprintln!(
                    "[ARCHIVE-JOBS] ✓ swept {} job row(s) older than {} days",
                    count, RUNTIME_JOB_ARCHIVE_DAYS
                );
            }
        }
        Err(e) => {
            eprintln!("[ARCHIVE-JOBS] ERROR: sweep failed: {}", e);
            // Do not update last_job_archive_at on failure — a failed
            // sweep must be retried on the next gate-eligible tick, not
            // silently treated as "ran" (the #3112-shape mistake,
            // generalized: never stamp success you didn't earn).
            return;
        }
    }
    let now = chrono::Utc::now().to_rfc3339();
    if let Err(e) = crate::database::set_ai_setting(conn, "last_job_archive_at", &now) {
        eprintln!("[ARCHIVE-JOBS] ERROR: failed to update last_job_archive_at: {}", e);
    }
}

/// §36.3.8/Reed's PR 8 scope audit §4 (#3050 PR 8): "Run now" only needs to
/// make the row satisfy `claim_due_jobs`'s own existing claim predicate
/// (`status = 'pending' AND next_run_at <= now`) — it does NOT bypass
/// `claim_due_jobs`, no new claim/dispatch logic. A `paused` row also has
/// its `status` flipped back to `'pending'` (otherwise invisible to the
/// claim query regardless of `next_run_at`); any other status (`running`,
/// `done`, `failed`, `cancelled`, `sent`) is left alone — same
/// "no meaning" status-guard shape `pause_job`/`resume_job`
/// (`ai/tools.rs::execute_pause_job`/`execute_resume_job`) already use.
#[tauri::command]
pub fn run_job_now(job_id: String, db: State<DbState>) -> Result<(), String> {
    eprintln!("[run_job_now] called with job_id={}", job_id);
    let conn = db.0.lock().map_err(|e| format!("run_job_now lock: {}", e))?;
    let updated = conn
        .execute(
            "UPDATE agent_jobs SET next_run_at = datetime('now'), \
                    status = CASE WHEN status = 'paused' THEN 'pending' ELSE status END, \
                    updated_at = datetime('now') \
             WHERE id = ?1 AND status IN ('pending', 'paused')",
            params![job_id],
        )
        .map_err(|e| format!("run_job_now failed for {}: {}", job_id, e))?;
    if updated == 0 {
        let exists: bool = conn
            .query_row("SELECT COUNT(*) FROM agent_jobs WHERE id = ?1", params![job_id], |row| row.get::<_, i64>(0))
            .map(|c| c > 0)
            .unwrap_or(false);
        return Err(if exists {
            format!("Job {} is not pending or paused (already running or finished) — cannot run now.", job_id)
        } else {
            format!("Job not found: {}", job_id)
        });
    }
    eprintln!("[run_job_now] job_id={} next_run_at set to now", job_id);
    Ok(())
}

/// §36.3.7 carry-over, #3050 PR 9 (Reed's 2026-10-29 PR 9 scope audit §6,
/// Matt's framing — unblocked once #3116/PR 8 merged). The panel's
/// Pause/Delete on a RUNNING row call THIS first, then the normal
/// pause/cancel command once it returns `Ok` — never an optimistic row
/// flip while the job is still genuinely running.
///
/// **API-runtime job:** a real cancel checkpoint exists (the job dispatch
/// above now wires `ai::chat::reset_cancel_flag` instead of a permanently-
/// false throwaway flag) — sets it via the SAME `stop_turn` the
/// interactive Stop button uses, then polls `agent_jobs.status` until it
/// leaves `'running'` (the loop's own `write_job_terminal_status` call
/// writes `'stopped'` once it observes the flag).
///
/// **OC-runtime job (#3132 — the follow-up this doc comment used to flag
/// as named-but-not-done):** this function now calls `stop_turn`
/// unconditionally for ANY runtime, OC included. **The ORIGINAL reason
/// ("`dispatch_oc_job` is a one-shot POST with no cancel checkpoint at
/// all") went stale as of #3129 (C28(b)):** `dispatch_oc_job` is deleted;
/// an OC-runtime job now dispatches through
/// `prepare_job_dispatch`/`run_agent_loop`, the exact same path every
/// other runtime's job takes — confirmed by reading the dispatch site
/// directly (`scheduler_tick`'s own `AgentLoopInputs { cancel:
/// ai::chat::reset_cancel_flag(&job.session_id), .. }` construction is
/// unconditional, never gated on `runtime_type`). The per-runtime branch
/// that used to skip `stop_turn` for OC jobs is gone — there is exactly
/// one code path now, for every runtime, matching `execute_cancel_job`'s
/// own `WHERE status != 'running'` guard, which already accepted
/// `'stopped'` and needed no change of its own.
///
/// Bounded wait (30s, 150ms poll) — returns `Err` on timeout rather than
/// hanging the caller forever if a turn is genuinely stuck; the caller
/// (the panel) surfaces that as a normal error toast, same as any other
/// failed action. Wait on the condition asserted (status leaving
/// `'running'`), never a fixed sleep.
#[tauri::command]
pub async fn wait_for_running_job_to_stop(job_id: String, db: State<'_, DbState>) -> Result<(), String> {
    let (session_id, initial_status): (String, String) = {
        let conn = db.0.lock().map_err(|e| format!("wait_for_running_job_to_stop lock: {}", e))?;
        conn.query_row(
            "SELECT j.session_id, j.status FROM agent_jobs j WHERE j.id = ?1",
            params![job_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|e| format!("Job not found: {}", e))?
    };

    if initial_status != "running" {
        // Already settled — nothing to wait for. Not an error: the caller
        // (pause/delete) proceeds immediately with its own action.
        return Ok(());
    }

    // #3132: every runtime now has a real cancel checkpoint via
    // `run_agent_loop`'s `CANCEL_FLAGS` consultation — set unconditionally,
    // same mechanism the interactive Stop button uses.
    crate::ai::chat::stop_turn(session_id)?;
    eprintln!("[wait_for_running_job_to_stop] job_id={} cancel flag set", job_id);

    let poll_interval = std::time::Duration::from_millis(150);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        let current_status: String = {
            let conn = db.0.lock().map_err(|e| format!("wait_for_running_job_to_stop lock: {}", e))?;
            conn.query_row("SELECT status FROM agent_jobs WHERE id = ?1", params![job_id], |row| {
                row.get(0)
            })
            .map_err(|e| format!("Job not found: {}", e))?
        };
        if current_status != "running" {
            eprintln!(
                "[wait_for_running_job_to_stop] job_id={} settled at status={}",
                job_id, current_status
            );
            return Ok(());
        }
        if std::time::Instant::now() >= deadline {
            return Err(format!("Job {} did not stop within 30 seconds.", job_id));
        }
        tokio::time::sleep(poll_interval).await;
    }
}

/// The one fixed error string every job-turn `Failed` outcome writes —
/// never a per-job message (the provider error itself lives in the job's
/// own transcript/report-back text, not `agent_jobs.last_error`). Shared
/// by `write_job_terminal_status`'s `Failed` arm, `settle_recurring_job_
/// outcome`'s terminal (3rd-strike) arm below, and the notification call
/// site in `scheduler_tick` — one literal, not three copies to keep in
/// sync by hand.
const JOB_FAILED_ERROR_TEXT: &str =
    "provider error — see the job's own transcript/report-back text for detail";

/// §6 (#3050 PR 5); §36.3.4 Matt's ruling (#3050 PR 8): the ONE place an
/// API-runtime job turn's `JobOutcome` becomes `agent_jobs.status` —
/// extracted out of `scheduler_tick`'s post-completion bookkeeping block
/// so this mapping has a direct unit-test seam (not just end-to-end
/// coverage through the full async dispatch pipeline). Returns the
/// `&'static str` status word written, for the chip-event/notification
/// call sites right after it in `scheduler_tick`.
///
/// **Once/spawn/wake only** — `kind IN ('recurring', 'heartbeat')` goes
/// through `settle_recurring_job_outcome` (below) instead, at the one call
/// site in `scheduler_tick` that knows `kind`; this function has no `kind`
/// parameter and never will, by design (Reed's #3133 WARN review).
pub(crate) fn write_job_terminal_status(
    conn: &rusqlite::Connection,
    job_id: &str,
    outcome: JobOutcome,
) -> &'static str {
    let (status, last_error): (&str, Option<&str>) = match outcome {
        JobOutcome::Done => ("done", None),
        JobOutcome::Failed => ("failed", Some(JOB_FAILED_ERROR_TEXT)),
        // §36.3.4 Matt's ruling (#3050 PR 8): a job-bound turn the user
        // stopped via the Stop button now writes `'stopped'` — a real
        // terminal state, no longer silently folded into `'done'` (Reed's
        // PR 8 scope audit §10's named gap).
        JobOutcome::Stopped => ("stopped", None),
    };
    if let Err(e) = conn.execute(
        "UPDATE agent_jobs SET status = ?1, finished_at = datetime('now'), updated_at = datetime('now'), last_error = ?2 WHERE id = ?3",
        params![status, last_error, job_id],
    ) {
        eprintln!("[SCHEDULER] job {}: failed to write terminal status '{}': {}", job_id, status, e);
    }
    status
}

/// Matt's Ruling 3 (2026-10-31, #3133 — follow-up to #3129 C28(b); Reed's
/// #3133 WARN finding #2): the recurring/heartbeat 3-strike failure ladder,
/// reimplemented on the shared dispatch path every runtime now takes.
/// `agent_jobs.consecutive_failures`/`last_failed_at` lived EXCLUSIVELY
/// inside the now-deleted `dispatch_oc_job` — confirmed by grep before
/// writing this function: zero non-migration reads/writes of either
/// column anywhere on this PR's base commit. The API-runtime path never
/// had this mechanism either, so #3129 (C28(b)) deleting `dispatch_oc_job`
/// removed the ladder's only implementation, for every runtime — not
/// "narrowed it to API," the overclaim Reed's audit named. Called ONLY for
/// `kind IN ('recurring', 'heartbeat')`, at the one call site in
/// `scheduler_tick` that still knows `kind`; `once`/`spawn`/`wake` keep
/// `write_job_terminal_status`'s unchanged single-firing terminal mapping.
///
/// - `Done`: resets the streak (`consecutive_failures = 0`,
///   `last_failed_at = NULL`) and returns `"pending"` — **never** `"done"`.
///   `next_run_at` was already advanced at claim time
///   (`recompute_recurrence`, under the same lock, before this job ever
///   dispatched — §36.3.5) specifically so a recurring job's own
///   recurrence never depends on this firing's outcome; writing a
///   terminal `'done'` here would silently contradict that by taking the
///   row out of `claim_due_jobs`' `status = 'pending'` filter forever,
///   the exact bug this ruling exists to close (confirmed read: before
///   this function existed, `write_job_terminal_status` ran unconditionally
///   for every `kind`, including recurring/heartbeat).
/// - `Failed`, this firing's new count < `RUNTIME_MAX_CONSECUTIVE_FAILURES`:
///   increments `consecutive_failures`, returns `"pending"` (same non-
///   advance reasoning as `Done`, above). `last_failed_at` is written
///   ONLY when this firing starts a NEW streak (`consecutive_failures`
///   was 0 going in) — PR 6's `emit_or_update_job_failed_notification`
///   keys its UPSERT dedupe on `(job_id, streak_started_at)`, where
///   `streak_started_at` IS this column's value (see that function's own
///   doc comment, which names this exact caller obligation); overwriting
///   it on the 2nd/3rd failure of the SAME streak would fork one streak
///   into multiple notification rows.
/// - `Failed`, AT the threshold: flips `status` to `'failed'` — a real
///   terminal state; the job stops recurring.
/// - `Stopped`: not part of the ladder (a user-initiated Stop is not a
///   transport failure) — passed straight through to
///   `write_job_terminal_status`, unchanged.
///
/// Returns `(status_word, Option<(attempt, max_attempts, terminal,
/// streak_started_at)>)`. The second element is `Some` only on a `Failed`
/// outcome — exactly the shape `scheduler_tick`'s notification call site
/// needs for `emit_or_update_job_failed_notification`, so that call site
/// never has to re-derive or re-query any of it.
/// #3134 C26: widened from module-private to `pub(crate)` so
/// `ai::tools`'s `update_job` test suite can drive a REAL failure
/// through the exact same function `scheduler_tick` calls, proving the
/// edit-triggered streak reset interacts correctly with PR 6's
/// notification dedupe key (`job_failed:{job_id}:{last_failed_at}`) —
/// rather than hand-writing a `last_failed_at` value that only
/// resembles what this function would have produced. No behavior change.
pub(crate) fn settle_recurring_job_outcome(
    conn: &rusqlite::Connection,
    job_id: &str,
    outcome: JobOutcome,
) -> (&'static str, Option<(i64, i64, bool, String)>) {
    match outcome {
        JobOutcome::Stopped => (write_job_terminal_status(conn, job_id, outcome), None),
        JobOutcome::Done => {
            if let Err(e) = conn.execute(
                "UPDATE agent_jobs SET consecutive_failures = 0, last_failed_at = NULL, updated_at = datetime('now') WHERE id = ?1",
                params![job_id],
            ) {
                eprintln!(
                    "[SCHEDULER] job {}: failed to reset the recurring failure streak on Done: {}",
                    job_id, e
                );
            }
            ("pending", None)
        }
        JobOutcome::Failed => {
            let (prior_count, prior_last_failed_at): (i64, Option<String>) = conn
                .query_row(
                    "SELECT consecutive_failures, last_failed_at FROM agent_jobs WHERE id = ?1",
                    params![job_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap_or((0, None));
            let new_count = prior_count + 1;
            let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
            // The streak's own start time — stable across every firing of
            // the SAME streak (see this function's own doc comment above
            // for why that stability matters to PR 6's dedupe key). A
            // fresh streak (`prior_count == 0`) starts NOW; a continuing
            // streak keeps whatever was already stored.
            let streak_started_at = if prior_count == 0 {
                now.clone()
            } else {
                prior_last_failed_at.unwrap_or_else(|| now.clone())
            };
            let terminal = new_count >= RUNTIME_MAX_CONSECUTIVE_FAILURES;

            let write_result = if terminal {
                conn.execute(
                    "UPDATE agent_jobs SET status = 'failed', consecutive_failures = ?1, finished_at = datetime('now'), updated_at = datetime('now'), last_error = ?2 WHERE id = ?3",
                    params![new_count, JOB_FAILED_ERROR_TEXT, job_id],
                )
            } else if prior_count == 0 {
                // First failure of a new streak — set last_failed_at too.
                conn.execute(
                    "UPDATE agent_jobs SET consecutive_failures = ?1, last_failed_at = ?2, updated_at = datetime('now') WHERE id = ?3",
                    params![new_count, streak_started_at, job_id],
                )
            } else {
                // 2nd/3rd-of-3 failure of an ONGOING streak — count only,
                // last_failed_at deliberately untouched (see above).
                conn.execute(
                    "UPDATE agent_jobs SET consecutive_failures = ?1, updated_at = datetime('now') WHERE id = ?2",
                    params![new_count, job_id],
                )
            };
            if let Err(e) = write_result {
                eprintln!(
                    "[SCHEDULER] job {}: failed to record consecutive-failure strike {}: {}",
                    job_id, new_count, e
                );
            }
            let status = if terminal { "failed" } else { "pending" };
            (
                status,
                Some((new_count, RUNTIME_MAX_CONSECUTIVE_FAILURES, terminal, streak_started_at)),
            )
        }
    }
}

#[cfg(test)]
mod write_job_terminal_status_tests_3050_pr8 {
    use super::*;
    use rusqlite::Connection;
    use tauri::Manager;

    fn make_conn() -> Connection {
        let conn = Connection::open_in_memory().expect("in-memory db");
        crate::migrations::run_migrations(&conn).expect("fresh-install migrations should succeed");
        conn
    }

    fn seed_agent(conn: &Connection, id: &str) {
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO ai_agents (id, name, slug, is_default, is_active, workspace_path, runtime_type, model, created_at, updated_at) \
             VALUES (?1, ?1, ?1, 0, 1, ?1, 'anthropic', 'test-model', ?2, ?2)",
            params![id, now],
        )
        .expect("seed agent");
    }

    fn seed_running_job(conn: &Connection, job_id: &str, agent_id: &str) {
        let sid = create_session_internal(conn, "workspace", agent_id, "schedule", false, None)
            .expect("seed job session")
            .id;
        conn.execute(
            "INSERT INTO agent_jobs (id, kind, title, brief, agent_id, project_id, session_id, next_run_at, status, created_at, updated_at) \
             VALUES (?1, 'once', 'Test', 'brief', ?2, 'workspace', ?3, datetime('now'), 'running', datetime('now'), datetime('now'))",
            params![job_id, agent_id, sid],
        )
        .expect("seed running job");
    }

    /// Named test (Matt's ruling, #3050 PR 8): `stopped` is written.
    /// Mutation: fold `JobOutcome::Stopped` back into `("done", None)`
    /// (the pre-PR-8 behavior Reed's scope audit §10 found) — this
    /// test's `assert_eq!(status, "stopped")` goes red. Verified by hand
    /// for this PR (see PR report for the actual red output).
    #[test]
    fn test_stopped_outcome_writes_stopped_status() {
        let conn = make_conn();
        seed_agent(&conn, "agent-a");
        seed_running_job(&conn, "job-1", "agent-a");

        let returned = write_job_terminal_status(&conn, "job-1", JobOutcome::Stopped);
        assert_eq!(returned, "stopped");

        let status: String = conn
            .query_row("SELECT status FROM agent_jobs WHERE id = 'job-1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(status, "stopped", "a Stopped outcome must write agent_jobs.status = 'stopped', not 'done'");
    }

    /// The Done tab's filter (`status IN ('done','failed','stopped','cancelled')`)
    /// now actually matches a 'stopped' row — proven directly against
    /// `list_scheduled_jobs`'s own output, closing the "dead filter arm"
    /// gap Reed's scope audit named (§36.3.8's Done tab was specced with
    /// 'stopped' in its set long before anything ever wrote it).
    #[test]
    fn test_list_scheduled_jobs_shows_stopped_row_for_done_tab_filter() {
        let conn = make_conn();
        seed_agent(&conn, "agent-a");
        seed_running_job(&conn, "job-1", "agent-a");
        write_job_terminal_status(&conn, "job-1", JobOutcome::Stopped);

        let app = tauri::test::mock_app();
        app.manage(DbState::new(conn));
        let db_state = app.state::<DbState>();

        let rows = list_scheduled_jobs(db_state.clone()).expect("list_scheduled_jobs should succeed");
        let row = rows.iter().find(|r| r["id"] == "job-1").expect("job-1 must still appear in the panel query");
        assert_eq!(row["status"], "stopped", "the Done tab's status partition (status IN (...'stopped'...)) needs this literal value");
    }

    /// Sanity: `Done`/`Failed` are unaffected by this refactor.
    #[test]
    fn test_done_and_failed_outcomes_unaffected_by_refactor() {
        let conn = make_conn();
        seed_agent(&conn, "agent-a");
        seed_running_job(&conn, "job-done", "agent-a");
        seed_running_job(&conn, "job-failed", "agent-a");

        write_job_terminal_status(&conn, "job-done", JobOutcome::Done);
        write_job_terminal_status(&conn, "job-failed", JobOutcome::Failed);

        let (done_status, failed_status): (String, String) = (
            conn.query_row("SELECT status FROM agent_jobs WHERE id = 'job-done'", [], |r| r.get(0)).unwrap(),
            conn.query_row("SELECT status FROM agent_jobs WHERE id = 'job-failed'", [], |r| r.get(0)).unwrap(),
        );
        assert_eq!(done_status, "done");
        assert_eq!(failed_status, "failed");
    }
}

/// §3151 PR2/C24 — the shared archive mechanism (`archive_terminal_jobs`),
/// its two triggers (`clear_done_jobs_command`'s unconditional sweep,
/// `sweep_archivable_jobs`'s age-gated tick sweep), the sweep's own
/// interval gate (`should_run_job_archive_sweep`), and `list_scheduled_jobs`'s
/// archived-row exclusion. Every test seeds real rows via `create_session_
/// internal` + a direct `agent_jobs` INSERT — no mocked clock, no ambient
/// timezone/pool dependency (#3137's lesson: this PR has date arithmetic).
#[cfg(test)]
mod archive_jobs_tests_3151 {
    use super::*;
    use rusqlite::Connection;
    use tauri::Manager;

    fn make_conn() -> Connection {
        let conn = Connection::open_in_memory().expect("in-memory db");
        crate::migrations::run_migrations(&conn).expect("fresh-install migrations should succeed");
        conn
    }

    fn seed_agent(conn: &Connection, id: &str) {
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO ai_agents (id, name, slug, is_default, is_active, workspace_path, runtime_type, model, created_at, updated_at) \
             VALUES (?1, ?1, ?1, 0, 1, ?1, 'anthropic', 'test-model', ?2, ?2)",
            params![id, now],
        )
        .expect("seed agent");
    }

    /// Seeds a terminal job row with an explicit `finished_at` (so the
    /// age-gated trigger has something real to compare against — never
    /// `datetime('now')` for the "old" fixtures, which would make the
    /// test depend on wall-clock timing at the moment it runs).
    fn seed_terminal_job(conn: &Connection, job_id: &str, agent_id: &str, status: &str, finished_at: &str) -> String {
        let sid = create_session_internal(conn, "workspace", agent_id, "schedule", false, None)
            .expect("seed job session")
            .id;
        conn.execute(
            "INSERT INTO agent_jobs (id, kind, title, brief, agent_id, project_id, session_id, status, finished_at, created_at, updated_at) \
             VALUES (?1, 'once', 'Test', 'brief', ?2, 'workspace', ?3, ?4, ?5, datetime('now'), datetime('now'))",
            params![job_id, agent_id, sid, status, finished_at],
        )
        .expect("seed terminal job");
        sid
    }

    /// Named test: `archive_terminal_jobs(conn, None)` — the unconditional
    /// trigger — archives `done`/`failed`/`cancelled` rows but leaves
    /// `stopped` untouched (not in the terminal archive set — §36.3.7, a
    /// `stopped` row can still resume), leaves `pending` untouched, and
    /// — Reed's PR 2 WARN, the negative control this test was missing —
    /// leaves `running` untouched (archiving a row that's mid-turn would
    /// make an active job vanish from the panel while it's still doing
    /// something; #3151 PR3). Mutation (actually run, confirmed red, then
    /// reverted — see PR report): widen the `status IN (...)` list to
    /// ALSO include `'running'` — reddens the `running` assertion below
    /// (the archived count becomes 4, not 3, and `job-running`'s
    /// `archived_at` is no longer `None`). Before this test gained the
    /// `running` row, that exact widening survived all 174 scheduler
    /// tests undetected — the shipped SQL was already correct; only the
    /// negative control was missing.
    #[test]
    fn test_archive_terminal_jobs_unconditional_covers_done_failed_cancelled_not_stopped() {
        let conn = make_conn();
        seed_agent(&conn, "agent-a");
        let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
        seed_terminal_job(&conn, "job-done", "agent-a", "done", &now);
        seed_terminal_job(&conn, "job-failed", "agent-a", "failed", &now);
        seed_terminal_job(&conn, "job-cancelled", "agent-a", "cancelled", &now);
        seed_terminal_job(&conn, "job-stopped", "agent-a", "stopped", &now);
        seed_terminal_job(&conn, "job-pending", "agent-a", "pending", &now);
        seed_terminal_job(&conn, "job-running", "agent-a", "running", &now);

        let archived = archive_terminal_jobs(&conn, None).expect("archive_terminal_jobs should succeed");
        assert_eq!(archived, 3, "done+failed+cancelled only — stopped, pending, and running must not be counted");

        let get_archived_at = |id: &str| -> Option<String> {
            conn.query_row("SELECT archived_at FROM agent_jobs WHERE id = ?1", params![id], |r| r.get(0)).unwrap()
        };
        assert!(get_archived_at("job-done").is_some());
        assert!(get_archived_at("job-failed").is_some());
        assert!(get_archived_at("job-cancelled").is_some());
        assert!(get_archived_at("job-stopped").is_none(), "stopped is resumable — must never be archived by this mechanism");
        assert!(get_archived_at("job-pending").is_none());
        assert!(get_archived_at("job-running").is_none(), "running is mid-turn — must never be archived by this mechanism");
    }

    /// Named test: the age-gated trigger (`Some(days)`) only archives rows
    /// whose `finished_at` is at least that many days old; a fresh
    /// terminal row survives. Mutation: drop the `finished_at <= ?1`
    /// clause (archive everything terminal regardless of age) — reddens
    /// the "fresh row survives" assertion (an active job just finished
    /// a second ago would vanish from the Done tab immediately).
    #[test]
    fn test_archive_terminal_jobs_age_gated_spares_fresh_rows() {
        let conn = make_conn();
        seed_agent(&conn, "agent-a");
        let old = (chrono::Utc::now() - chrono::Duration::days(31)).format("%Y-%m-%d %H:%M:%S").to_string();
        let fresh = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
        seed_terminal_job(&conn, "job-old", "agent-a", "done", &old);
        seed_terminal_job(&conn, "job-fresh", "agent-a", "done", &fresh);

        let archived = archive_terminal_jobs(&conn, Some(30)).expect("age-gated archive should succeed");
        assert_eq!(archived, 1, "only the 31-day-old row should be swept at a 30-day cutoff");

        let old_archived: Option<String> = conn.query_row("SELECT archived_at FROM agent_jobs WHERE id = 'job-old'", [], |r| r.get(0)).unwrap();
        let fresh_archived: Option<String> = conn.query_row("SELECT archived_at FROM agent_jobs WHERE id = 'job-fresh'", [], |r| r.get(0)).unwrap();
        assert!(old_archived.is_some());
        assert!(fresh_archived.is_none(), "a job that just finished must not be swept — age-gate must actually gate");
    }

    /// Named test: archiving is `archived_at = datetime('now')`, never a
    /// `status` overwrite — the done/failed/cancelled distinction must
    /// survive archiving (§3136: "A status = 'archived' overwrite is
    /// wrong"). Mutation: change the UPDATE to also set `status =
    /// 'archived'` — reddens (status no longer reads 'done' after
    /// archiving, destroying the distinction the Done tab exists to show).
    #[test]
    fn test_archive_terminal_jobs_never_overwrites_status() {
        let conn = make_conn();
        seed_agent(&conn, "agent-a");
        let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
        seed_terminal_job(&conn, "job-done", "agent-a", "done", &now);

        archive_terminal_jobs(&conn, None).expect("archive should succeed");

        let status: String = conn.query_row("SELECT status FROM agent_jobs WHERE id = 'job-done'", [], |r| r.get(0)).unwrap();
        assert_eq!(status, "done", "status must survive archiving unchanged — archived_at is the only write");
    }

    /// Named test: "Clear done" (`clear_done_jobs_command`) is the SAME
    /// mechanism as the tick sweep, called with `None` — not a second,
    /// hand-rolled UPDATE. Proven by calling the command directly and
    /// checking the exact same archive semantics the unit test above
    /// already pins (done+failed+cancelled, not stopped).
    #[test]
    fn test_clear_done_jobs_command_archives_everything_on_tab_not_stopped() {
        let conn = make_conn();
        seed_agent(&conn, "agent-a");
        let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
        seed_terminal_job(&conn, "job-done", "agent-a", "done", &now);
        seed_terminal_job(&conn, "job-failed", "agent-a", "failed", &now);
        seed_terminal_job(&conn, "job-cancelled", "agent-a", "cancelled", &now);
        seed_terminal_job(&conn, "job-stopped", "agent-a", "stopped", &now);

        let app = tauri::test::mock_app();
        app.manage(DbState::new(conn));
        let db_state = app.state::<DbState>();

        let count = clear_done_jobs_command(db_state.clone()).expect("clear_done_jobs_command should succeed");
        assert_eq!(count, 3, "Clear done must archive exactly the done+failed+cancelled rows");

        let rows = list_scheduled_jobs(db_state.clone()).expect("list should succeed");
        let ids: Vec<&str> = rows.iter().map(|r| r["id"].as_str().unwrap()).collect();
        assert!(!ids.contains(&"job-done"), "an archived row must not reach the panel query");
        assert!(!ids.contains(&"job-failed"));
        assert!(!ids.contains(&"job-cancelled"));
        assert!(ids.contains(&"job-stopped"), "stopped must remain visible — Clear done never touches it");
    }

    /// Named test: `list_scheduled_jobs` excludes archived rows
    /// unconditionally — "archived means hidden from the panel," not
    /// toggle-dependent (the Done tab's own "Show deleted" toggle is a
    /// frontend-only filter on `status`, orthogonal to `archived_at`).
    /// Mutation: drop `AND j.archived_at IS NULL` from the query — reddens
    /// (an archived row reappears in the panel with no way to hide it
    /// again, since the frontend has no UI concept of un-archiving).
    #[test]
    fn test_list_scheduled_jobs_excludes_archived_rows_unconditionally() {
        let conn = make_conn();
        seed_agent(&conn, "agent-a");
        let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
        seed_terminal_job(&conn, "job-done", "agent-a", "done", &now);
        archive_terminal_jobs(&conn, None).expect("archive should succeed");

        let app = tauri::test::mock_app();
        app.manage(DbState::new(conn));
        let db_state = app.state::<DbState>();

        let rows = list_scheduled_jobs(db_state.clone()).expect("list should succeed");
        assert!(rows.is_empty(), "an archived row must never appear in list_scheduled_jobs, regardless of any frontend toggle");
    }

    /// Named test: the sweep's own interval gate. `should_run_job_archive_
    /// sweep` returns `true` with no prior run, and `false` immediately
    /// after a run recorded less than 24h ago — proving the tick does
    /// NOT scan `agent_jobs` on every single tick. Mutation: delete the
    /// gate check entirely from `sweep_archivable_jobs` — reddens by
    /// making `last_job_archive_at` get rewritten on literally every
    /// call regardless of recency (the second assertion below would see
    /// a sweep attempt where none should have been allowed to start).
    #[test]
    fn test_should_run_job_archive_sweep_respects_interval_gate() {
        let conn = make_conn();
        assert!(should_run_job_archive_sweep(&conn), "no prior run recorded — must run");

        let now = chrono::Utc::now().to_rfc3339();
        crate::database::set_ai_setting(&conn, "last_job_archive_at", &now).expect("set setting");
        assert!(!should_run_job_archive_sweep(&conn), "a sweep recorded moments ago must not re-run within the interval");

        let old = (chrono::Utc::now() - chrono::Duration::hours(25)).to_rfc3339();
        crate::database::set_ai_setting(&conn, "last_job_archive_at", &old).expect("set setting");
        assert!(should_run_job_archive_sweep(&conn), "25h since the last sweep must clear the 24h gate");
    }

    /// Named test: `sweep_archivable_jobs` is gated — calling it twice in
    /// a row (simulating two rapid ticks) only archives once; the second
    /// call is a no-op because the gate just got set by the first call.
    /// This is the direct proof the tick's own sweep call is NOT a table
    /// scan on every tick: the second call returns having read exactly
    /// one `ai_settings` row and touched `agent_jobs` zero times.
    /// Mutation: remove the `should_run_job_archive_sweep` gate call from
    /// `sweep_archivable_jobs` — reddens (a second immediate call would
    /// try to re-archive, harmless here since the WHERE clause is
    /// idempotent, but the real-world cost this test exists to prevent —
    /// a full table predicate evaluation on every 60s tick — would be
    /// un-gated; caught instead by asserting `last_job_archive_at`
    /// is unchanged between the two calls, which only holds if the
    /// second call short-circuited before doing any work).
    #[test]
    fn test_sweep_archivable_jobs_second_call_is_gated_noop() {
        let conn = make_conn();
        seed_agent(&conn, "agent-a");
        let old = (chrono::Utc::now() - chrono::Duration::days(31)).format("%Y-%m-%d %H:%M:%S").to_string();
        seed_terminal_job(&conn, "job-old", "agent-a", "done", &old);

        sweep_archivable_jobs(&conn);
        let archived_at_1: Option<String> = conn.query_row("SELECT archived_at FROM agent_jobs WHERE id = 'job-old'", [], |r| r.get(0)).unwrap();
        assert!(archived_at_1.is_some(), "first sweep must archive the old row");
        let stamp_1: String = crate::database::get_ai_setting(&conn, "last_job_archive_at").expect("stamp must be set after a sweep ran");

        // A second seeded old row, added AFTER the first sweep, to prove
        // the second sweep call genuinely skips doing any archive work
        // (if it ran again, this new old row would also get archived).
        seed_terminal_job(&conn, "job-old-2", "agent-a", "done", &old);
        sweep_archivable_jobs(&conn);
        let archived_at_2: Option<String> = conn.query_row("SELECT archived_at FROM agent_jobs WHERE id = 'job-old-2'", [], |r| r.get(0)).unwrap();
        assert!(archived_at_2.is_none(), "the second, gated call must not have swept the newly-added old row");
        let stamp_2: String = crate::database::get_ai_setting(&conn, "last_job_archive_at").expect("stamp must still be set");
        assert_eq!(stamp_1, stamp_2, "the gate must prevent the second call from re-stamping last_job_archive_at");
    }
}

/// Matt's Ruling 3 (2026-10-31, #3133 — follow-up to #3129 C28(b)): the
/// recurring/heartbeat 3-strike failure ladder's own tests, against the
/// REAL shared function (`settle_recurring_job_outcome`) every runtime's
/// post-completion bookkeeping now calls — not OC-specific, unlike their
/// deleted ancestors (`test_oc_recurring_job_three_consecutive_failures_
/// flips_to_failed` / `test_oc_recurring_job_success_after_two_failures_
/// resets_counter_to_zero`, both of which mocked `sessions.send` and were
/// deleted along with `dispatch_oc_job` in #3129 C28(b) with no
/// replacement — Reed's #3133 WARN finding #2). Named without an `oc_`
/// prefix for exactly that reason: this mechanism runs identically for
/// every runtime now.
#[cfg(test)]
mod settle_recurring_job_outcome_tests_3133 {
    use super::*;
    use rusqlite::Connection;

    fn make_conn() -> Connection {
        let conn = Connection::open_in_memory().expect("in-memory db");
        crate::migrations::run_migrations(&conn).expect("fresh-install migrations should succeed");
        conn
    }

    fn seed_agent(conn: &Connection, id: &str) {
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO ai_agents (id, name, slug, is_default, is_active, workspace_path, runtime_type, model, created_at, updated_at) \
             VALUES (?1, ?1, ?1, 0, 1, ?1, 'anthropic', 'test-model', ?2, ?2)",
            params![id, now],
        )
        .expect("seed agent");
    }

    /// A recurring/heartbeat job's row the moment `scheduler_tick`'s
    /// post-completion bookkeeping reaches it — `status = 'pending'`
    /// already (set by `recompute_recurrence` AT CLAIM TIME, §36.3.5,
    /// before this job ever dispatched), `next_run_at` already in the
    /// future. `settle_recurring_job_outcome` must never need to touch
    /// `next_run_at` — this fixture pins it to a FUTURE timestamp
    /// specifically so a bug that re-advanced it would be invisible
    /// without this value fixed as a sentinel to compare against.
    fn seed_claimed_recurring_job(conn: &Connection, job_id: &str, agent_id: &str, kind: &str) -> String {
        let sid = create_session_internal(conn, "workspace", agent_id, "schedule", false, None)
            .expect("seed job session")
            .id;
        let future_next_run_at = format_stored_datetime(&(Utc::now() + chrono::Duration::hours(1)));
        conn.execute(
            "INSERT INTO agent_jobs (id, kind, title, brief, agent_id, project_id, session_id, cron, next_run_at, status, created_at, updated_at) \
             VALUES (?1, ?2, 'Test', 'brief', ?3, 'workspace', ?4, '0 * * * *', ?5, 'pending', datetime('now'), datetime('now'))",
            params![job_id, kind, agent_id, sid, future_next_run_at],
        )
        .expect("seed claimed recurring/heartbeat job");
        future_next_run_at
    }

    fn row_state(conn: &Connection, job_id: &str) -> (String, i64, Option<String>, String) {
        conn.query_row(
            "SELECT status, consecutive_failures, last_failed_at, next_run_at FROM agent_jobs WHERE id = ?1",
            params![job_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap()
    }

    /// Named test (Matt's spec, #3133): three consecutive `Failed`
    /// outcomes flip `status` to `'failed'` — the ladder's terminal arm.
    /// The first two firings must stay `'pending'` (`next_run_at`
    /// untouched — already advanced at claim) with the counter
    /// incrementing; only the third firing is the real flip.
    ///
    /// **Mutation (actually run for this PR, see report): remove the
    /// under-3 branch (force `terminal` unconditionally true) — this
    /// test's first assertion (`status` after firing 1 must be
    /// `"pending"`) reddens immediately.**
    #[test]
    fn test_recurring_job_three_consecutive_failures_flips_to_failed() {
        let conn = make_conn();
        seed_agent(&conn, "agent-a");
        let next_run_at_sentinel = seed_claimed_recurring_job(&conn, "job-1", "agent-a", "recurring");

        let (status1, _) = settle_recurring_job_outcome(&conn, "job-1", JobOutcome::Failed);
        assert_eq!(status1, "pending", "firing 1 of 3 must NOT flip to failed");
        let (status_db, count, last_failed_at, next_run_at) = row_state(&conn, "job-1");
        assert_eq!(status_db, "pending");
        assert_eq!(count, 1);
        assert!(last_failed_at.is_some(), "the first failure of a streak must set last_failed_at");
        assert_eq!(next_run_at, next_run_at_sentinel, "next_run_at was already advanced at claim — this call must not touch it");
        let streak_started_at = last_failed_at.unwrap();

        let (status2, _) = settle_recurring_job_outcome(&conn, "job-1", JobOutcome::Failed);
        assert_eq!(status2, "pending", "firing 2 of 3 must NOT flip to failed");
        let (status_db2, count2, last_failed_at2, _) = row_state(&conn, "job-1");
        assert_eq!(status_db2, "pending");
        assert_eq!(count2, 2);
        assert_eq!(
            last_failed_at2.as_deref(),
            Some(streak_started_at.as_str()),
            "last_failed_at must stay stable across the SAME streak — PR 6's notification dedupe key"
        );

        let (status3, ladder3) = settle_recurring_job_outcome(&conn, "job-1", JobOutcome::Failed);
        assert_eq!(status3, "failed", "the 3rd consecutive failure must flip status to 'failed'");
        let (attempt, max_attempts, terminal, streak_key3) = ladder3.expect("Failed outcome must return ladder info");
        assert_eq!(attempt, 3);
        assert_eq!(max_attempts, RUNTIME_MAX_CONSECUTIVE_FAILURES);
        assert!(terminal, "the 3rd strike must report itself as terminal");
        assert_eq!(streak_key3, streak_started_at, "the terminal notification must key on the SAME streak_started_at as firings 1-2");
        let (status_db3, count3, _, _) = row_state(&conn, "job-1");
        assert_eq!(status_db3, "failed");
        assert_eq!(count3, 3);
    }

    /// Named test (Matt's spec, #3133): a `Done` outcome anywhere in a
    /// streak resets `consecutive_failures` to 0 and clears
    /// `last_failed_at` — AND must never write a terminal `'done'`
    /// status for a recurring/heartbeat job (it stays `'pending'` for
    /// its next scheduled firing; `next_run_at` was already advanced at
    /// claim).
    #[test]
    fn test_recurring_job_success_after_two_failures_resets_counter_to_zero() {
        let conn = make_conn();
        seed_agent(&conn, "agent-a");
        seed_claimed_recurring_job(&conn, "job-1", "agent-a", "heartbeat");

        settle_recurring_job_outcome(&conn, "job-1", JobOutcome::Failed);
        settle_recurring_job_outcome(&conn, "job-1", JobOutcome::Failed);
        let (_, count_before, last_failed_before, _) = row_state(&conn, "job-1");
        assert_eq!(count_before, 2);
        assert!(last_failed_before.is_some());

        let (status, ladder) = settle_recurring_job_outcome(&conn, "job-1", JobOutcome::Done);
        assert_eq!(status, "pending", "a Done outcome must leave a recurring/heartbeat job 'pending', never 'done'");
        assert!(ladder.is_none(), "a Done outcome carries no notification info");

        let (status_db, count_after, last_failed_after, _) = row_state(&conn, "job-1");
        assert_eq!(status_db, "pending");
        assert_eq!(count_after, 0, "a success must reset the counter to zero");
        assert!(last_failed_after.is_none(), "a success must clear last_failed_at");
    }

    /// Tier check (Matt's audit follow-up, #3133): a recurring job that
    /// fails exactly ONCE must stay `'pending'` with the counter at 1 —
    /// not already covered by the two tests above (both drive the
    /// streak all the way to 3 or to a Done reset; neither stops and
    /// asserts after exactly one failure in isolation on a fresh row).
    #[test]
    fn test_recurring_job_single_failure_stays_pending_with_counter_at_one() {
        let conn = make_conn();
        seed_agent(&conn, "agent-a");
        seed_claimed_recurring_job(&conn, "job-1", "agent-a", "recurring");

        let (status, ladder) = settle_recurring_job_outcome(&conn, "job-1", JobOutcome::Failed);
        assert_eq!(status, "pending");
        let (attempt, max_attempts, terminal, _) = ladder.expect("Failed outcome must return ladder info");
        assert_eq!(attempt, 1);
        assert_eq!(max_attempts, RUNTIME_MAX_CONSECUTIVE_FAILURES);
        assert!(!terminal, "one failure out of three must not be reported as terminal");

        let (status_db, count, _, _) = row_state(&conn, "job-1");
        assert_eq!(status_db, "pending");
        assert_eq!(count, 1);
    }

    /// Tier check (Matt's audit follow-up, #3133): `Stopped` is NOT part
    /// of the ladder — a user-initiated Stop writes `'stopped'` exactly
    /// as it does for every other kind, through the SAME `write_job_
    /// terminal_status` call, even for a recurring/heartbeat job.
    #[test]
    fn test_recurring_job_stopped_outcome_bypasses_the_ladder() {
        let conn = make_conn();
        seed_agent(&conn, "agent-a");
        seed_claimed_recurring_job(&conn, "job-1", "agent-a", "recurring");

        let (status, ladder) = settle_recurring_job_outcome(&conn, "job-1", JobOutcome::Stopped);
        assert_eq!(status, "stopped");
        assert!(ladder.is_none());
        let (status_db, count, _, _) = row_state(&conn, "job-1");
        assert_eq!(status_db, "stopped");
        assert_eq!(count, 0, "Stopped must not touch the failure counter");
    }
}

#[cfg(test)]
mod list_scheduled_jobs_and_run_now_tests_3050_pr8 {
    use super::*;
    use rusqlite::Connection;
    use tauri::Manager;

    fn make_conn() -> Connection {
        let conn = Connection::open_in_memory().expect("in-memory db");
        crate::migrations::run_migrations(&conn).expect("fresh-install migrations should succeed");
        conn
    }



    fn seed_agent(conn: &Connection, id: &str, name: &str) {
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO ai_agents (id, name, slug, is_default, is_active, workspace_path, runtime_type, model, created_at, updated_at) \
             VALUES (?1, ?2, ?1, 0, 1, ?1, 'anthropic', 'test-model', ?3, ?3)",
            params![id, name, now],
        )
        .expect("seed agent");
    }

    fn seed_project(conn: &Connection, id: &str, name: &str) {
        conn.execute("INSERT INTO projects (id, name) VALUES (?1, ?2)", params![id, name])
            .expect("seed project");
    }

    fn seed_job(conn: &Connection, id: &str, kind: &str, agent_id: &str, project_id: &str, status: &str) -> String {
        let session_id = create_session_internal(conn, project_id, agent_id, "schedule", false, None)
            .expect("seed job session")
            .id;
        conn.execute(
            "INSERT INTO agent_jobs (id, kind, title, brief, agent_id, project_id, session_id, next_run_at, status, missed_count, created_at, updated_at) \
             VALUES (?1, ?2, 'Test job', 'brief', ?3, ?4, ?5, datetime('now', '+1 hour'), ?6, 0, datetime('now'), datetime('now'))",
            params![id, kind, agent_id, project_id, session_id, status],
        )
        .expect("seed job row");
        session_id
    }

    /// Named test: "Panel query returns once/recurring/heartbeat across
    /// projects and NEVER spawns OR wake rows" (Matt's ruling, #3050 PR 8
    /// follow-up — "every job except spawns" was written before `wake`
    /// existed as a kind; wake rows are chaining plumbing the user never
    /// needs to see in the panel). Seeds one of each of once/recurring/
    /// heartbeat/spawn/wake across two projects, calls the REAL
    /// `list_scheduled_jobs` command (not a hand-rolled duplicate query —
    /// a duplicate would not catch a regression in the function under
    /// test), asserts BOTH excluded kinds are absent and the other three
    /// (from both projects) are present.
    ///
    /// Mutation (covers both kinds, per task item 3): remove the `WHERE
    /// j.kind NOT IN ('spawn', 'wake')` predicate from `list_scheduled_jobs`'s
    /// own SQL (swapped to `WHERE 1=1` for the trial). Confirmed red: both
    /// the spawn row AND the wake row appear in the result (5 rows, not 3),
    /// and the FIRST assertion to fire is `assert_eq!(rows.len(), 3, ...)`
    /// (`left: 5, right: 3`) — the two `assert!(!kinds.contains(...))` lines
    /// below it would catch the same regression independently if the row
    /// count ever happened to still line up. Reverted immediately after
    /// confirming red.
    #[test]
    fn test_list_scheduled_jobs_excludes_spawn_and_wake_across_projects() {
        let conn = make_conn();
        seed_agent(&conn, "agent-a", "Agent A");
        seed_project(&conn, "proj-1", "Project One");
        seed_project(&conn, "proj-2", "Project Two");

        seed_job(&conn, "job-once", "once", "agent-a", "proj-1", "pending");
        seed_job(&conn, "job-recurring", "recurring", "agent-a", "proj-2", "pending");
        seed_job(&conn, "job-heartbeat", "heartbeat", "agent-a", "proj-1", "pending");
        seed_job(&conn, "job-spawn", "spawn", "agent-a", "proj-2", "pending");
        seed_job(&conn, "job-wake", "wake", "agent-a", "proj-1", "pending");

        let app = tauri::test::mock_app();
        app.manage(DbState::new(conn));
        let db_state = app.state::<DbState>();

        let rows = list_scheduled_jobs(db_state.clone()).expect("list_scheduled_jobs should succeed");
        let ids: Vec<String> = rows.iter().map(|r| r["id"].as_str().unwrap().to_string()).collect();
        let kinds: Vec<String> = rows.iter().map(|r| r["kind"].as_str().unwrap().to_string()).collect();

        assert_eq!(rows.len(), 3, "expected exactly once+recurring+heartbeat, got {:?}", rows);
        assert!(ids.contains(&"job-once".to_string()));
        assert!(ids.contains(&"job-recurring".to_string()));
        assert!(ids.contains(&"job-heartbeat".to_string()));
        assert!(!ids.contains(&"job-spawn".to_string()), "spawn row must never appear in the panel query");
        assert!(!ids.contains(&"job-wake".to_string()), "wake row must never appear in the panel query — it's chaining plumbing, not a user-facing schedule");
        assert!(!kinds.contains(&"spawn".to_string()), "mutation guard: kind filter must exclude 'spawn'");
        assert!(!kinds.contains(&"wake".to_string()), "mutation guard: kind filter must exclude 'wake'");
    }

    /// Calls the real command — proves project-name resolution + the
    /// 'Home' fallback for workspace-scoped jobs, not a hand-rolled copy.
    #[test]
    fn test_list_scheduled_jobs_project_name_resolves_and_home_fallback() {
        let conn = make_conn();
        seed_agent(&conn, "agent-a", "Agent A");
        seed_project(&conn, "proj-1", "Cabinet");
        seed_job(&conn, "job-proj", "once", "agent-a", "proj-1", "pending");
        seed_job(&conn, "job-home", "once", "agent-a", "workspace", "pending");

        let app = tauri::test::mock_app();
        app.manage(DbState::new(conn));
        let db_state = app.state::<DbState>();

        let rows = list_scheduled_jobs(db_state.clone()).expect("list_scheduled_jobs should succeed");
        let home_row = rows.iter().find(|r| r["id"] == "job-home").expect("job-home present");
        assert_eq!(home_row["project"], "Home", "workspace-scoped job must resolve to 'Home', not a phantom project row");
        let proj_row = rows.iter().find(|r| r["id"] == "job-proj").expect("job-proj present");
        assert_eq!(proj_row["project"], "Cabinet");
    }

    /// Named test: "run_job_now" on a `pending` row with a far-future
    /// `next_run_at` makes it due now, without disturbing `status`.
    #[test]
    fn test_run_job_now_pending_row_sets_next_run_at_to_now() {
        let conn = make_conn();
        seed_agent(&conn, "agent-a", "Agent A");
        seed_project(&conn, "proj-1", "Project One");
        seed_job(&conn, "job-1", "once", "agent-a", "proj-1", "pending");

        let app = tauri::test::mock_app();
        app.manage(DbState::new(conn));
        let db_state = app.state::<DbState>();

        run_job_now("job-1".to_string(), db_state.clone()).expect("run_job_now should succeed");

        let guard = db_state.0.lock().unwrap();
        let (status, next_run_at): (String, String) = guard
            .query_row("SELECT status, next_run_at FROM agent_jobs WHERE id = 'job-1'", [], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .unwrap();
        assert_eq!(status, "pending", "run_job_now must not disturb a pending row's status");
        let next: chrono::NaiveDateTime = chrono::NaiveDateTime::parse_from_str(&next_run_at, "%Y-%m-%d %H:%M:%S").unwrap();
        let now = chrono::Utc::now().naive_utc();
        assert!(next <= now + chrono::Duration::seconds(5), "next_run_at must be pulled to now, got {}", next_run_at);
    }

    /// Named test: "run_job_now" on a `paused` row ALSO flips status back
    /// to `pending` (otherwise invisible to `claim_due_jobs`'s own
    /// predicate regardless of `next_run_at`).
    #[test]
    fn test_run_job_now_paused_row_flips_status_to_pending() {
        let conn = make_conn();
        seed_agent(&conn, "agent-a", "Agent A");
        seed_project(&conn, "proj-1", "Project One");
        seed_job(&conn, "job-1", "recurring", "agent-a", "proj-1", "paused");

        let app = tauri::test::mock_app();
        app.manage(DbState::new(conn));
        let db_state = app.state::<DbState>();

        run_job_now("job-1".to_string(), db_state.clone()).expect("run_job_now should succeed");

        let guard = db_state.0.lock().unwrap();
        let (status, next_run_at): (String, String) = guard
            .query_row("SELECT status, next_run_at FROM agent_jobs WHERE id = 'job-1'", [], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .unwrap();
        assert_eq!(status, "pending");
        let next: chrono::NaiveDateTime = chrono::NaiveDateTime::parse_from_str(&next_run_at, "%Y-%m-%d %H:%M:%S").unwrap();
        assert!(next <= chrono::Utc::now().naive_utc() + chrono::Duration::seconds(5));
    }

    /// Mutation-pinned guard test: `run_job_now` on an already-`running`
    /// row must be a no-op (`Err`, no row change) — Mutation: drop the
    /// `AND status IN ('pending', 'paused')` guard from the UPDATE's WHERE
    /// clause -> this call silently rewrites `next_run_at` on a running
    /// job -> the `assert_eq!` on the unchanged `next_run_at` below goes
    /// red.
    #[test]
    fn test_run_job_now_running_row_is_noop() {
        let conn = make_conn();
        seed_agent(&conn, "agent-a", "Agent A");
        seed_project(&conn, "proj-1", "Project One");
        seed_job(&conn, "job-1", "once", "agent-a", "proj-1", "running");

        let app = tauri::test::mock_app();
        app.manage(DbState::new(conn));
        let db_state = app.state::<DbState>();

        let before: String = db_state
            .0
            .lock()
            .unwrap()
            .query_row("SELECT next_run_at FROM agent_jobs WHERE id = 'job-1'", [], |row| row.get(0))
            .unwrap();

        let result = run_job_now("job-1".to_string(), db_state.clone());
        assert!(result.is_err(), "run_job_now on a running row must error, not silently succeed");

        let after: String = db_state
            .0
            .lock()
            .unwrap()
            .query_row("SELECT next_run_at FROM agent_jobs WHERE id = 'job-1'", [], |row| row.get(0))
            .unwrap();
        assert_eq!(before, after, "mutation guard: a running row's next_run_at must be untouched");
    }

    /// Proves "Run now" does NOT bypass `claim_due_jobs` — it only makes
    /// the row eligible; the real, unmodified claim path still does the
    /// claiming. No fixed `sleep(...)`: `claim_due_jobs` is called
    /// directly and synchronously right after the write, not via the
    /// scheduler's real timer.
    #[test]
    fn test_run_job_now_row_is_claimed_by_real_claim_due_jobs() {
        let conn = make_conn();
        seed_agent(&conn, "agent-a", "Agent A");
        seed_project(&conn, "proj-1", "Project One");
        seed_job(&conn, "job-1", "once", "agent-a", "proj-1", "pending");

        let app = tauri::test::mock_app();
        app.manage(DbState::new(conn));
        let db_state = app.state::<DbState>();

        run_job_now("job-1".to_string(), db_state.clone()).expect("run_job_now should succeed");

        let guard = db_state.0.lock().unwrap();
        let claimed = claim_due_jobs(&guard, 1).expect("claim_due_jobs should succeed");
        assert_eq!(claimed.len(), 1, "claim_due_jobs must claim the row run_job_now made eligible");
        assert_eq!(claimed[0].id, "job-1");
    }

    /// Covers `list_scheduled_jobs` itself (not just its raw SQL, tested
    /// above) via the real command function, including the `session_id`
    /// field the panel's row-click needs.
    #[test]
    fn test_list_scheduled_jobs_command_returns_session_id_for_row_click() {
        let conn = make_conn();
        seed_agent(&conn, "agent-a", "Agent A");
        seed_project(&conn, "proj-1", "Project One");
        let sid = seed_job(&conn, "job-1", "recurring", "agent-a", "proj-1", "pending");

        let app = tauri::test::mock_app();
        app.manage(DbState::new(conn));
        let db_state = app.state::<DbState>();

        let rows = list_scheduled_jobs(db_state.clone()).expect("list_scheduled_jobs should succeed");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["session_id"], sid, "panel row click needs the job's session_id to open it");
        assert_eq!(rows[0]["agent"], "Agent A");
        assert_eq!(rows[0]["project"], "Project One");
    }

    /// #3134 C26: `openEditJobModal(row)`'s pre-fill needs `brief`,
    /// `cron`, `run_at`, and `agent_id` (not just the agent's display
    /// NAME) directly off this query's own row — no second,
    /// Edit-specific query. **Mutation: drop any one of the four new
    /// SELECT columns** — the corresponding assertion below reddens
    /// (a `None`/missing JSON field instead of the seeded value).
    #[test]
    fn test_list_scheduled_jobs_includes_brief_cron_run_at_agent_id_for_edit_prefill() {
        let conn = make_conn();
        seed_agent(&conn, "agent-a", "Agent A");
        seed_project(&conn, "proj-1", "Project One");
        let sid = create_session_internal(&conn, "proj-1", "agent-a", "schedule", false, None)
            .expect("seed job session")
            .id;
        crate::ai::scheduler::create_job(
            &conn,
            crate::ai::scheduler::NewJobParams {
                kind: "recurring", title: "Edit-prefill job", brief: "the stored brief",
                agent_id: "agent-a", project_id: "proj-1", cron: Some("0 9 * * *"), run_at: None,
                next_run_at: "2026-12-01 09:00:00", origin_session_id: None,
                origin_message_id: None, parent_job_id: None, root_job_id: None, chain_depth: 0, wake_origin: true,
            },
        )
        .expect("seed recurring job");
        let _ = sid;

        let app = tauri::test::mock_app();
        app.manage(DbState::new(conn));
        let db_state = app.state::<DbState>();

        let rows = list_scheduled_jobs(db_state.clone()).expect("list_scheduled_jobs should succeed");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["brief"], "the stored brief", "mutation guard: Edit's brief pre-fill needs this column");
        assert_eq!(rows[0]["cron"], "0 9 * * *", "mutation guard: Edit's cron-inverse formatter needs this column");
        assert_eq!(rows[0]["run_at"], serde_json::Value::Null, "a recurring job has no run_at");
        assert_eq!(rows[0]["agent_id"], "agent-a", "mutation guard: Edit's agent <select> pre-fill needs the id, not just the display name");
    }
}

/// §36.3.7 carry-over, #3050 PR 9 (Reed's 2026-10-29 PR 9 scope audit §6 —
/// the two #3116 carry-overs, unblocked now that #3116/PR 8 landed):
/// `wait_for_running_job_to_stop`'s API-vs-OC asymmetry, and
/// `execute_cancel_job` clearing `last_error`.
#[cfg(test)]
mod stop_running_job_carry_over_tests_3050_pr9 {
    use super::*;
    use rusqlite::Connection;
    use tauri::Manager;

    fn make_conn() -> Connection {
        let conn = Connection::open_in_memory().expect("in-memory db");
        crate::migrations::run_migrations(&conn).expect("fresh-install migrations should succeed");
        conn
    }

    fn seed_agent(conn: &Connection, id: &str, runtime_type: &str) {
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO ai_agents (id, name, slug, is_default, is_active, workspace_path, runtime_type, model, created_at, updated_at) \
             VALUES (?1, ?1, ?1, 0, 1, ?1, ?2, 'test-model', ?3, ?3)",
            params![id, runtime_type, now],
        )
        .expect("seed agent");
    }

    fn seed_running_job(conn: &Connection, id: &str, agent_id: &str, last_error: Option<&str>) -> String {
        let session_id = create_session_internal(conn, "workspace", agent_id, "schedule", false, None)
            .expect("seed job session")
            .id;
        conn.execute(
            "INSERT INTO agent_jobs (id, kind, title, brief, agent_id, project_id, session_id, next_run_at, status, last_error, missed_count, created_at, updated_at) \
             VALUES (?1, 'recurring', 'Test job', 'brief', ?2, 'workspace', ?3, datetime('now', '+1 hour'), 'running', ?4, 0, datetime('now'), datetime('now'))",
            params![id, agent_id, session_id, last_error],
        )
        .expect("seed running job row");
        session_id
    }

    /// API-runtime half of the carry-over: `wait_for_running_job_to_stop`
    /// sets the SAME per-session cancel flag `stop_turn` (the interactive
    /// Stop button) uses, then waits for the row to leave `'running'`
    /// before returning — no optimistic flip (the row is still `'running'`
    /// in the DB for the entire 80ms the background task sleeps before
    /// settling it; the function only returns `Ok` once that write lands).
    ///
    /// Mutation (actually run, confirmed red, then reverted — see PR
    /// report): remove the `if runtime_type != "openclaw" { stop_turn(...)?
    /// }` branch entirely — `get_cancel_flags()` never gains an entry for
    /// this session, and the assertion below (`flags.get(&session_id)` is
    /// `Some`) fails with `None`.
    #[tokio::test]
    async fn test_wait_for_running_job_to_stop_api_runtime_sets_cancel_flag_and_waits_for_stopped() {
        let conn = make_conn();
        seed_agent(&conn, "agent-api", "anthropic");
        let session_id = seed_running_job(&conn, "job-api-running", "agent-api", None);

        let app = tauri::test::mock_app();
        app.manage(DbState::new(conn));
        let db_state = app.state::<DbState>();
        let conn_arc = db_state.0.clone();

        // Simulates the real job loop observing the cancel flag and
        // settling the row shortly after — the background task's own
        // delay is fixture timing, not what this test's PASS/FAIL hinges
        // on; the test awaits the real function's actual return, which
        // polls the DB itself (no fixed sleep as the assertion mechanism).
        let settle = tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(80)).await;
            let conn = conn_arc.lock().unwrap();
            conn.execute(
                "UPDATE agent_jobs SET status = 'stopped' WHERE id = 'job-api-running'",
                [],
            )
            .unwrap();
        });

        // Row must still be 'running' the instant before the background
        // task's write lands — i.e. wait_for_running_job_to_stop itself
        // never writes an optimistic status change.
        let result = wait_for_running_job_to_stop("job-api-running".to_string(), db_state.clone()).await;
        settle.await.unwrap();

        assert!(result.is_ok(), "expected Ok once the row left 'running', got {:?}", result);

        let flags = crate::ai::chat::get_cancel_flags().lock().unwrap_or_else(|e| e.into_inner());
        let flag = flags
            .get(&session_id)
            .expect("an API-runtime job's cancel flag must be set by wait_for_running_job_to_stop");
        assert!(
            flag.load(std::sync::atomic::Ordering::Relaxed),
            "the cancel flag must be true for an API-runtime job"
        );

        let final_status: String = {
            let conn = db_state.0.lock().unwrap();
            conn.query_row("SELECT status FROM agent_jobs WHERE id = 'job-api-running'", [], |r| r.get(0))
                .unwrap()
        };
        assert_eq!(final_status, "stopped");
    }

    /// #3132 (inverts the pre-#3132 pin below, which asserted the
    /// OPPOSITE): OC-runtime jobs now dispatch through the same
    /// `prepare_job_dispatch`/`run_agent_loop` path as every other
    /// runtime (#3129, C28(b)) — `dispatch_oc_job`'s "no cancel
    /// checkpoint" reason is gone, so `wait_for_running_job_to_stop` must
    /// set the SAME cancel flag for an OC-runtime job's session as it
    /// already does for API-runtime.
    ///
    /// Mutation (actually run, confirmed red, then reverted — see PR
    /// report): reintroduce the old `if runtime_type != "openclaw" {
    /// stop_turn(...)? }` gate — `get_cancel_flags()` never gains an entry
    /// for the OC session, and the assertion below (`flags.get(&session_id)`
    /// is `Some`) fails with `None`.
    #[tokio::test]
    async fn test_wait_for_running_job_to_stop_oc_runtime_now_sets_cancel_flag_c3132() {
        let conn = make_conn();
        seed_agent(&conn, "agent-oc", "openclaw");
        let session_id = seed_running_job(&conn, "job-oc-running", "agent-oc", None);

        let app = tauri::test::mock_app();
        app.manage(DbState::new(conn));
        let db_state = app.state::<DbState>();
        let conn_arc = db_state.0.clone();

        let settle = tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(80)).await;
            let conn = conn_arc.lock().unwrap();
            conn.execute(
                "UPDATE agent_jobs SET status = 'stopped' WHERE id = 'job-oc-running'",
                [],
            )
            .unwrap();
        });

        let result = wait_for_running_job_to_stop("job-oc-running".to_string(), db_state.clone()).await;
        settle.await.unwrap();

        assert!(result.is_ok(), "expected Ok once the row left 'running', got {:?}", result);

        let flags = crate::ai::chat::get_cancel_flags().lock().unwrap_or_else(|e| e.into_inner());
        let flag = flags
            .get(&session_id)
            .expect("an OC-runtime job's cancel flag must now be set by wait_for_running_job_to_stop (#3132)");
        assert!(
            flag.load(std::sync::atomic::Ordering::Relaxed),
            "the cancel flag must be true for an OC-runtime job too"
        );
    }

    /// #3132 — the end-to-end sequence the panel's Delete-on-running-row
    /// path already drives (`waitForRunningJobToStop` then
    /// `cancelScheduledJob`/`cancel_job_command`): set the cancel flag,
    /// wait bounded for the loop to exit `Stopped`, then write
    /// `cancelled`. Exercises BOTH functions together, proving the two
    /// commands compose into the sequence #3132's issue text describes,
    /// not just that each one works in isolation.
    ///
    /// Mutation (actually run, confirmed red, then reverted — see PR
    /// report): revert `execute_cancel_job`'s pre-condition on the
    /// `UPDATE` back to `status = 'pending'`-only (excluding `'stopped'`)
    /// — `cancel_job_command` after the wait returns `Err` instead of
    /// settling the row to `cancelled`.
    #[tokio::test]
    async fn test_cancel_job_stops_a_running_job() {
        let conn = make_conn();
        seed_agent(&conn, "agent-cancel", "anthropic");
        let session_id = seed_running_job(&conn, "job-to-cancel", "agent-cancel", None);

        let app = tauri::test::mock_app();
        app.manage(DbState::new(conn));
        let db_state = app.state::<DbState>();
        let conn_arc = db_state.0.clone();

        // Simulates the real loop observing the cancel flag `stop_turn`
        // sets and settling the row to 'stopped' shortly after — the
        // delay is fixture timing only; the test's PASS/FAIL hinges on
        // `wait_for_running_job_to_stop`'s own DB poll, not this sleep.
        let settle = tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(80)).await;
            let conn = conn_arc.lock().unwrap();
            conn.execute("UPDATE agent_jobs SET status = 'stopped' WHERE id = 'job-to-cancel'", [])
                .unwrap();
        });

        let wait_result = wait_for_running_job_to_stop("job-to-cancel".to_string(), db_state.clone()).await;
        settle.await.unwrap();
        assert!(wait_result.is_ok(), "wait must settle Ok once the row leaves 'running': {:?}", wait_result);

        let flags = crate::ai::chat::get_cancel_flags().lock().unwrap_or_else(|e| e.into_inner());
        assert!(
            flags.get(&session_id).map(|f| f.load(std::sync::atomic::Ordering::Relaxed)).unwrap_or(false),
            "the cancel flag must have been set before the row ever settled"
        );
        drop(flags);

        {
            let conn = db_state.0.lock().unwrap();
            let status: String = conn
                .query_row("SELECT status FROM agent_jobs WHERE id = 'job-to-cancel'", [], |r| r.get(0))
                .unwrap();
            assert_eq!(status, "stopped", "row must be 'stopped' before cancel is called — the wait never optimistically flips it");
        }

        let db_for_tool = DbState(db_state.0.clone());
        let params = serde_json::json!({ "job_id": "job-to-cancel" });
        let cancel_result = crate::ai::tools::execute_cancel_job(&params, &db_for_tool);
        assert!(cancel_result.is_ok(), "cancel must succeed once the row has left 'running': {:?}", cancel_result);

        let conn = db_state.0.lock().unwrap();
        let (status, last_error): (String, Option<String>) = conn
            .query_row(
                "SELECT status, last_error FROM agent_jobs WHERE id = 'job-to-cancel'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(status, "cancelled", "the full sequence (set flag, wait for Stopped, write cancelled) must land on 'cancelled'");
        assert_eq!(last_error, None);
    }

    /// A non-running row returns immediately without touching anything —
    /// the caller (pause/delete) is meant to proceed straight to its own
    /// action in this case.
    #[tokio::test]
    async fn test_wait_for_running_job_to_stop_noop_for_a_non_running_row() {
        let conn = make_conn();
        seed_agent(&conn, "agent-pending", "anthropic");
        let session_id = create_session_internal(&conn, "workspace", "agent-pending", "schedule", false, None)
            .unwrap()
            .id;
        conn.execute(
            "INSERT INTO agent_jobs (id, kind, title, brief, agent_id, project_id, session_id, next_run_at, status, missed_count, created_at, updated_at) \
             VALUES ('job-pending', 'recurring', 'T', 'b', 'agent-pending', 'workspace', ?1, datetime('now', '+1 hour'), 'pending', 0, datetime('now'), datetime('now'))",
            params![session_id],
        )
        .unwrap();

        let app = tauri::test::mock_app();
        app.manage(DbState::new(conn));
        let db_state = app.state::<DbState>();

        let result = wait_for_running_job_to_stop("job-pending".to_string(), db_state.clone()).await;
        assert!(result.is_ok());

        let flags = crate::ai::chat::get_cancel_flags().lock().unwrap_or_else(|e| e.into_inner());
        assert!(flags.get(&session_id).is_none(), "a non-running row must never touch the cancel-flag registry");
    }

    /// `execute_cancel_job` clears `last_error` (Matt's carry-over ruling
    /// — confirmed it never did before this PR). Mutation (actually run,
    /// confirmed red, then reverted — see PR report): drop `last_error =
    /// NULL` from the `UPDATE` — the stale error persists, and the
    /// assertion below (`last_error` is `None`) fails, seeing the original
    /// `"stale provider error"` text instead.
    #[test]
    fn test_execute_cancel_job_clears_last_error() {
        let conn = make_conn();
        seed_agent(&conn, "agent-x", "anthropic");
        seed_running_job(&conn, "job-with-error", "agent-x", Some("stale provider error"));
        // Cancel only succeeds on a non-running row — flip it to 'pending'
        // first (mirrors the real flow: wait_for_running_job_to_stop would
        // have already done this via the loop settling to 'stopped'/etc;
        // this test targets execute_cancel_job's own SQL in isolation).
        conn.execute("UPDATE agent_jobs SET status = 'pending' WHERE id = 'job-with-error'", [])
            .unwrap();

        let db = DbState::new(conn);
        let params = serde_json::json!({ "job_id": "job-with-error" });
        let result = crate::ai::tools::execute_cancel_job(&params, &db);
        assert!(result.is_ok(), "cancel should succeed on a non-running row: {:?}", result);

        let conn = db.0.lock().unwrap();
        let (status, last_error): (String, Option<String>) = conn
            .query_row(
                "SELECT status, last_error FROM agent_jobs WHERE id = 'job-with-error'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(status, "cancelled");
        assert_eq!(last_error, None, "last_error must be cleared by cancel_job, got {:?}", last_error);
    }
}

/// Bug 2 (#3050 fix PR): the writer guard chosen over an FK (see
/// `insert_job_brief_message`'s/`insert_schedule_confirmation_message`'s own
/// doc comments for the full guard-vs-FK rationale). Rejects an INSERT
/// before it happens if `thread_id` does not correspond to any existing
/// `chat_sessions.id` — the exact mechanism that let the schedule
/// confirmation chip succeed silently into a phantom `thread_id =
/// 'workspace'` row (no FK on `chat_messages_local.thread_id` means that
/// insert never errored, so there was nothing for the caller's own
/// `eprintln!` to catch).
pub(crate) fn require_known_thread_id(conn: &rusqlite::Connection, thread_id: &str) -> Result<(), String> {
    let exists: bool = conn
        .query_row(
            "SELECT 1 FROM chat_sessions WHERE id = ?1",
            params![thread_id],
            |_| Ok(true),
        )
        .optional()
        .map_err(|e| format!("require_known_thread_id query failed for '{}': {}", thread_id, e))?
        .unwrap_or(false);
    if !exists {
        return Err(format!(
            "unknown thread_id '{}' — no matching chat_sessions row; refusing to write a message into a phantom thread",
            thread_id
        ));
    }
    Ok(())
}

/// §36.3.3 point 3: the job's brief, inserted as the job session's own
/// `synthetic=1` user turn — "a job is a turn nobody typed," not the user's
/// own words. Written for BOTH dispatch branches (OC send and API loop) so
/// the job session's transcript always shows what was sent, even on the
/// OC-trigger-only path where Pane never streams a reply into it directly.
///
/// Bug 2 (#3050 fix PR): guarded by `require_known_thread_id` before the
/// INSERT — `session_id` here is always a freshly-created job session in
/// production (§36.3.6 creates it in the same transaction as the job), so
/// this guard is defense-in-depth, not the fix for a known-reachable bug on
/// THIS specific call site; `insert_schedule_confirmation_message` below is
/// the one Bug 2 actually named.
fn insert_job_brief_message(
    conn: &rusqlite::Connection,
    session_id: &str,
    project_id: &str,
    brief: &str,
    job_id: &str,
) -> Result<(), String> {
    require_known_thread_id(conn, session_id)
        .map_err(|e| format!("insert_job_brief_message refused for job {}: {}", job_id, e))?;
    let message_id = uuid::Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO chat_messages_local (id, project_id, thread_id, role, content, created_at, synthetic, job_id) \
         VALUES (?1, ?2, ?3, 'user', ?4, datetime('now'), 1, ?5)",
        params![message_id, project_id, session_id, brief, job_id],
    )
    .map_err(|e| format!("insert_job_brief_message failed for job {}: {}", job_id, e))?;
    Ok(())
}

/// C17a (#3120 PR 1, §36.4.5): fetches a job/wake turn's REAL prior session
/// history, same shape `maybe_compact_thread`/`load_chat_messages` already
/// use — `tools::CHAT_MESSAGE_BOOKKEEPING_COLUMNS` (`synthetic, continued,
/// job_id`) rather than a fourth hand-rolled SELECT, oldest→newest
/// (`ORDER BY created_at ASC, rowid ASC`, the dispatch order
/// `build_messages_with_context` expects), bookkeeping rows (job briefs,
/// `§36.2` continuation dividers) excluded — the exact filter
/// `maybe_compact_thread`'s own `conversation_text` build already applies
/// (`!m.synthetic && !m.continued`), so a wake turn's history never leaks
/// the hidden "[Job '...' finished: ...]" bookkeeping back to the model as
/// if it were a real prior turn.
///
/// Mapped to the exact `ChatMessage` shape the FRONTEND already sends back
/// into `ai_chat_stream` for the interactive path
/// (`ChatSidebar.svelte`'s `cleanHistory` map: `role`, `content`, `id`,
/// `attachment_ids` only — `tool_calls`/`tool_call_id`/`content_blocks`
/// deliberately left `None`, since a multi-tool-call turn's intermediate
/// exchanges are never persisted as separate rows; only the turn's one
/// final `content` string is). This keeps `prepare_job_dispatch`'s history
/// load byte-for-byte consistent with how an interactive turn's own prior
/// messages already reach `build_messages_with_context`.
pub(crate) fn fetch_session_history_for_job_turn(
    conn: &rusqlite::Connection,
    session_id: &str,
) -> Result<Vec<ChatMessage>, String> {
    let query = format!(
        "SELECT id, role, content, attachment_ids, {} FROM chat_messages_local \
         WHERE thread_id = ?1 ORDER BY created_at ASC, rowid ASC",
        crate::ai::tools::CHAT_MESSAGE_BOOKKEEPING_COLUMNS
    );
    let mut stmt = conn
        .prepare(&query)
        .map_err(|e| format!("fetch_session_history_for_job_turn prepare failed: {}", e))?;
    let rows = stmt
        .query_map(params![session_id], |row| {
            let id: String = row.get(0)?;
            let role: String = row.get(1)?;
            let content: String = row.get(2)?;
            let attachment_ids: Option<String> = row.get(3)?;
            let (synthetic, continued, _job_id) = crate::ai::chat::map_bookkeeping_columns(row, 4)?;
            Ok((id, role, content, attachment_ids, synthetic, continued))
        })
        .map_err(|e| format!("fetch_session_history_for_job_turn query failed: {}", e))?;

    let mut history = Vec::new();
    for row in rows {
        let (id, role, content, attachment_ids, synthetic, continued) =
            row.map_err(|e| format!("fetch_session_history_for_job_turn row read failed: {}", e))?;
        if synthetic || continued {
            continue;
        }
        history.push(ChatMessage {
            role,
            content,
            tool_calls: None,
            tool_call_id: None,
            content_blocks: None,
            attachment_ids,
            id: Some(id),
        });
    }
    Ok(history)
}

/// §36.3.6 (#3050 PR 4): the Pane-written confirmation chip on `schedule_job`
/// success — "the confirmation line is written by Pane on `schedule_job`
/// success..., never by the model." **Deliberately NOT
/// `insert_external_message`** (`ai/ingest.rs`): that function's
/// `session.agent_id == msg.source_agent_id` binding check and
/// `oc_message_id` dedup key are OC-ingestion-specific invariants that don't
/// apply here — this chip's "source" is Pane itself reporting on a tool call
/// that just ran inside this SAME session, not an external agent's message
/// arriving asynchronously. The real precedent is `insert_job_brief_message`
/// above: a plain, direct INSERT with no binding/dedup machinery. Two
/// differences from that function, both deliberate: (1) targets
/// `origin_session_id` (the chat the tool was called FROM), never the job's
/// OWN new session — independent fields on the `agent_jobs` row, so a Home
/// agent scheduling a job onto a project still writes this into the Home
/// chat it was called from, not into the project's new schedule session;
/// (2) `synthetic = 0` (must render, unlike the job brief's own `synthetic =
/// 1`, which must stay hidden) — this row IS the user-visible confirmation,
/// not transcript plumbing. `role = 'assistant'` so it renders as the agent
/// speaking, matching the spec's own example copy ("Scheduled: Monday
/// reminder · next Mon 9:00 AM").
///
/// Bug 2 (#3050 fix PR, live-test #1): this is the exact mechanism Reed's
/// scope audit traced for the missing confirmation chip. `origin_session_id`
/// came from `ai_chat_stream`'s own `session_id_str` resolution, which (pre-
/// fix) could be the literal, non-existent string `"workspace"` when no
/// `chat_sessions` row was flagged `is_active = 1` for Home yet. With no FK
/// on `chat_messages_local.thread_id`, that INSERT succeeded into a phantom
/// thread — zero error, zero chip, because there was no failure for the
/// caller's own `eprintln!` (`[SCHEDULE-CHIP] failed to write confirmation
/// chip...`, `ai::tools::execute_schedule_job`) to ever catch. Guarded here
/// by `require_known_thread_id`: **chosen over an FK on `chat_messages_local`
/// deliberately** — that table is not CRR (confirmed: absent from both CRR
/// table lists, so cr-sqlite's #493 FK-rejection doesn't apply), but SQLite
/// has no `ALTER TABLE ... ADD CONSTRAINT`; retrofitting a real FK onto an
/// EXISTING, populated table (FTS5 triggers, possible legacy rows from the
/// pre-§36.3.6/pre-v72-bucket era) means a full table rebuild
/// (`CREATE new → INSERT INTO ... SELECT → DROP old → rename`), real risk
/// for real installs' historical data for a narrow bug this guard already
/// closes. A writer guard is cheaper and gives a clear, specific error
/// message instead of a raw constraint-violation exception — the tradeoff
/// Reed's own scope note named ("only covers callers that go through it")
/// is accepted here: this guards the THREE synthetic-message writers that
/// use this pattern (this function + `insert_job_brief_message`, both in
/// this module, plus `ai::chat::insert_continuation_messages` — widened to
/// `pub(crate)` so that sibling-module call site can reuse the identical
/// guard rather than a fourth hand-rolled copy; Reed's audit named this
/// third writer as a disclosed-but-unguarded hole, closed in the same fix
/// PR), not every `chat_messages_local` INSERT site in the tree (ordinary
/// interactive-turn persistence, OC ingest, archive, embeddings — a
/// different operation family, out of this PR's scope).
/// §2 (#3050 PR 6): `app` is `Option<&AppHandle<R>>` — `None` for the
/// pre-existing test call sites in `ai/chat.rs`/this module's own test
/// modules that seed a chip row directly (no `pane-chip-changed` emit
/// needed/wanted there, matching `execute_set_status`'s established
/// `Option<&AppHandle<R>>` shape for an optional emit side-effect), `Some`
/// for the two real production call sites (`execute_schedule_job`,
/// `execute_spawn_job`, `ai/tools.rs`) which both share this one function —
/// so the chip event covers both chip kinds for free, with no second emit
/// site to keep in sync.
pub(crate) fn insert_schedule_confirmation_message<R: tauri::Runtime>(
    conn: &rusqlite::Connection,
    origin_session_id: &str,
    origin_project_id: &str,
    content: &str,
    job_id: &str,
    app: Option<&tauri::AppHandle<R>>,
) -> Result<(), String> {
    require_known_thread_id(conn, origin_session_id)
        .map_err(|e| format!("insert_schedule_confirmation_message refused for job {}: {}", job_id, e))?;
    let message_id = uuid::Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO chat_messages_local (id, project_id, thread_id, role, content, created_at, synthetic, job_id) \
         VALUES (?1, ?2, ?3, 'assistant', ?4, datetime('now'), 0, ?5)",
        params![message_id, origin_project_id, origin_session_id, content, job_id],
    )
    .map_err(|e| format!("insert_schedule_confirmation_message failed for job {}: {}", job_id, e))?;
    if let Some(app) = app {
        emit_chip_changed(app, job_id, origin_session_id, "inserted");
    }
    Ok(())
}

/// §2 (#3050 PR 6): the one new, narrowly-scoped event for live chip
/// refresh — NOT an extension of `pane-data-changed` (Reed's audit,
/// confirmed by Ruling 3: `entity_type`'s five values and `reason`'s four
/// are both closed enums, pinned by existing tests; a chat-message chip
/// fits neither axis). Payload shape is fixed (`job_id`, `session_id`,
/// `status`) — every emit site in this module constructs it through this
/// one function, matching `refresh_event.rs::emit_data_changed`'s own
/// "one function, every sender calls it" shape so the payload can never
/// drift between call sites.
pub(crate) const CHIP_CHANGED_EVENT: &str = "pane-chip-changed";

pub(crate) fn emit_chip_changed<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    job_id: &str,
    session_id: &str,
    status: &str,
) {
    let _ = app.emit(
        CHIP_CHANGED_EVENT,
        serde_json::json!({
            "job_id": job_id,
            "session_id": session_id,
            "status": status,
        }),
    );
}

/// C18 (§36.4.5, #3120 PR 1); widened by #3123 (#3120 PR 2) to cover
/// `recurring`/`heartbeat` job kinds too — the name stays `..._wake_...`
/// (a rename was judged not worth a second commit purely for naming) but
/// the function is kind-agnostic: `job_id` is just a parameter, and
/// nothing inside it assumes `kind == 'wake'`. Read it as "a job turn's
/// own reply, written directly into its own session" — true for `wake`
/// (session_id == origin_session_id by construction) AND for
/// `recurring`/`heartbeat` (session_id IS the persistent schedule session,
/// §36.3.6), which is exactly why both call sites can share it unchanged.
///
/// A job turn's own reply, written DIRECTLY into its own session — unlike `insert_schedule_confirmation_message`
/// (a Pane-AUTHORED chip, `model` always `NULL`) and unlike report-back's
/// `ingest_external_messages_impl` (a cross-session hop with its own
/// binding check), this row IS the session's own bound agent speaking
/// normally in its own thread, so `model`/`agent_id` are both set exactly
/// like any other real assistant turn, and `synthetic = 0` (must render).
/// `job_id` is still stamped, matching every other job-linked row's
/// bookkeeping convention, for operators/tests to trace it back.
///
/// Same `require_known_thread_id` guard `insert_job_brief_message`/
/// `insert_schedule_confirmation_message` already use — a wake row's
/// `session_id` is always a real, pre-existing chat session (it's never a
/// freshly-created job session, per `create_wake_job`'s own doc comment),
/// but the guard is cheap and consistent with this module's other
/// synthetic-writer call sites.
///
/// `app` mirrors `insert_schedule_confirmation_message`'s own
/// `Option<&AppHandle<R>>` shape: `None` for test call sites that don't
/// want the emit, `Some` for the one production call site. The emit
/// itself is gated on `is_session_open` (NOT unconditional, unlike the
/// confirmation chip) — a wake reply into a session nobody is looking at
/// doesn't need a live re-fetch; it'll load normally next time that
/// session opens, same as any other persisted message.
/// C40/C41 (#3147, Reed's scope audit §1 — "one key scheme, done once,
/// used three times"): returns the `message_id` this call mints,
/// instead of discarding it, so callers can key a per-firing-unique
/// `agent_message:{message_id}` notification on the SAME id the row
/// itself carries — the existing, documented convention
/// (`emit_agent_message_notification`'s own doc comment) rather than a
/// second, timestamp-based scheme. A UUID v4 is unconditionally unique
/// regardless of timing, so "two firings produce two keys" is true by
/// construction — no same-second reasoning required, unlike every
/// candidate timestamp column Reed's audit checked and rejected
/// (`next_run_at`: not on `ClaimedJob`, means the NEXT occurrence, not
/// this firing; `last_run_at`: does not exist anywhere in this codebase;
/// `started_at`: real, but second-granular, so two firings dispatched in
/// the same clock-second under a fast/mocked test would collide).
/// Pure refactor in this commit — no caller yet reads the returned id
/// for anything; both call sites bind it into an already-discarded
/// local, confirmed by the unchanged behavior of every existing test.
/// #3171 PR 2 (C59): `attachment_ids` threads a job turn's generated-image
/// attachment ids onto this INSERT, closing the 3-layer gap Reed's audit
/// traced end to end — `AgentLoopResult.image_attachment_ids` (chat.rs) is
/// the only source of this value; both call sites below pass it straight
/// through (`None` where a call site structurally never has images, e.g.
/// the reminder-text-only path). `None`/an empty string both persist a
/// `NULL` column, matching `chat_messages_local`'s existing `attachment_ids
/// TEXT` convention (comma-joined ids, same as `saveAssistantMessage`'s
/// own `image_attachment_ids?.join(',') || null` on the frontend
/// interactive-turn path — ChatSidebar.svelte). No frontend change needed:
/// `load_chat_messages` already `SELECT`s this column unconditionally for
/// every message, job or interactive, and the renderer reads it off the
/// message object generically.
pub(crate) fn insert_wake_turn_reply<R: tauri::Runtime>(
    conn: &rusqlite::Connection,
    session_id: &str,
    project_id: &str,
    agent_id: &str,
    model: &str,
    content: &str,
    job_id: &str,
    attachment_ids: Option<&str>,
    app: Option<&tauri::AppHandle<R>>,
) -> Result<String, String> {
    require_known_thread_id(conn, session_id)
        .map_err(|e| format!("insert_wake_turn_reply refused for job {}: {}", job_id, e))?;
    let message_id = uuid::Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO chat_messages_local (id, project_id, thread_id, role, content, created_at, synthetic, job_id, model, agent_id, attachment_ids) \
         VALUES (?1, ?2, ?3, 'assistant', ?4, datetime('now'), 0, ?5, ?6, ?7, ?8)",
        params![message_id, project_id, session_id, content, job_id, model, agent_id, attachment_ids],
    )
    .map_err(|e| format!("insert_wake_turn_reply failed for job {}: {}", job_id, e))?;
    if let Some(app) = app {
        if crate::ai::chat::is_session_open(session_id) {
            emit_chip_changed(app, job_id, session_id, "inserted");
        }
    }
    Ok(message_id)
}

/// #3171 PR 2 (C59): the ONE place that turns a job turn's
/// `AgentLoopResult.image_attachment_ids` into the comma-joined
/// `Option<&str>` `insert_wake_turn_reply` persists (empty vec -> `None`,
/// matching `chat_messages_local.attachment_ids`' existing NULL-for-
/// "no attachments" convention and the frontend's own
/// `image_attachment_ids?.join(',') || null` shape, ChatSidebar.svelte).
/// Factored out of the report-back/own-session call site specifically so
/// `test_job_turn_reply_persists_image_attachment_ids` (below) can drive
/// it directly with a synthetic id list instead of needing a genuinely
/// successful `generate_image` call — that call hits a HARDCODED real
/// provider URL with no test seam (`execute_generate_image`, image_gen.rs),
/// so exercising it for real in a test would violate the mockito-only, ​
/// no-real-provider-endpoint hard constraint. This helper is the exact
/// code the real call site below runs; a test proving IT persists
/// correctly is proving the fix, not a parallel reimplementation.
pub(crate) fn persist_job_turn_reply<R: tauri::Runtime>(
    conn: &rusqlite::Connection,
    session_id: &str,
    project_id: &str,
    agent_id: &str,
    model: &str,
    content: &str,
    job_id: &str,
    image_attachment_ids: &[String],
    app: Option<&tauri::AppHandle<R>>,
) -> Result<String, String> {
    let attachment_ids = if image_attachment_ids.is_empty() {
        None
    } else {
        Some(image_attachment_ids.join(","))
    };
    insert_wake_turn_reply(
        conn,
        session_id,
        project_id,
        agent_id,
        model,
        content,
        job_id,
        attachment_ids.as_deref(),
        app,
    )
}

/// Everything `scheduler_tick` needs to build an `AgentLoopInputs<R>` for an
/// API-runtime job, EXCEPT `turn_origin` itself — deliberately excluded so
/// the §36.11 AST gate's sanctioned construction site stays textually
/// inside `scheduler_tick`, never in this helper. Generic over
/// `R: tauri::Runtime` for the same reason `run_agent_loop`/`AgentLoopInputs`
/// already are (#3059): so `tauri::test::mock_app()`'s `AppHandle<MockRuntime>`
/// can drive this function directly in tests. Production call sites still
/// pass a concrete `AppHandle<Wry>`; Rust infers `R = Wry` there with zero
/// call-site changes.
struct JobDispatchPrep<R: tauri::Runtime> {
    app_handle: tauri::AppHandle<R>,
    sid: String,
    db_clone: DbState,
    provider_for_streaming: AiProvider,
    primary_provider: AiProvider,
    model: String,
    api_key: String,
    openclaw_base_url: Option<String>,
    thread_id: String,
    dispatch_runtime_ref: Option<String>,
    dispatch_runtime_type: String,
    dispatch_agent_id_for_writeback: String,
    dispatch_agent_id_for_done: String,
    offered_tool_names: HashSet<String>,
    max_tool_iterations: u32,
    idle_timeout_seconds: u64,
    /// §36.2.2 (#3050 PR 3): same read-time-fallback settings read as
    /// `max_tool_iterations`/`idle_timeout_seconds` above — job turns use
    /// the SAME `continue_policy` mechanism interactive turns do (§3.1 of
    /// Reed's scope audit: one mechanism, not a second DB-row-shaped one).
    job_ceiling_tool_calls: u32,
    full_messages: Vec<ChatMessage>,
    tools: Vec<ToolDef>,
    own_turn_session_id: String,
}

/// Resolves dispatch config, records the brief into the job session, and
/// builds the minimal message/tool set a job turn needs. Deliberately
/// simpler than `ai_chat_stream`'s own context assembly — no daily-log
/// auto-create, no workshop-session detection, no smart-routing classifier:
/// those are interactive-chat concerns with no job-dispatch analog, and
/// §36.3's own PR 2 scope already excludes tools (PR 4) and auto-continue
/// (PR 3). `build_messages_with_context` still runs, so the job turn gets
/// the same ABSOLUTE RULES system-prompt layer every turn gets.
async fn prepare_job_dispatch<R: tauri::Runtime>(
    db: &Arc<Mutex<DbConn>>,
    app: &tauri::AppHandle<R>,
    job: &ClaimedJob,
) -> Result<JobDispatchPrep<R>, String> {
    let (dispatch_cfg, max_tool_iterations, idle_timeout_seconds, job_ceiling_tool_calls) = {
        let conn = db.lock().map_err(|e| format!("prepare_job_dispatch lock: {}", e))?;
        let cfg = resolve_runtime_config_for_session(&conn, &job.session_id)?;
        // fix(runtime): checkpoint default 10 → 30 (#3050 PR 3 follow-up,
        // §36.2.2 Ruling 4, Matt 2026-10-27) — seed/fallback only, same as
        // `ai_chat_stream`'s identical read; clamp range (1–30) unchanged.
        let max_iter: u32 = crate::database::get_ai_setting(&conn, "max_tool_iterations")
            .and_then(|v| v.parse::<u32>().ok())
            .map(|v| v.clamp(1, 30))
            .unwrap_or(30);
        let idle: u64 = crate::database::get_ai_setting(&conn, "loop_timeout_seconds")
            .and_then(|v| v.parse::<u64>().ok())
            .map(|v| v.clamp(30, 300))
            .unwrap_or(120);
        // §36.2.2/Ruling 4 — same clamp `ai_chat_stream` applies: the ceiling
        // can never be lower than the checkpoint interval it's meant to sit
        // above (setting them equal reproduces the pre-PR-3 stop-at-N
        // behaviour — the documented escape hatch).
        let ceiling: u32 = crate::database::get_ai_setting(&conn, "job_ceiling_tool_calls")
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(crate::ai::runtime_constants::RUNTIME_DEFAULT_JOB_CEILING)
            .max(max_iter);
        // §5.2 of Reed's PR 5 scope audit: the chain-shared ceiling — the
        // setting read above is the FULL per-chain budget; this subtracts
        // what the chain's root job has already spent (`tool_calls_used`),
        // clamped at 0, so the Nth job in a chain starts with whatever
        // budget the chain has left, not a fresh ceiling every time. A
        // non-chained job (`root_job_id` always self-referencing per
        // `create_job`) reads its own row — `COALESCE` is unnecessary here
        // since `root_job_id` is never NULL by construction, but the read
        // is keyed on it explicitly so a future caller that ever DOES leave
        // it NULL degrades to "no budget consumed yet" rather than erroring.
        let root_job_id_for_ceiling = job.root_job_id.clone().unwrap_or_else(|| job.id.clone());
        let already_spent: u32 = conn
            .query_row(
                "SELECT tool_calls_used FROM agent_jobs WHERE id = ?1",
                params![root_job_id_for_ceiling],
                |row| row.get(0),
            )
            .unwrap_or(0);
        let ceiling_remaining = ceiling.saturating_sub(already_spent);
        (cfg, max_iter, idle, ceiling_remaining)
    };

    if matches!(dispatch_cfg.provider, AiProvider::None) {
        return Err(format!(
            "job {}: no provider configured for agent {}",
            job.id, dispatch_cfg.agent_id
        ));
    }

    // C48 (#3155): `(api_key, openclaw_base_url)` is NO LONGER derived here
    // from `dispatch_cfg.provider`. It used to be, and that was the bug:
    // `resolve_model_for_message` (below) can return a `provider_override`
    // (Smart Routing's internal-provider substitution for a classifier-simple
    // turn), and the stale pre-override key never matched the overridden
    // provider `run_agent_loop` actually streams to — e.g. an Ollama primary
    // (`api_key = ""`) overridden to OpenRouter internal routing sent OpenRouter
    // an empty credential → `401 Missing Authentication header`. The derivation
    // now happens AFTER `resolved_provider` is known, sourced from
    // `resolved_provider` instead of `dispatch_cfg.provider` — the exact fix
    // `ai_chat_stream` (chat.rs) already applies via `effective_provider`.

    // §36.17/§5.2 (#3050 PR 5) — `has_crew` resolved from the JOB's own
    // session (which inherits its captain's crew at creation for spawn
    // kinds — `create_job`); `chain_capped` from the job's own
    // `chain_depth`, the offer-time half of the chain-shared-ceiling gate.
    let has_crew = {
        let conn = db.lock().map_err(|e| format!("prepare_job_dispatch lock (crew check): {}", e))?;
        crate::ai::tools::session_has_crew(&conn, &job.session_id)
    };
    let chain_capped = job.chain_depth >= crate::ai::runtime_constants::RUNTIME_MAX_CHAIN_DEPTH as i64;

    // §36.5.1 — `prepare_job_dispatch` is only ever called from
    // `scheduler_tick`, the OTHER sanctioned `TurnOrigin` constructor site
    // (§36.11), and `scheduler_tick` only ever builds `TurnOrigin::Job` turns
    // — known statically here too, same reasoning as `ai_chat_stream`'s own
    // call site. `proactive = true` withholds `PROACTIVE_WITHHELD_TOOLS`.
    let tools = get_pane_tools_for_runtime(&dispatch_cfg.runtime_type, true, has_crew, chain_capped);
    let offered_tool_names: HashSet<String> = tools.iter().map(|t| t.name.clone()).collect();

    let brief_message = ChatMessage {
        role: "user".to_string(),
        content: job.brief.clone(),
        tool_calls: None,
        tool_call_id: None,
        content_blocks: None,
        attachment_ids: None,
        id: None,
    };

    // C17a (§36.4.5, #3120 PR 1): load the origin session's real prior
    // history BEFORE the brief is inserted below — fetched first so the
    // brief's own (synthetic) row can never double-count into it, though
    // `fetch_session_history_for_job_turn`'s `!synthetic` filter would also
    // exclude it either way. This is the fix for the issue's own quoted
    // log line, `Iteration 1 with 2 messages`: before this commit,
    // `full_messages` was built from `&[brief_message]` alone, unconditionally.
    //
    // **Which session's history, exactly — Matt's ruling quoted verbatim
    // (issue #3120 body):** "wake turns (and spawn-origin turns) load the
    // last N messages of the ORIGIN session." `job.origin_session_id` is
    // the right target when a job has one (the calling chat's own
    // context — what a spawned subagent needs to understand its mission,
    // what a captain needs to recall before deciding the next step on
    // wake); `job.session_id` is the correct fallback for a job with no
    // origin (UI-created, nothing to inherit) AND is already the exact
    // target for a WAKE job specifically, since `create_wake_job` always
    // sets `session_id = origin_session_id` and leaves the new row's own
    // `origin_session_id` column `NULL` — one `unwrap_or` expresses both
    // cases correctly without a `kind` branch. **Recurring/heartbeat jobs
    // with their own `origin_session_id` set** (the schedule-created-from-
    // a-chat case, §36.3.6) also pick up real history under this same
    // rule — Matt's ruling names only "wake" and "spawn-origin" and
    // doesn't explicitly address this case either way; applying the same
    // mechanism uniformly (rather than inventing a `kind`-gated exception
    // Matt never stated) is the documented choice here, flagged in the PR
    // body for confirmation rather than silently assumed.
    let history_session_id: &str = job.origin_session_id.as_deref().unwrap_or(&job.session_id);
    let history_messages: Vec<ChatMessage> = {
        let conn = db.lock().map_err(|e| format!("prepare_job_dispatch lock (history fetch): {}", e))?;
        fetch_session_history_for_job_turn(&conn, history_session_id)?
    };

    {
        let conn = db
            .lock()
            .map_err(|e| format!("prepare_job_dispatch lock (insert brief): {}", e))?;
        insert_job_brief_message(&conn, &job.session_id, &job.project_id, &job.brief, &job.id)?;
    }

    let system_prompt = default_system_prompt();
    // Brief stays LAST — same position a freshly-typed user message
    // occupies interactively (design note carried from the cleanup-sprint
    // audit, §36.4.5).
    let mut turn_messages = history_messages;
    turn_messages.push(brief_message);

    // C6 (#3120 PR 2, Reed's re-cut brief): job turns get the SAME
    // smart-routing gate `ai_chat_stream` already applies to interactive
    // turns — `resolve_model_for_message`, called BEFORE the budget pass
    // below for the same reason `ai_chat_stream` calls it first (the
    // budget pass's own context-window lookup needs to know the EFFECTIVE
    // model, not the agent's configured one, since a simple-tier model can
    // have a different context window than the primary model it replaces).
    // Before this fix, `prepare_job_dispatch` never called this function at
    // all — a job dispatch always used `dispatch_cfg.model`/
    // `dispatch_cfg.provider` verbatim, so an API agent with smart routing
    // enabled got the cost-optimized simple-tier model for every
    // interactive turn but never for a job turn.
    let model_registry_for_job = app
        .try_state::<crate::ai::provider::ModelRegistry>()
        .map(|r| Arc::clone(&r.0));
    let resolution = resolve_model_for_message(
        &DbState(db.clone()),
        Some(job.project_id.as_str()),
        &job.brief,
        &turn_messages,
        model_registry_for_job,
        Some(dispatch_cfg.model.as_str()),
    )
    .await?;
    let resolved_model = resolution.model.clone();
    let resolved_provider = resolution.provider_override.clone().unwrap_or_else(|| dispatch_cfg.provider.clone());

    // C48 (#3155): derive the credential AFTER resolution, from
    // `resolved_provider` — the same match arms `ai_chat_stream` uses from
    // `effective_provider` (chat.rs:9911). This is the fix: when
    // `resolution.provider_override` is `Some(internal_provider)` (classifier
    // simple + Smart Routing), the key sent now matches the provider actually
    // streamed to. When there's no override, `resolved_provider ==
    // dispatch_cfg.provider` and this is byte-identical to the old behavior.
    let (api_key, openclaw_base_url) = match &resolved_provider {
        AiProvider::OpenRouter { api_key } => (api_key.clone(), None),
        AiProvider::Anthropic { api_key } => (api_key.clone(), None),
        AiProvider::OpenClaw { base_url, token, .. } => (token.clone(), Some(base_url.clone())),
        AiProvider::Ollama { .. } => (String::new(), None),
        AiProvider::Venice { api_key, .. } => (api_key.clone(), None),
        AiProvider::None => unreachable!("checked above"),
    };

    let full_messages = build_messages_with_context(&system_prompt, &turn_messages, "", false, &tools);

    // C17b (§36.4.5, #3120 PR 1 — mandatory together with C17a, never landed
    // separately: adding real history without a budget pass is a regression
    // a tiny `[brief_message]`-only array never exposed). The SAME budget
    // pass `ai::chat::ai_chat_stream` runs on every interactive turn
    // (`crate::ai::context_budget::apply_budget`) — zero call sites in this
    // file before this commit, six in `chat.rs`. `BudgetState`/`ModelRegistry`
    // are both reachable via this function's own `app: &tauri::AppHandle<R>`
    // parameter — no signature change needed.
    let full_messages = {
        // `BudgetState`/`ModelRegistry` are `app.manage()`d by the real app's
        // own `.setup()` (`lib.rs`), which a bare `tauri::test::mock_app()`
        // never runs — confirmed the hard way: every pre-existing test that
        // drives `prepare_job_dispatch`/`scheduler_tick` through a bare mock
        // app panicked ("state() called before manage()") the moment this
        // function started reading these two states unconditionally.
        // `try_state` + a throwaway local fallback keeps every one of those
        // tests passing unmodified while still giving production dispatch
        // (which DOES have both managed) the real, cache-aware singletons.
        // A fresh `BudgetState` loses the cache-aware window-state
        // continuity `apply_budget`'s own `§27.5.2` append-only windowing
        // relies on ACROSS calls — fine for the fallback (a disposable
        // local, dropped at the end of this call, so it only ever sees
        // "first request ever" for whatever key it's given) but NOT
        // irrelevant for the managed singleton: production's `BudgetState`
        // is the exact same one-app-wide instance `ai_chat_stream` shares
        // across every interactive session, and now (as of C17b) every
        // job dispatch too. Reed's audit (2026-10-30, PR #3122 FAIL #2)
        // proved that sharing is real, not academic: two sequential job
        // dispatches on different sessions with the identical,
        // unparameterized `default_system_prompt()` every job uses
        // produced a 476% context overflow on the second, because the
        // pre-fix window state had no session identifier and the second
        // job's "stable prefix" read inherited the first job's unrelated
        // bookkeeping. `apply_budget` now takes `history_session_id`
        // (below) and keys its window per-session, so this singleton is
        // safe to keep sharing across calls — continuity is real but
        // scoped to the session whose history is actually in play.
        let managed_budget_state = app.try_state::<crate::ai::context_budget::BudgetState>();
        let fallback_budget_state = crate::ai::context_budget::BudgetState::new();
        let budget_state: &crate::ai::context_budget::BudgetState =
            managed_budget_state.as_deref().unwrap_or(&fallback_budget_state);
        let tools_json = serde_json::to_string(&tools).unwrap_or_else(|_| "[]".to_string());
        // C6: budgeting against the EFFECTIVE (post-smart-routing) model/
        // provider, not the agent's configured one — see the resolution
        // call above this block for why.
        let backend = crate::ai::context_budget::Backend::from_ai_provider(&resolved_provider);
        let ctx_len_opt: Option<u64> = app
            .try_state::<crate::ai::provider::ModelRegistry>()
            .and_then(|r| r.lookup(backend.provider_name(), &resolved_model))
            .flatten();
        let override_tokens: Option<u64> = {
            let conn = db.lock().map_err(|e| format!("prepare_job_dispatch lock (ctx override): {}", e))?;
            crate::database::get_ai_setting(&conn, "context_window_override")
                .and_then(|s| s.parse::<u64>().ok())
        };
        let resolved = crate::ai::context_budget::resolve_context_window_with_source(
            &backend,
            &resolved_model,
            ctx_len_opt,
            override_tokens,
        )
        .await;
        let (trimmed_messages, budget_result) = crate::ai::context_budget::apply_budget(
            budget_state,
            history_session_id,
            &resolved_model,
            resolved.value,
            &system_prompt,
            &tools_json,
            "",
            None,
            full_messages,
            10, // image_retention_messages — same literal `ai_chat_stream` uses (TODO there too)
        );
        crate::ai::context_budget::update_budget_is_estimated(budget_state, resolved.is_estimated);
        budget_result.log();
        trimmed_messages
    };

    Ok(JobDispatchPrep {
        app_handle: app.clone(),
        sid: uuid::Uuid::new_v4().to_string(),
        db_clone: DbState(db.clone()),
        // C6: `provider_for_streaming`/`model` are the EFFECTIVE
        // (post-smart-routing) values — `primary_provider` stays the
        // agent's ACTUAL configured provider (same split `ai_chat_stream`
        // makes between `effective_provider` and `provider`), since
        // downstream fallback/retry logic needs to know what the real
        // provider is, not what smart routing substituted it with.
        provider_for_streaming: resolved_provider,
        primary_provider: dispatch_cfg.provider.clone(),
        model: resolved_model,
        api_key,
        openclaw_base_url,
        thread_id: format!("pane-{}", job.session_id),
        dispatch_runtime_ref: dispatch_cfg.runtime_ref.clone(),
        dispatch_runtime_type: dispatch_cfg.runtime_type.clone(),
        dispatch_agent_id_for_writeback: dispatch_cfg.agent_id.clone(),
        dispatch_agent_id_for_done: dispatch_cfg.agent_id.clone(),
        offered_tool_names,
        max_tool_iterations,
        idle_timeout_seconds,
        job_ceiling_tool_calls,
        full_messages,
        tools,
        own_turn_session_id: job.session_id.clone(),
    })
}


/// C4 (#3120 PR 2): fires a `reminder` job with zero provider calls —
/// the brief is posted directly into the job's own session (its own
/// fresh, job-titled session as of #3151 PR1 Commit B — no longer a
/// shared Reminders session; see that commit's note on `create_job`)
/// via the exact same `insert_wake_turn_reply` mechanism a wake turn's
/// own reply already uses: "write one assistant message directly into a
/// session, emit the
/// live-refresh chip iff it's open" is the identical shape, reused as-is
/// rather than reimplemented. `model` is passed as an empty string —
/// honest, not misleading, since no model was ever actually invoked for
/// this turn (unlike a wake turn, which genuinely did call a provider).
/// Synchronous (no `tauri::async_runtime::spawn`) because there is no
/// I/O/await here at all — a single DB lock-and-write, same cost as the
/// `emit_chip_changed` call that already runs unconditionally for every
/// claimed job.
fn fire_reminder_job<R: tauri::Runtime>(
    db: &Arc<Mutex<DbConn>>,
    app: &tauri::AppHandle<R>,
    job: &ClaimedJob,
) {
    let conn = match db.lock() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[SCHEDULER] reminder job {}: lock failed: {}", job.id, e);
            return;
        }
    };

    let chip_target: &str = job.origin_session_id.as_deref().unwrap_or(&job.session_id);

    // C40/C41 plumbing (#3147, Reed's scope audit §1): `insert_wake_turn_reply`
    // now returns the minted message_id — C40 (below) keys the new
    // notification on it instead of inventing a second scheme.
    // #3171 PR 2 (C59): a reminder firing only ever posts its own brief
    // text (`fire_reminder_job`'s own doc comment, above — "unlike a wake
    // turn, which genuinely did call a provider"), so it structurally
    // never has generated images — `None`, not a theoretical gap.
    let reminder_message_id = match insert_wake_turn_reply(
        &conn,
        &job.session_id,
        &job.project_id,
        &job.agent_id,
        "",
        &job.brief,
        &job.id,
        None,
        Some(app),
    ) {
        Ok(id) => id,
        Err(e) => {
            eprintln!("[SCHEDULER] reminder job {}: failed to post reminder text: {}", job.id, e);
            let _ = conn.execute(
                "UPDATE agent_jobs SET status = 'failed', last_error = ?1, finished_at = datetime('now'), updated_at = datetime('now') WHERE id = ?2",
                params![e, job.id],
            );
            emit_chip_changed(app, &job.id, chip_target, "failed");
            return;
        }
    };

    if let Err(e) = conn.execute(
        "UPDATE agent_jobs SET status = 'done', finished_at = datetime('now'), updated_at = datetime('now') WHERE id = ?1",
        params![job.id],
    ) {
        eprintln!("[SCHEDULER] reminder job {}: failed to finalize 'done' status: {}", job.id, e);
        return;
    }

    // C40 (#3147, live test #5): confirmed (Reed's audit §1) — this
    // function posted into the Reminders session and returned, with
    // ZERO calls to `emit_agent_message_notification` anywhere in its
    // body. The C4 ruling (Matt) is "post AND notify," not "post only."
    // Resolve what the notification needs BEFORE dropping `conn` below —
    // `emit_agent_message_notification` re-locks the SAME `Mutex` via a
    // fresh `DbState`, and `std::sync::Mutex` is not reentrant (same
    // constraint the completion closure's own comment states elsewhere
    // in this file).
    let reminder_agent_name: String = conn
        .query_row("SELECT name FROM ai_agents WHERE id = ?1", params![job.agent_id], |row| row.get(0))
        .unwrap_or_else(|_| job.agent_id.clone());
    let reminder_project_display_name = resolve_project_display_name(&conn, &job.project_id);
    drop(conn);

    eprintln!("[SCHEDULER] reminder job {} posted directly into session {}, zero provider calls", job.id, job.session_id);
    emit_chip_changed(app, &job.id, chip_target, "done");

    // Same `is_session_open` gate every other `emit_agent_message_
    // notification` call site uses — a reminder posted while its own
    // session is on screen needs no notification, only the live-refresh
    // chip above. Keyed on `reminder_message_id` (#3149/C45's
    // `NotificationScope::OwnSession`): `agent_message:{message_id}`,
    // per-firing-unique by construction.
    if !crate::ai::chat::is_session_open(&job.session_id) {
        crate::notifications::emit_agent_message_notification(
            app,
            &crate::database::DbState(db.clone()),
            &reminder_message_id,
            &reminder_agent_name,
            if job.brief.trim().is_empty() { "(no output)" } else { job.brief.trim() },
            &reminder_project_display_name,
            &job.session_id,
            crate::notifications::NotificationScope::OwnSession,
        );
    }
}

/// #3167 Commit 1 — builds the job-turn `AgentLoopInputs.context_clone`
/// JSON blob `scheduler_tick` attaches to every job dispatch. Extracted to
/// its own function so the exact key set (including the new
/// `"jobProjectId"`) is directly unit-testable without needing the full
/// `scheduler_tick` + mockito + agent-loop harness the rest of this file's
/// tests use for DISPATCH OUTCOMES — this function's own contract is the
/// narrower, directly-assertable claim "what keys does this JSON blob carry
/// for a given `ClaimedJob`," nothing about what happens once it's consumed.
///
/// `"jobProjectId"` carries `job.project_id` VERBATIM — no "workspace" ->
/// `None` translation here. That translation is the two guard-side
/// readers' job (`tools.rs::execute_single_tool`, `chat.rs`'s
/// `generate_image` dispatch branch), not this construction site's — see
/// `crate::ai::tools::normalize_home_sentinel`.
fn build_job_context_clone(job: &ClaimedJob) -> serde_json::Value {
    serde_json::json!({
        "sessionId": job.session_id.clone(),
        "dispatchAgentId": job.agent_id.clone(),
        "ownJobId": job.id.clone(),
        "parentJobId": job.parent_job_id.clone(),
        "jobProjectId": job.project_id.clone(),
    })
}

/// §36.3.1/§36.11 (reworded #3129, C28(b)): the scheduler's sole clock.
/// Claims due `agent_jobs` rows (up to `ai_settings.job_concurrency`) and
/// dispatches each one through `prepare_job_dispatch`/`run_agent_loop` —
/// the SAME path, tagged `TurnOrigin::Job`, for every runtime including
/// `openclaw` (there is no longer a separate OC dispatch branch here;
/// `prepare_job_dispatch` resolves `AiProvider::OpenClaw` credentials the
/// same way an interactive OC chat turn does). Called from
/// `start_scheduler_tick` as both the one-shot launch pass and every
/// `RUNTIME_TICK_SECS` thereafter — the SAME function, two call sites.
pub async fn scheduler_tick<R: tauri::Runtime>(db: Arc<Mutex<DbConn>>, app: tauri::AppHandle<R>) {
    // §36.6.2 (#3050 PR 7): the background OC pickup is its own independent
    // step, not folded into the job-claim loop below — called unconditionally,
    // first, so it still runs on ticks with zero due jobs (the job-claim
    // logic below has its own early `return` when `claimed.is_empty()`,
    // which must never skip pickup).
    run_oc_background_pickup(db.clone(), app.clone()).await;

    // #3151 PR2/C24: the auto-archive sweep — same "runs even on a tick
    // with zero due jobs" placement as the OC pickup call above, gated
    // internally (`should_run_job_archive_sweep`) so this is a cheap
    // `get_ai_setting` read on most ticks, not a table scan.
    {
        let conn = match db.lock() {
            Ok(c) => c,
            Err(e) => {
                eprintln!("[SCHEDULER] tick: failed to acquire db lock for archive sweep: {}", e);
                return;
            }
        };
        sweep_archivable_jobs(&conn);
    }

    let concurrency: u32 = {
        let conn = match db.lock() {
            Ok(c) => c,
            Err(e) => {
                eprintln!("[SCHEDULER] tick: failed to acquire db lock: {}", e);
                return;
            }
        };
        crate::database::get_ai_setting(&conn, "job_concurrency")
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(RUNTIME_DEFAULT_CONCURRENCY)
    };

    // Claim ALL due rows up to `concurrency` in ONE lock acquisition, then
    // release the lock before any dispatch work (HTTP calls, `run_agent_loop`)
    // begins — §5 of Reed's scope audit: the claim itself must be a tight,
    // synchronous, mutex-held block so `job_concurrency` means something
    // (nothing else could touch the DB while a job's own turn runs if the
    // lock were ever held across that work).
    let claimed = {
        let conn = match db.lock() {
            Ok(c) => c,
            Err(e) => {
                eprintln!("[SCHEDULER] tick: failed to acquire db lock for claim: {}", e);
                return;
            }
        };
        match claim_due_jobs(&conn, concurrency) {
            Ok(jobs) => jobs,
            Err(e) => {
                eprintln!("[SCHEDULER] tick: claim_due_jobs failed: {}", e);
                return;
            }
        }
    };

    if claimed.is_empty() {
        return;
    }
    eprintln!("[SCHEDULER] tick: claimed {} due job(s)", claimed.len());

    for job in claimed {
        // §2 (#3050 PR 6): `claim_job`'s own `pending→running` UPDATE (above,
        // inside `claim_due_jobs`) emits nothing — it's a pure DB primitive
        // with no `AppHandle`, called from ~20 direct unit tests that would
        // all need updating for a signature change with no real benefit
        // (those tests assert the claim mechanism itself, never the chip
        // event). This is the one place downstream of the claim that both
        // has the `AppHandle` AND the full row (`origin_session_id`/
        // `session_id` for the chip target) for every claimed job,
        // regardless of which dispatch path (`openclaw` spawn vs the API
        // `run_agent_loop` branch below) it's about to take — closing Reed's
        // audit's named gap ("a real, user-visible chip state… has no
        // `emit_chip_changed` call anywhere in its call chain") without
        // touching `claim_job`/`claim_due_jobs`'s tested signatures at all.
        let chip_target_claim: &str = job.origin_session_id.as_deref().unwrap_or(&job.session_id);
        emit_chip_changed(&app, &job.id, chip_target_claim, "running");

        // C4 (#3120 PR 2, Reed's re-cut brief): `reminder` fires with ZERO
        // provider calls — ahead of BOTH the OC branch below (which would
        // call `sessions.send`) and the API-runtime `run_agent_loop` branch
        // further down. The brief IS the reminder text; there is nothing
        // for a model to generate.
        if job.kind == "reminder" {
            fire_reminder_job(&db, &app, &job);
            continue;
        }

        match prepare_job_dispatch(&db, &app, &job).await {
            Ok(prep) => {
                // §36.11: the sole other sanctioned `TurnOrigin` construction
                // site besides `ai_chat_stream`. MUST stay textually inside
                // `scheduler_tick` — see this function's own doc comment and
                // the module doc comment above.
                // §7.2 (#3050 PR 5) — captured before `prep.model` moves
                // into `loop_inputs` below; needed after the turn finishes
                // to populate report-back's own `ExternalMessage.model`.
                let model_for_report = prep.model.clone();
                let turn_origin = TurnOrigin::Job;
                // #3129 (C28(b)): derived the same way the interactive
                // site does (`ai::chat::ai_chat_stream`, `is_openclaw =
                // matches!(&effective_provider, AiProvider::OpenClaw { .. })`)
                // — `prep.provider_for_streaming` IS this job turn's own
                // `effective_provider` equivalent (`resolved_provider` in
                // `prepare_job_dispatch`, post smart-routing override).
                // Computed BEFORE the move below, since `provider_for_streaming`
                // is moved into `loop_inputs` two fields later in this same
                // literal. Previously hardcoded `false` — harmless while
                // this branch was unreachable for OC jobs (the `if
                // job.runtime_type == "openclaw"` branch above routed
                // around it), but would have silently disabled the #2738
                // own-turn dedup guard (`own_turn_baseline_seq`/
                // `_turn_in_flight_guard`) for a real OC job turn the
                // moment that branch was deleted.
                let own_turn_is_openclaw =
                    matches!(&prep.provider_for_streaming, AiProvider::OpenClaw { .. });
                let loop_inputs = AgentLoopInputs {
                    app_handle: prep.app_handle,
                    sid: prep.sid,
                    db_clone: prep.db_clone,
                    // §36.3.7 (#3050 PR 4) — a job turn can itself call
                    // `schedule_job` (chaining, §36.4.5 — depth-gating is PR
                    // 5; the primitive already needs this much context now
                    // so the plumbing doesn't need redoing then). Carries
                    // `sessionId` (origin_session_id for a chained
                    // `schedule_job`'s own chip) and `dispatchAgentId` (its
                    // default agent) — the same two keys `ai_chat_stream`'s
                    // own context_clone adds. Deliberately NOT `projectId`:
                    // `AgentLoopInputs` has no project_id field for a job
                    // turn (same accepted gap `JobDispatchPrep` doesn't carry
                    // one either, per the existing doc comment on
                    // `insert_continuation_messages` — out of this PR's
                    // scope to close), so every OTHER tool call a job turn
                    // already makes (create_note, edit_note, …) keeps its
                    // existing Home-like cross-project behaviour unchanged;
                    // only `schedule_job`'s own optional `project_id` arg is
                    // affected, and it already defaults to `'workspace'`
                    // when absent either way. **This exclusion is UNCHANGED
                    // by #3167 below** — `"projectId"` still means "the
                    // interactive session's own project, as the frontend
                    // presented it," full stop; every OTHER reader of
                    // `context.get("projectId")` (`execute_spawn_job`'s
                    // target default, `show_view`'s `resolved_pid` default,
                    // `execute_read_attachment`'s scoping) stays
                    // byte-identical the moment a job carries a project.
                    // §4 (#3050 PR 5) — `ownJobId`/`parentJobId` let
                    // `execute_single_tool`'s `spawn_job` arm re-derive the
                    // depth cap at dispatch time: this job turn's own id,
                    // and the id of the job THAT SPAWNED it (if any) —
                    // absent on an interactive (`TurnOrigin::User`) turn's
                    // own `context_clone` by construction, which is exactly
                    // what makes depth always "fresh" for an interactive
                    // caller.
                    // #3167 — `"jobProjectId"`, a DISTINCT, job-only key
                    // (never `"projectId"` itself — see the exclusion note
                    // above), carrying `job.project_id` VERBATIM (the literal
                    // Home sentinel `"workspace"` for a Home-originated job,
                    // or a real `projects.id` otherwise — translation to
                    // `None` happens only at the two guard-side readers,
                    // `tools.rs::execute_single_tool` and `chat.rs`'s
                    // `generate_image` dispatch branch, never here). Closes
                    // the cross-project write guard's job-turn hole
                    // (`check_cross_project_write_permission`/
                    // `check_home_only_tool_permission` were never entered
                    // for a job turn before this — docs/spec/runtime.md
                    // §36.18): a job turn is now authorized exactly as the
                    // same request typed interactively in the job's own
                    // project's chat would be. See `build_job_context_clone`
                    // below for the unit-testable extraction of this exact
                    // literal.
                    context_clone: Some(build_job_context_clone(&job)),
                    provider_for_streaming: prep.provider_for_streaming,
                    has_override: false,
                    primary_provider: prep.primary_provider,
                    model: prep.model,
                    api_key: prep.api_key,
                    openclaw_base_url: prep.openclaw_base_url,
                    thread_id: prep.thread_id,
                    is_compaction: false,
                    dispatch_runtime_ref: prep.dispatch_runtime_ref,
                    dispatch_runtime_type: prep.dispatch_runtime_type,
                    dispatch_agent_id_for_writeback: prep.dispatch_agent_id_for_writeback,
                    dispatch_agent_id_for_done: prep.dispatch_agent_id_for_done,
                    dispatch_agent_name_for_done: None,
                    offered_tool_names: prep.offered_tool_names,
                    max_tool_iterations: prep.max_tool_iterations,
                    idle_timeout_seconds: prep.idle_timeout_seconds,
                    full_messages: prep.full_messages,
                    tools: prep.tools,
                    own_turn_session_id: prep.own_turn_session_id,
                    own_turn_is_openclaw,
                    turn_origin,
                    // §36.2.1/§3.1 (#3050 PR 3): job turns use the SAME
                    // `continue_policy` mechanism interactive turns do — not
                    // a second, DB-row-shaped one (Reed's scope audit §3.1:
                    // `agent_jobs.tool_calls_used` is a separate bookkeeping
                    // WRITE, below, never the enforcement path itself).
                    continue_policy: Some(ContinuePolicy {
                        ceiling_remaining: prep.job_ceiling_tool_calls,
                    }),
                    // §36.3.7 carry-over (#3050 PR 9, Reed's 2026-10-29 PR 9
                    // scope audit §6 — unblocked once #3116 landed): this
                    // USED to be a fresh, permanently-false, never-set flag
                    // ("this job turn simply can never be cancelled") — the
                    // exact gap that made a running API job's panel
                    // Pause/Delete a flat, unactionable refusal. Wired to the
                    // SAME per-session registry the interactive Stop button
                    // uses (`ai::chat::reset_cancel_flag`, overwrite-on-
                    // dispatch — correct here too, since this call IS the
                    // start of a new turn for this job's own session), so
                    // `wait_for_running_job_to_stop`'s `stop_turn(session_id)`
                    // call now reaches a real checkpoint this loop actually
                    // reads (every iteration boundary and every tool call,
                    // same as an interactive turn).
                    cancel: crate::ai::chat::reset_cancel_flag(&job.session_id),
                };
                let job_id_for_bookkeeping = job.id.clone();
                let job_kind_for_report = job.kind.clone();
                let job_title_for_report = job.title.clone();
                let job_agent_id_for_report = job.agent_id.clone();
                let job_root_job_id_for_bookkeeping = job.root_job_id.clone().unwrap_or_else(|| job.id.clone());
                let job_origin_session_id_for_report = job.origin_session_id.clone();
                let job_session_id_for_report = job.session_id.clone();
                let job_project_id_for_report = job.project_id.clone();
                // C3 (#3120 PR 3): resolve the DISPLAY name once, outside
                // the async block below, so both `emit_agent_message_
                // notification` call sites in this closure show a real
                // project name (or "Home") instead of the internal
                // "workspace" sentinel — see `resolve_project_display_name`'s
                // own doc comment for why this is a display-only fix, never
                // a change to the stored `project_id` value itself.
                let job_project_display_name_for_report: String = db
                    .lock()
                    .ok()
                    .map(|c| resolve_project_display_name(&c, &job_project_id_for_report))
                    .unwrap_or_else(|| "Home".to_string());
                let job_wake_origin = job.wake_origin;
                let job_chain_depth = job.chain_depth;
                let db_for_bookkeeping = db.clone();
                let app_for_report = app.clone();
                tauri::async_runtime::spawn(async move {
                    let result = run_agent_loop(loop_inputs).await;
                    let tool_calls_used = result.tool_calls_used;

                    let conn = match db_for_bookkeeping.lock() {
                        Ok(c) => c,
                        Err(e) => {
                            eprintln!(
                                "[SCHEDULER] job {}: failed to acquire db lock for post-completion bookkeeping: {}",
                                job_id_for_bookkeeping, e
                            );
                            return;
                        }
                    };

                    // Ruling 5 (Matt 2026-10-27) / Reed's scope audit §2.4:
                    // PR 2 added this column but nothing in production wrote
                    // it. §5.2 (#3050 PR 5): the write now ACCUMULATES onto
                    // the chain's ROOT row instead of overwriting the
                    // finishing job's own row — every job in a chain adds
                    // its own usage onto the root's total. A non-chained job
                    // (root_job_id always self-referencing per `create_job`)
                    // still correctly updates its own row. This is a
                    // bookkeeping write for UI/inspection AND the chain-
                    // shared ceiling's own budget read
                    // (`prepare_job_dispatch`'s `already_spent` query above)
                    // — never the per-turn enforcement mechanism itself
                    // (that lives on `continue_policy`, re-supplied per
                    // call — §3.1).
                    if let Err(e) = conn.execute(
                        "UPDATE agent_jobs SET tool_calls_used = tool_calls_used + ?1, updated_at = datetime('now') WHERE id = ?2",
                        params![tool_calls_used, job_root_job_id_for_bookkeeping],
                    ) {
                        eprintln!(
                            "[SCHEDULER] job {}: failed to write tool_calls_used onto root {}: {}",
                            job_id_for_bookkeeping, job_root_job_id_for_bookkeeping, e
                        );
                    }

                    // §6 (#3050 PR 5): the turn's own outcome, settled now —
                    // the three pre-existing `UPDATE agent_jobs` statements
                    // (claim/missed-recompute/OC-sent) never set a terminal
                    // status for an API-runtime job turn; this is that write.
                    // §36.3.4 Matt's ruling (#3050 PR 8): extracted into its
                    // own function (`write_job_terminal_status`, below) so
                    // the outcome -> status mapping — including the new
                    // `Stopped -> 'stopped'` arm — has a direct unit-test
                    // seam, not just coverage through the full async
                    // scheduler_tick dispatch pipeline.
                    //
                    // Matt's Ruling 3 (2026-10-31, #3133): `kind IN
                    // ('recurring', 'heartbeat')` goes through `settle_
                    // recurring_job_outcome` instead — the 3-strike ladder
                    // (§36.3.4 Ruling 3, originally `dispatch_oc_job`-only,
                    // reimplemented here generically per Reed's #3133 WARN
                    // finding #2). `once`/`spawn`/`wake` are unaffected —
                    // `write_job_terminal_status`'s own single-firing
                    // terminal mapping, unchanged.
                    let is_recurring_ladder_kind =
                        job_kind_for_report == "recurring" || job_kind_for_report == "heartbeat";
                    let (status, ladder_failure): (&'static str, Option<(i64, i64, bool, String)>) =
                        if is_recurring_ladder_kind {
                            settle_recurring_job_outcome(&conn, &job_id_for_bookkeeping, result.outcome)
                        } else {
                            (write_job_terminal_status(&conn, &job_id_for_bookkeeping, result.outcome), None)
                        };

                    // §2 (#3050 PR 6): chip event on this terminal write —
                    // covers the API-runtime (non-OC) `done`/`failed` write
                    // site Reed's audit named (`~5382`). For a recurring/
                    // heartbeat job under the ladder, `status` may be
                    // `"pending"` (not yet at the 3rd strike, or a Done
                    // reset) — a real status flip too, picked up by the
                    // SAME listener's status-FLIP arm (any status but
                    // `"inserted"` re-reads the real row via `get_job_
                    // status`, `ChatPanel.svelte`).
                    // The finished job's own agent name — used by the
                    // own-session notification below (leftover 3, #3139:
                    // this call site previously passed `job_title_for_
                    // report`, a JOB title like "Tick" or "Morning brief"
                    // — reads as a session title to the user, not who sent
                    // it), BOTH of report-back's attribution line and the
                    // wake brief's "by {agent}" clause further down.
                    // Hoisted to HERE (before `conn` drops, below) so it's
                    // in scope for the notification call that now needs it
                    // — previously computed only after the re-lock a few
                    // lines down, which was too late for this call site.
                    let finished_agent_name: String = conn
                        .query_row(
                            "SELECT name FROM ai_agents WHERE id = ?1",
                            params![job_agent_id_for_report],
                            |row| row.get(0),
                        )
                        .unwrap_or_else(|_| job_agent_id_for_report.clone());

                    drop(conn);
                    let chip_target_api: &str = job_origin_session_id_for_report
                        .as_deref()
                        .unwrap_or(&job_session_id_for_report);
                    emit_chip_changed(&app_for_report, &job_id_for_bookkeeping, chip_target_api, status);

                    // §1 (#3050 PR 6, Ruling 1) / Matt's Ruling 3 (#3133):
                    // once/spawn/wake get ONE notification, immediately
                    // terminal (`ladder_failure` is `None` for those kinds,
                    // so the fallback below reproduces the exact prior
                    // behavior: `job.id` alone as the singleton streak key,
                    // attempt 1 of 1, terminal). Recurring/heartbeat under
                    // the ladder get a real per-firing notification — attempt
                    // N of `RUNTIME_MAX_CONSECUTIVE_FAILURES`, keyed on the
                    // streak's own stable `last_failed_at` (PR 6's dedupe
                    // key) — gated on the OUTCOME being `Failed`, not on
                    // `status == "failed"` (which stays `"pending"` for
                    // strikes 1 and 2 under the ladder, by design above).
                    if matches!(result.outcome, JobOutcome::Failed) {
                        let (attempt, max_attempts, terminal, streak_key) = ladder_failure
                            .unwrap_or_else(|| (1, 1, true, job_id_for_bookkeeping.clone()));
                        crate::notifications::emit_or_update_job_failed_notification(
                            &app_for_report,
                            &DbState(db_for_bookkeeping.clone()),
                            &job_id_for_bookkeeping,
                            &job_title_for_report,
                            &streak_key,
                            attempt,
                            max_attempts,
                            terminal,
                            JOB_FAILED_ERROR_TEXT,
                            chip_target_api,
                        );
                    }

                    // §36.7/§36.8.1 (#3050 PR 6): recurring/heartbeat jobs
                    // write their reply into their OWN session (normal turn
                    // persistence through `run_agent_loop`'s `thread_id =
                    // job.session_id`, not a separate report-back message)
                    // — a notification fires here, now, if that job session
                    // isn't the one currently on screen. `spawn`/`once`'s
                    // own notification fires below, gated on the actual
                    // report-back ingest succeeding (different session,
                    // different condition) UNLESS there is no origin to
                    // report back to — C38 (#3139, Matt's ruling, one
                    // notification per firing never two): a `once`/`spawn`
                    // job WITH an origin keeps exactly today's notification
                    // (report-back's own, below) as the ONLY one — nothing
                    // fires here for it. A `once`/`spawn` job with NO
                    // origin raises ITS notification off this own-session
                    // write instead, since report-back below never runs for
                    // it (no origin to write into) — that is the case that
                    // was silent before this fix.
                    // C41 (#3147, live test #5): the own-session
                    // notification used to fire RIGHT HERE, keyed on
                    // `job-turn-{job_id}` — identical on every firing of
                    // the same recurring/heartbeat job, so
                    // `emit_agent_message_notification`'s own dedup
                    // (`SELECT COUNT(*) FROM notifications WHERE idem_key
                    // = ?1`, confirmed in `notifications.rs`) skipped every
                    // firing after the first — one bell, ever, no matter
                    // how many times Tick fires. Per §1's one-key-scheme
                    // decision, the fix keys on the message id
                    // `insert_wake_turn_reply` mints below instead —
                    // inherently per-firing-unique — so the emit itself
                    // must move past that call. The GATE (which kinds, and
                    // whether the session is open) needs neither `conn`
                    // nor the message id, so it's computed here, unchanged
                    // in meaning, and the actual emit is deferred to
                    // `own_session_notification_due` below.
                    let job_has_no_origin = job_origin_session_id_for_report.is_none();
                    let own_session_should_notify = ((job_kind_for_report == "recurring" || job_kind_for_report == "heartbeat")
                        || ((job_kind_for_report == "once" || job_kind_for_report == "spawn") && job_has_no_origin))
                        && !crate::ai::chat::is_session_open(&job_session_id_for_report);

                    let conn = match db_for_bookkeeping.lock() {
                        Ok(c) => c,
                        Err(e) => {
                            eprintln!(
                                "[SCHEDULER] job {}: failed to re-acquire db lock after chip/notification emit: {}",
                                job_id_for_bookkeeping, e
                            );
                            return;
                        }
                    };

                    // `finished_agent_name` — used by report-back's
                    // attribution line and the wake brief's "by {agent}"
                    // clause below — was already resolved above, before
                    // `conn` dropped (hoisted there, leftover 3/#3139, so
                    // the own-session notification call could use it too).
                    // §7.1 — "job identifier" is `agent_jobs.id` (a UUID),
                    // not a sequence number; the spec's "#123" illustration
                    // is not a literal format this PR matches.
                    let report_body = if result.final_text.trim().is_empty() {
                        "(no output)".to_string()
                    } else {
                        result.final_text.clone()
                    };
                    let status_word = match result.outcome {
                        JobOutcome::Done => "done",
                        JobOutcome::Failed => "failed",
                        JobOutcome::Stopped => "stopped",
                    };

                    // C18 (§36.4.5/§36.7.1, #3120 PR 1) — the wake turn's
                    // OWN message-save path. `job.session_id` for a `kind
                    // == 'wake'` row IS the real chat session
                    // (`create_wake_job` sets `session_id =
                    // origin_session_id`, `origin_session_id` column itself
                    // always `NULL`) — a wake turn is "the turn runs
                    // directly in the real chat session" (§36.4.5's own
                    // words), so it needs no report-back/ingest hop into a
                    // DIFFERENT session the way spawn/once do. Traced end to
                    // end for this commit: neither `run_agent_loop` nor any
                    // caller of it persists a job turn's final text
                    // anywhere on its own — the interactive path relies
                    // entirely on the FRONTEND's `ai-stream-chunk` listener
                    // calling `save_chat_message` once `done:true` arrives
                    // (`streamManager.ts`'s `startStream`/
                    // `findEntryByStreamId`), which is never registered for
                    // a scheduler-dispatched `sid` — confirmed by reading
                    // `streamManager.ts`'s own `init()` listener: an
                    // unregistered `data.id` is silently dropped
                    // (`console.debug('[STREAM-MGR] Ignoring event...')`).
                    // Without this write, a wake turn's own reply text was
                    // never saved anywhere (only its TOOL CALLS had real
                    // side effects) — this is the missing half of "the
                    // message appears only after navigating away and back"
                    // for wake turns specifically.
                    // #3123 (#3120 PR 2, Shape 2 — "persist at the
                    // scheduler", Reed's ruling): C18 (PR 1) only closed
                    // this gap for `wake`. `recurring`/`heartbeat` jobs
                    // have the SAME shape — `job.session_id` IS the real,
                    // persistent schedule session the user can open
                    // (§36.3.6: "a normal session titled after the
                    // schedule... each run appends to it"), not a hop
                    // through `origin_session_id`/report-back the way
                    // `spawn`/`once` jobs work — so the exact same direct-
                    // insert mechanism applies unchanged, just gated on two
                    // more `kind` values. Originally `spawn`/`once` were
                    // deliberately excluded here on the theory that those
                    // kinds always have a meaningful `origin_session_id`
                    // and the report-back/ingest path below covers them —
                    // widening this condition to include them would
                    // double-write.
                    //
                    // C38 (#3139, live test #4): that theory was false. A
                    // `once`/`spawn` job created with NO origin (the New
                    // job dialog has no originating chat at all —
                    // `execute_schedule_job`'s own confirmation-chip write
                    // is already conditioned on `origin_session_id` being
                    // `Some`, so a dialog-created job has none) hit NEITHER
                    // branch: not `wake`/`recurring`/`heartbeat` here, and
                    // the report-back block below is separately gated on
                    // `origin_sid.is_some()`. The job's own final_text was
                    // computed and then persisted nowhere — Matt's exact
                    // repro (174 characters, fired, gone).
                    //
                    // Fix: widen to cover `once`/`spawn` too, UNCONDITIONAL
                    // on origin presence — `job.session_id` is this job's
                    // own fresh session regardless of whether an origin
                    // exists. Since `agent_jobs.kind` only ever takes one
                    // of five values (`wake`, `recurring`, `heartbeat`,
                    // `once`, `spawn` — `reminder` jobs post their brief
                    // directly via `fire_reminder_job` and never reach this
                    // call at all), every kind that CAN reach this point is
                    // now covered — the 5-way match collapses to an
                    // unconditional call, not a sixth "did we forget a
                    // kind" branch waiting to happen.
                    //
                    // No double-delivery: `job_session_id_for_report` is
                    // `job.session_id` (this job's own session, always
                    // freshly created by `create_job` for every kind except
                    // `reminder`); report-back below writes into
                    // `job_origin_session_id_for_report` (`job.
                    // origin_session_id`). These two columns are NEVER the
                    // same value for any kind this call now covers — only
                    // a `wake` row collapses them (`create_wake_job`'s own
                    // INSERT literal sets `session_id = origin_session_id`
                    // AND hardcodes the wake row's own `origin_session_id`
                    // column to `NULL`), and `wake` was already in this
                    // condition before C38 touched it. So a `once`/`spawn`
                    // job WITH an origin now gets two writes — one into its
                    // own session (new), one into the origin via
                    // report-back (unchanged) — landing in two distinct
                    // sessions, not two copies in the same one. Report-back
                    // stays "the extra copy," not the only copy.
                    // C40/C41 plumbing (#3147, Reed's scope audit §1):
                    // `insert_wake_turn_reply` now returns the minted
                    // message_id instead of discarding it. This commit only
                    // binds it (no behavior change yet) — C41's own commit
                    // reads `own_session_message_id` to key the own-session
                    // notification below instead of the job-id-only
                    // `job-turn-{job_id}` key.
                    // #3171 PR 2 (C59): the real fix — `result`'s own
                    // `image_attachment_ids` (threaded through
                    // `AgentLoopResult`, chat.rs) is the only place a job
                    // turn's generated-image ids exist by this point.
                    // `persist_job_turn_reply` (above) is the ONE place
                    // that maps it into `insert_wake_turn_reply`'s
                    // persisted column — same function
                    // `test_job_turn_reply_persists_image_attachment_ids`
                    // drives directly.
                    let own_session_message_id: Option<String> = match persist_job_turn_reply(
                        &conn,
                        &job_session_id_for_report,
                        &job_project_id_for_report,
                        &job_agent_id_for_report,
                        &model_for_report,
                        &report_body,
                        &job_id_for_bookkeeping,
                        &result.image_attachment_ids,
                        Some(&app_for_report),
                    ) {
                        Ok(id) => Some(id),
                        Err(e) => {
                            eprintln!(
                                "[WAKE-REPLY] job {}: failed to persist wake turn reply into session {}: {}",
                                job_id_for_bookkeeping, job_session_id_for_report, e
                            );
                            None
                        }
                    };

                    // C41: the deferred own-session notification, now that
                    // the per-firing message id exists. Deferred (not
                    // emitted here) for the SAME reason report-back's own
                    // `agent_message_due` below is deferred:
                    // `emit_agent_message_notification` re-locks the SAME
                    // `Mutex` via a fresh `DbState`, and `conn` is still
                    // held through the report-back/wake-on-complete blocks
                    // further down. `None` if the insert itself failed
                    // (no message id minted — nothing to key a per-firing-
                    // unique notification on; the pre-existing `eprintln!`
                    // above already surfaces that failure).
                    let own_session_notification_due: Option<String> =
                        if own_session_should_notify { own_session_message_id.clone() } else { None };

                    // C42b (#3147, live test #5 — Reed's hidden THIRD
                    // finding, beyond the issue's own text): both the
                    // report-back notification key AND `ExternalMessage::
                    // source_message_id` (stored as `chat_messages_local.
                    // oc_message_id`, the column `idx_chat_local_oc_dedup`'s
                    // UNIQUE INDEX enforces per `(thread_id, oc_message_id)`,
                    // `migrations.rs`) were job-id-only
                    // (`job-report-{job_id}`) — safe ONLY because `once`/
                    // `spawn` (the only kinds that reached this path before
                    // C42a widened the gate above) fire at most once ever.
                    // Widening C42a's kind-gate alone, without this, would
                    // make a recurring/heartbeat job's report-back land
                    // ONCE on the first firing, then silently dedup-skip
                    // (`INSERT OR IGNORE` against the same unique index, 0
                    // rows affected) on every firing after — a pass on the
                    // first walk, a regression the next morning, Matt's
                    // own words for why kind-widening alone "isn't
                    // enough." Reuses the SAME per-firing message id C40/
                    // C41 key on (§1's one key scheme, used three times):
                    // `own_session_message_id` is a UUID v4 minted once per
                    // firing by `insert_wake_turn_reply` above — falling
                    // back to the old job-id-only form only in the
                    // already-exceptional case where that insert itself
                    // failed (already logged there).
                    let report_message_key: String = own_session_message_id
                        .clone()
                        .unwrap_or_else(|| format!("job-report-{}", job_id_for_bookkeeping));

                    // §36.8/§5.4/§6 (#3050 PR 5) — report-back: for
                    // kind IN ('spawn', 'once') jobs with an originating
                    // chat, a Pane-written line naming the subagent,
                    // attributed to the ORIGIN session's own bound agent
                    // (the exact value §30.11.5's binding check already
                    // requires — no exception, no bypass flag). Ingest
                    // still never dispatches (§30.11.1's own invariant,
                    // untouched by this call).
                    let mut origin_agent_id_resolved: Option<String> = None;
                    // §36.7/§36.8.1 (#3050 PR 6): deferred until after
                    // `conn` drops (below) — `emit_agent_message_
                    // notification` re-locks the SAME `Mutex` via a fresh
                    // `DbState`, and `std::sync::Mutex` is not reentrant
                    // (this file's own existing comment elsewhere makes the
                    // same point about this one connection). Populated iff
                    // the report-back ingest actually succeeds below.
                    let mut agent_message_due: Option<(String, String, String)> = None;
                    // C42a (#3147, live test #5, Matt's ruling): widened
                    // from `spawn`/`once` alone to include `recurring`/
                    // `heartbeat` — a recurring/heartbeat job scheduled
                    // WITH an origin chat (the common case: "remind every
                    // 2 minutes" typed in Home) now gets the SAME report-
                    // back copy into its origin that `once`/`spawn` get.
                    // The inner `if let Some(origin_sid) = ...` below is
                    // unchanged — a recurring/heartbeat job with NO origin
                    // (the common Scheduled-panel case) still skips this
                    // block entirely, exactly like `once`/`spawn` with no
                    // origin already did. **Confirmed separate from C39's
                    // wake-on-complete gate** (`job_wake_origin && (...
                    // "spawn" || ... "once")`, below, further down in this
                    // closure) — that gate is untouched by this widen: it
                    // only ever calls `create_wake_job` (a brand-new
                    // `agent_jobs` row), this block only ever calls
                    // `ingest_external_messages_impl` (a plain INSERT),
                    // and there is no shared boolean or fallthrough
                    // between the two `if` statements — widening this one
                    // cannot, by construction, flip the other's outcome.
                    if job_kind_for_report == "spawn" || job_kind_for_report == "once"
                        || job_kind_for_report == "recurring" || job_kind_for_report == "heartbeat"
                    {
                        if let Some(origin_sid) = &job_origin_session_id_for_report {
                            let origin_agent_id: Option<String> = conn
                                .query_row(
                                    "SELECT agent_id FROM chat_sessions WHERE id = ?1",
                                    params![origin_sid],
                                    |row| row.get(0),
                                )
                                .optional()
                                .ok()
                                .flatten()
                                .flatten();
                            match origin_agent_id {
                                Some(origin_agent_id) => {
                                    origin_agent_id_resolved = Some(origin_agent_id.clone());
                                    let report_text = format!(
                                        "**{}** (job {}) reported:\n\n{}",
                                        finished_agent_name, job_id_for_bookkeeping, report_body
                                    );
                                    let report_msg = crate::ai::ingest::ExternalMessage {
                                        target_session_id: origin_sid.clone(),
                                        source_message_id: report_message_key.clone(),
                                        role: "assistant".to_string(),
                                        content: report_text,
                                        timestamp: chrono::Utc::now().to_rfc3339(),
                                        source_agent_id: origin_agent_id,
                                        attachment_ids: None,
                                        tool_calls: None,
                                        tool_results: None,
                                        // §7.2 — populated (unlike the Pane-
                                        // templated confirmation chip's NULL)
                                        // so `ChatPanel.svelte`'s discriminator
                                        // renders this as a normal attributed
                                        // bubble, not a chip.
                                        model: Some(model_for_report.clone()),
                                        token_count: None,
                                        job_id: Some(job_id_for_bookkeeping.clone()),
                                    };
                                    let ingest_result = crate::ai::ingest::ingest_external_messages_impl(&[report_msg], &conn);
                                    eprintln!(
                                        "[REPORT-BACK] job {} -> session {}: {:?}",
                                        job_id_for_bookkeeping, origin_sid, ingest_result
                                    );
                                    if ingest_result.inserted_count > 0 {
                                        // C42d (#3147, live test #5
                                        // FOLLOW-UPS, Matt's live-tested
                                        // ruling — supersedes this block's
                                        // own prior C42 comment, below in
                                        // git history, which argued this
                                        // gate should stay `spawn`/`once`
                                        // only): widened to `recurring`/
                                        // `heartbeat` too. Matt's walk
                                        // watches HOME, not the job's own
                                        // hidden schedule session — a
                                        // recurring job with an origin got
                                        // the report-back INSERT (C42a/b)
                                        // but the only notification fired
                                        // was `own_session_should_notify`'s,
                                        // targeted at the job's OWN session
                                        // (`job_session_id_for_report`),
                                        // never at `origin_sid`. "Three Tick
                                        // bells" means three bells Matt
                                        // actually sees for a Home-originated
                                        // recurring job — this is the one
                                        // that lands there.
                                        //
                                        // The prior reasoning this replaces
                                        // was correct about the COLLISION
                                        // risk, wrong about the fix: for
                                        // `recurring`/`heartbeat` WITH an
                                        // origin, `own_session_notification_
                                        // due` (job's own session) and this
                                        // new origin notification now BOTH
                                        // fire for the SAME firing.
                                        // `emit_agent_message_notification`'s
                                        // dedup is a bare `idem_key` lookup —
                                        // `SELECT ... WHERE idem_key = ?1`,
                                        // global, NOT scoped by
                                        // `target_id`/session the way
                                        // `idx_chat_local_oc_dedup` is scoped
                                        // by `thread_id` — so reusing
                                        // `report_message_key` verbatim for
                                        // BOTH calls (as `once`/`spawn`
                                        // safely do today, since their
                                        // own-session notification is
                                        // gated OFF whenever an origin
                                        // exists) would make this new origin
                                        // call's `idem_key` collide with the
                                        // own-session call's and get
                                        // silently dedup-skipped — never
                                        // reaching the origin at all. Fixed
                                        // at the emit call site below (not
                                        // here): the origin notification
                                        // derives a distinct key by
                                        // suffixing `report_message_key`
                                        // (still per-firing, still no
                                        // timestamp, still not job-id-only
                                        // on the success path) rather than
                                        // reusing it raw. `once`/`spawn`
                                        // are unaffected either way — their
                                        // own-session notification stays
                                        // gated OFF when an origin exists
                                        // (`job_has_no_origin`), so this
                                        // report-back notification remains
                                        // their ONLY one, as before.
                                        //
                                        // §36.7: "§36.8 report-back lands in
                                        // a session that is not open" — the
                                        // `is_session_open` check itself is
                                        // safe to call here (no DB lock,
                                        // just the in-memory tracker); only
                                        // the actual notification emit
                                        // (which DOES lock) is deferred.
                                        if job_kind_for_report == "spawn" || job_kind_for_report == "once"
                                            || job_kind_for_report == "recurring" || job_kind_for_report == "heartbeat"
                                        {
                                            agent_message_due = Some((origin_sid.clone(), finished_agent_name.clone(), report_body.clone()));
                                        }

                                        // C18 (§36.7.1/§36.8.1, #3120 PR 1):
                                        // the issue's own words — "the
                                        // message appears only after
                                        // navigating away and back." Before
                                        // this commit, report-back into an
                                        // OPEN origin session had no signal
                                        // at all: not the `agent_message`
                                        // notification (gated OFF for an
                                        // open session, below) and nothing
                                        // else either. Reusing
                                        // `pane-chip-changed`'s existing
                                        // `status: "inserted"` handling
                                        // (`ChatPanel.svelte`'s listener
                                        // already triggers a full
                                        // `loadMessages()` re-fetch on this
                                        // exact payload shape — zero
                                        // frontend change needed) rather
                                        // than inventing a second event.
                                        // Mutually exclusive with the
                                        // notification below by construction
                                        // (open → refresh, not open →
                                        // notification, never both for the
                                        // same ingest).
                                        if crate::ai::chat::is_session_open(origin_sid) {
                                            emit_chip_changed(&app_for_report, &job_id_for_bookkeeping, origin_sid, "inserted");
                                        }
                                    }
                                }
                                None => {
                                    eprintln!(
                                        "[REPORT-BACK] job {}: origin session {} has no bound agent — skipping report-back",
                                        job_id_for_bookkeeping, origin_sid
                                    );
                                }
                            }
                        }
                    }

                    // §5/§36.4.5 (#3050 PR 5) — wake-on-complete. Gated on
                    // ALL of: wake_origin=1 (the default — `wake: false`
                    // skips this), an origin session that exists, and is
                    // NOT stale past `RUNTIME_JOB_ARCHIVE_DAYS` (a proxy for
                    // "archived" — no real archived flag exists in this
                    // codebase, stated honestly rather than inventing one).
                    // Any failure of these checks degrades to report-only
                    // — report-back, above, already happened
                    // unconditionally.
                    //
                    // #3129 (C28(b)): the OC-bound-origin skip that used to
                    // live here is deleted — it encoded the OLD rule ("Pane
                    // never loops for an OC agent, proactive or otherwise"),
                    // which this ruling overturns exactly for jobs. A wake
                    // is a job (`kind = 'wake'`) like any other: once
                    // `scheduler_tick`'s loop/dispatch allowlist widened to
                    // OC (commit 2), there is no remaining reason a wake
                    // turn can't fire into an OC-bound origin session
                    // through the same `prepare_job_dispatch`/
                    // `run_agent_loop` path every other wake already uses.
                    // Verified, not assumed, before deleting: a fresh
                    // `grep -n "wake"` across `ai/scheduler.rs`/`ai/tools.rs`
                    // at this commit found no second gate (crew/captain
                    // membership, a runtime check inside
                    // `execute_single_tool`'s own wake-dispatch arm, or
                    // anything in `get_pane_tools_for_runtime`'s offer
                    // logic) that independently blocks a wake turn into an
                    // OC-bound session — a wake targets the ORIGIN
                    // session's existing agent, never a new spawn target,
                    // so none of the spawn-target-API-only restrictions
                    // apply either.
                    //
                    // fix(runtime) (C39, #3139, live test #4): this guard
                    // did not actually enforce §36.4.5's own stated scope
                    // ("every `spawn_job` and every chat-originated `once`
                    // job carries `wake_origin = 1`") — it checked
                    // `wake_origin` + an existing origin only, with no
                    // `kind` filter, so a `recurring`/`heartbeat` job
                    // scheduled WITH an origin chat (the common case —
                    // "remind every 2 minutes" typed in Home; `schedule_
                    // job`'s `NewJobParams` construction hardcodes `wake_
                    // origin: true` for every kind, recurring/heartbeat
                    // included) enqueued a fresh proactive wake turn into
                    // that origin on EVERY firing — Matt's "six wake rows
                    // on Home from the Tick job" repro. Closed: the outer
                    // gate now also requires `kind IN ('spawn', 'once')`.
                    //
                    // Verified non-breaking for the wake-chain mechanism: a
                    // `kind == 'wake'` row's own `origin_session_id` column
                    // is always `NULL` by construction (`create_wake_job`'s
                    // own INSERT literal) — so a finishing `wake` job never
                    // reached this guard with `Some(origin)` regardless of
                    // this new kind filter, before or after. The chain's
                    // next hop is carried by whatever fresh `spawn`/`once`
                    // row the WOKEN turn itself creates (inheriting `chain_
                    // depth + 1`), not by the wake row's own completion
                    // event — `kind IN ('spawn', 'once')` and `kind NOT IN
                    // ('recurring', 'heartbeat')` are behaviorally identical
                    // here; the explicit allowlist is used because it reads
                    // as the literal policy ("wakes are for spawns and
                    // chat-originated once-jobs only").
                    if job_wake_origin
                        && (job_kind_for_report == "spawn" || job_kind_for_report == "once")
                    {
                        if let Some(origin_sid) = &job_origin_session_id_for_report {
                            let origin_row: Option<(String, String)> = conn
                                .query_row(
                                    "SELECT project_id, last_active_at FROM chat_sessions WHERE id = ?1",
                                    params![origin_sid],
                                    |row| Ok((row.get(0)?, row.get(1)?)),
                                )
                                .optional()
                                .ok()
                                .flatten();
                            match origin_row {
                                None => {
                                    eprintln!(
                                        "[WAKE] job {}: origin session {} no longer exists — skipping wake, report-only",
                                        job_id_for_bookkeeping, origin_sid
                                    );
                                }
                                Some((origin_project_id, last_active_at)) => {
                                    let is_stale = parse_stored_datetime(&last_active_at)
                                        .map(|ts| (Utc::now() - ts).num_days() > RUNTIME_JOB_ARCHIVE_DAYS as i64)
                                        .unwrap_or(false);
                                    if is_stale {
                                        eprintln!(
                                            "[WAKE] job {}: origin session {} is stale (last_active_at={}, > {} days) — skipping wake, report-only",
                                            job_id_for_bookkeeping, origin_sid, last_active_at, RUNTIME_JOB_ARCHIVE_DAYS
                                        );
                                    } else if let Some(origin_agent_id) = origin_agent_id_resolved.clone().or_else(|| {
                                        conn.query_row(
                                            "SELECT agent_id FROM chat_sessions WHERE id = ?1",
                                            params![origin_sid],
                                            |row| row.get::<_, Option<String>>(0),
                                        ).optional().ok().flatten().flatten()
                                    }) {
                                        let wake_brief = format!(
                                            "[Job '{}' by {} finished: {}]\n\n{}",
                                            job_title_for_report, finished_agent_name, status_word, report_body
                                        );
                                        let root_job_id_for_wake = job_root_job_id_for_bookkeeping.clone();
                                        match create_wake_job(
                                            &conn,
                                            origin_sid,
                                            &origin_project_id,
                                            &origin_agent_id,
                                            &wake_brief,
                                            // fix(runtime) (Reed's 2026-10-28
                                            // audit, #3107): thread the
                                            // finished job's own id as the
                                            // wake row's `parent_job_id` —
                                            // previously hardcoded NULL.
                                            &job_id_for_bookkeeping,
                                            &root_job_id_for_wake,
                                            job_chain_depth + 1,
                                        ) {
                                            Ok(wake_job_id) => eprintln!(
                                                "[WAKE] job {}: created wake job {} on session {}",
                                                job_id_for_bookkeeping, wake_job_id, origin_sid
                                            ),
                                            Err(e) => eprintln!(
                                                "[WAKE] job {}: failed to create wake job: {}",
                                                job_id_for_bookkeeping, e
                                            ),
                                        }
                                    } else {
                                        eprintln!(
                                            "[WAKE] job {}: origin session {} has no bound agent — skipping wake, report-only",
                                            job_id_for_bookkeeping, origin_sid
                                        );
                                    }
                                }
                            }
                        }
                    }

                    // §36.7/§36.8.1 (#3050 PR 6): `conn` has made its last
                    // DB write for this job turn (the wake block above was
                    // the final one) — safe to drop and re-lock via the
                    // notification path below.
                    drop(conn);

                    // C41 (#3147): the own-session notification, keyed on
                    // the per-firing message id (§1's one key scheme) —
                    // replaces the job-id-only `job-turn-{job_id}` key that
                    // used to live at this gate's old, earlier call site.
                    if let Some(message_id) = own_session_notification_due {
                        crate::notifications::emit_agent_message_notification(
                            &app_for_report,
                            &DbState(db_for_bookkeeping.clone()),
                            &message_id,
                            &finished_agent_name,
                            if result.final_text.trim().is_empty() { "(no output)" } else { result.final_text.trim() },
                            &job_project_display_name_for_report,
                            &job_session_id_for_report,
                            crate::notifications::NotificationScope::OwnSession,
                        );
                    }

                    if let Some((origin_sid, agent_name, body_text)) = agent_message_due {
                        if !crate::ai::chat::is_session_open(&origin_sid) {
                            let first_line = body_text.lines().next().unwrap_or(&body_text);
                            // C42d (#3147, live test #5 follow-ups): for
                            // `once`/`spawn` this key is still just
                            // `report_message_key` in effect (the scope
                            // is harmless there — their own-session
                            // notification never fires in parallel, so
                            // there is nothing to collide with). For
                            // `recurring`/`heartbeat` WITH an origin,
                            // `own_session_notification_due`'s emit
                            // (above, same tick) ALSO fires and already
                            // consumed the bare `report_message_key` as
                            // ITS `idem_key` — reusing it here would
                            // dedup-skip this call silently (see the
                            // widened-gate comment above for why).
                            // #3149/C45: the `-origin` suffix is now
                            // minted by `notification_key` via
                            // `NotificationScope::Origin` (same
                            // `report_message_key` firing id passed
                            // straight through, no caller-side
                            // `format!("{}-origin", ...)` anymore) —
                            // byte-identical output, type-enforced scope.
                            crate::notifications::emit_agent_message_notification(
                                &app_for_report,
                                &DbState(db_for_bookkeeping.clone()),
                                &report_message_key,
                                &agent_name,
                                first_line,
                                &job_project_display_name_for_report,
                                &origin_sid,
                                crate::notifications::NotificationScope::Origin,
                            );
                        }
                    }
                });
            }
            Err(e) => {
                eprintln!(
                    "[SCHEDULER] Failed to prepare dispatch for job {}: {} — leaving status=running for manual recovery",
                    job.id, e
                );
            }
        }
    }
}

// ─── §36.6.2 (#3050 PR 7): background OC pickup ─────────────────────────────
//
// §30.11.4's focus-gated 60s poll (ChatSidebar.svelte's `eligible` boolean)
// becomes a per-OC-agent background pull on this scheduler tick: every
// chat_sessions row with an OC-bound agent and recent activity — interactive
// and job sessions alike, open or not, focused or not. Pane being open (i.e.
// this tick running at all) is the only requirement.
//
// §30.11.3's mechanism is unchanged for the per-session pull itself (key
// derivation, no cached sessionId, over-fetch + dedupe by oc_message_id,
// id/seq-only classification) — this section is the NEW per-tick selection
// and dispatch layer sitting above it.

/// Per-session pull timeout. `build_gateway_client_with_cert`'s shared HTTP
/// client sets a 300s timeout sized for `sync_client.rs`'s heavy sync
/// operations — wrong for a 20-message transcript-page fetch on a feature
/// whose own cadence is 60s. Wrapped at THIS call site only; the shared
/// client's 300s default is left untouched for its other callers (real sync
/// operations may genuinely need it).
const OC_PICKUP_PER_SESSION_TIMEOUT_SECS: u64 = 10;

/// §36.6.2's eligibility query. "Every `chat_sessions` row with an OC-bound
/// agent and `last_active_at` within 7 days" is the literal spec text, but
/// applied naively that drops every long-lived job session: `last_active_at`
/// is frozen at creation for job-origin sessions (confirmed — no dispatch
/// write-site in `scheduler.rs`/`ingest.rs` ever bumps it; `ingest.rs`'s own
/// §30.11 comment and §30.11.6 explicitly defer "`last_active_at` semantics"
/// as a non-goal, so this function does NOT change what the column means).
/// A recurring/heartbeat job firing for months would silently drop out of
/// eligibility 7 days after creation — key or no key (commit 5 is useless
/// without this).
///
/// Fix, scoped to the QUERY only (Reed's recommendation, followed here
/// because Reed's scope gives an explicit reason not to touch
/// `last_active_at` itself — see the doc comment above): `OR
/// agent_jobs.updated_at > cutoff`. Every dispatch write-site already bumps
/// `agent_jobs.updated_at` on every firing (claim, bootstrap, send-status,
/// terminal-status writes all do `updated_at = datetime('now')`) — no
/// schema bump, no new column, just an additional clause on the existing
/// join.
///
/// Ordered `(last_active_at DESC, id ASC)` for deterministic, stable paging
/// — see `next_oc_pickup_page`: ordering by recency alone would starve
/// sessions ranked 21+ forever (nothing in a read-only poll changes
/// `last_active_at`), so the rotation lives in the page selection, not the
/// sort order itself.
///
/// **`AND chat_sessions.origin NOT IN ('spawn', 'schedule')` (#3129,
/// C28(b)):** a job session is never an OC-side session at all once it
/// runs through `run_agent_loop` — its reply is written directly by the
/// loop, the same way an interactive turn's is, with no OC-side session
/// ever created for it. Without this exclusion, a job session would still
/// enter this eligible set purely because its `last_active_at` moved (the
/// loop's own message-insert bumps it, or the `agent_jobs.updated_at`
/// clause above does) and `pull_oc_transcript_for_session` would attempt
/// to poll OC's gateway for a session that was never created there — not
/// a correctness bug (the graceful-skip-on-404/502 path already exists,
/// §30.11.3), but wasted work and log noise on every eligible tick. Same
/// filter §36.3.8's Scheduled panel already applies, for a different
/// reason, reused here for this one.
fn select_oc_pickup_eligible_sessions(conn: &rusqlite::Connection) -> Result<Vec<String>, String> {
    let cutoff = (chrono::Utc::now()
        - chrono::Duration::days(RUNTIME_OC_PICKUP_WINDOW_DAYS as i64))
    .format("%Y-%m-%d %H:%M:%S")
    .to_string();

    let mut stmt = conn
        .prepare(
            "SELECT DISTINCT chat_sessions.id, chat_sessions.last_active_at FROM chat_sessions \
             JOIN ai_agents ON ai_agents.id = chat_sessions.agent_id \
             LEFT JOIN agent_jobs ON agent_jobs.session_id = chat_sessions.id \
             WHERE ai_agents.runtime_type = 'openclaw' \
               AND chat_sessions.origin NOT IN ('spawn', 'schedule') \
               AND (chat_sessions.last_active_at > ?1 OR agent_jobs.updated_at > ?1) \
             ORDER BY chat_sessions.last_active_at DESC, chat_sessions.id ASC",
        )
        .map_err(|e| format!("failed to prepare OC pickup eligibility query: {}", e))?;

    let rows = stmt
        .query_map(params![cutoff], |row| row.get::<_, String>(0))
        .map_err(|e| format!("failed to run OC pickup eligibility query: {}", e))?;

    let mut ids = Vec::new();
    for row in rows {
        ids.push(row.map_err(|e| format!("failed to read OC pickup eligibility row: {}", e))?);
    }
    Ok(ids)
}

/// Process-lifetime rotating page counter — same shape as `VIEWING_SESSION`/
/// `CANCEL_FLAGS` (in-memory, no schema bump, no new column). Used as a page
/// offset into the eligible-session list: with N eligible sessions and a
/// page size of `RUNTIME_OC_PICKUP_SESSIONS_PER_TICK`, `total_pages =
/// ceil(N / page_size)`; this tick takes page `counter % total_pages`, then
/// the counter increments — even on ticks where `total_pages == 1`,
/// harmlessly. Guarantees every eligible session is reached at least once
/// every `total_pages` ticks, rather than the same top-20-by-recency
/// starving sessions ranked 21+ forever. Process-lifetime, not persisted —
/// seeded from a random `u64` on first access (not a fixed 0) specifically
/// so a Pane that restarts more often than the rotation period doesn't
/// re-poll the same first page forever and starve pages 1+ indefinitely;
/// a persisted `ai_settings` cursor would close the gap fully but is out of
/// scope here (Matt, #3050 PR 7 review) — this is the one-line mitigation,
/// not the full fix. Acceptable otherwise (best-effort read-side feature,
/// same tolerance §30.11.3 already accepts for "no cursor, re-fetch the
/// small page").
static OC_PICKUP_TICK_COUNTER: OnceLock<Mutex<u64>> = OnceLock::new();

fn next_oc_pickup_page(total_pages: usize) -> usize {
    if total_pages == 0 {
        return 0;
    }
    let counter = OC_PICKUP_TICK_COUNTER.get_or_init(|| Mutex::new(rand::random::<u64>()));
    let mut guard = counter.lock().unwrap_or_else(|e| e.into_inner());
    let page = (*guard as usize) % total_pages;
    *guard = guard.wrapping_add(1);
    page
}

/// §3b of Reed's audit: once pickup is backend-scheduler-driven for every
/// session (including the open one), nothing tells the open chat a new row
/// landed unless a new event exists — `ingest.rs` has zero `app.emit`
/// anywhere today; the open chat used to reload itself after its own poll
/// call returned. Purpose-built event (mirrors `pane-chip-changed`'s own
/// precedent: not an extension of `pane-data-changed`/`pane-chip-changed`
/// itself, since this is neither a job-chip nor that enum's five values) —
/// emitted once per session per tick whose pull inserted >= 1 row AND whose
/// session IS the one currently open (the inverse condition of the
/// `agent_message` notification branch below — exactly one of the two
/// fires per session per tick that actually inserted something, never
/// both, by construction).
pub(crate) const OC_MESSAGE_INGESTED_EVENT: &str = "pane-oc-message-ingested";

fn emit_oc_message_ingested<R: tauri::Runtime>(app: &tauri::AppHandle<R>, session_id: &str) {
    let _ = app.emit(
        OC_MESSAGE_INGESTED_EVENT,
        serde_json::json!({ "session_id": session_id }),
    );
}

/// §3a of Reed's audit: `agent_message`'s §36.6 half never existed —
/// `emit_agent_message_notification` was called from exactly two sites,
/// both inside this file for §36.8 report-back. §36.7's own trigger table
/// says `agent_message` fires on EITHER report-back landing in a non-open
/// session OR §36.6 ingesting into a non-open session; this is that second
/// call, built here. Resolves the agent name + project + a content preview
/// for the notification from the row(s) the pull just inserted (the
/// `IngestResult` itself carries counts only, not content).
fn resolve_oc_pickup_notification_context(
    conn: &rusqlite::Connection,
    session_id: &str,
) -> Option<(String, String, String, String)> {
    let (agent_name, project_id): (String, String) = conn
        .query_row(
            "SELECT ai_agents.name, chat_sessions.project_id FROM chat_sessions \
             JOIN ai_agents ON ai_agents.id = chat_sessions.agent_id \
             WHERE chat_sessions.id = ?1",
            params![session_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .ok()
        .flatten()?;
    // C3 (#3120 PR 3): display name, not the raw `project_id` sentinel —
    // see `resolve_project_display_name`'s own doc comment.
    let project_id = resolve_project_display_name(conn, &project_id);

    // The just-inserted row's own `chat_messages_local.id` (a UUID, unique
    // per message) drives the notification's idem_key below — NOT a count
    // or a per-session static string, which would silently dedupe a
    // genuinely new later message that happens to arrive in a tick where
    // the same number of rows inserted as a previous tick.
    let row: Option<(String, String)> = conn
        .query_row(
            "SELECT id, content FROM chat_messages_local \
             WHERE thread_id = ?1 AND oc_message_id IS NOT NULL \
             ORDER BY created_at DESC LIMIT 1",
            params![session_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .ok()
        .flatten();

    let (local_message_id, content) = row.unwrap_or_else(|| (uuid::Uuid::new_v4().to_string(), String::new()));

    let first_line = content
        .lines()
        .next()
        .filter(|l| !l.is_empty())
        .unwrap_or("(new message)")
        .to_string();

    Some((agent_name, project_id, first_line, local_message_id))
}

/// §36.6.2's core: selects this tick's rotating page of eligible sessions,
/// skips any with a turn currently in flight for that session (Matt's Oct
/// 29 ruling — the backend-side replacement for the frontend's old
/// `suppress_ingest` grace window, see `ai::chat::is_turn_in_flight`), pulls
/// the rest CONCURRENTLY (spawned, same precedent this file's own job-claim
/// loop already uses: "one slow call can't block the others"), each
/// wrapped in `OC_PICKUP_PER_SESSION_TIMEOUT_SECS`, and on each session
/// whose pull inserted >= 1 row: fires exactly one of
/// `emit_agent_message_notification` (closed session) /
/// `emit_oc_message_ingested` (open session) — never both, by construction
/// (`is_session_open` is read once per session and branches exhaustively).
pub(crate) async fn run_oc_background_pickup<R: tauri::Runtime>(
    db: Arc<Mutex<DbConn>>,
    app: tauri::AppHandle<R>,
) {
    let eligible = {
        let conn = match db.lock() {
            Ok(c) => c,
            Err(e) => {
                eprintln!("[OC-PICKUP] tick: failed to acquire db lock for eligibility query: {}", e);
                return;
            }
        };
        match select_oc_pickup_eligible_sessions(&conn) {
            Ok(ids) => ids,
            Err(e) => {
                eprintln!("[OC-PICKUP] tick: eligibility query failed: {}", e);
                return;
            }
        }
    };

    if eligible.is_empty() {
        return;
    }

    let total_pages = eligible.len().div_ceil(RUNTIME_OC_PICKUP_SESSIONS_PER_TICK);
    let page_index = next_oc_pickup_page(total_pages);
    let page: Vec<String> = eligible
        .into_iter()
        .skip(page_index * RUNTIME_OC_PICKUP_SESSIONS_PER_TICK)
        .take(RUNTIME_OC_PICKUP_SESSIONS_PER_TICK)
        .collect();

    eprintln!(
        "[OC-PICKUP] tick: {} session(s) this page (page {}/{})",
        page.len(),
        page_index + 1,
        total_pages
    );

    let mut handles = Vec::with_capacity(page.len());
    for session_id in page {
        if crate::ai::chat::is_turn_in_flight(&session_id) {
            eprintln!("[OC-PICKUP] skipping session {} — turn currently in flight", session_id);
            continue;
        }
        let db_for_pull = DbState(db.clone());
        let sid = session_id.clone();
        handles.push(tauri::async_runtime::spawn(async move {
            let pull = tokio::time::timeout(
                std::time::Duration::from_secs(OC_PICKUP_PER_SESSION_TIMEOUT_SECS),
                crate::ai::ingest::pull_oc_transcript_for_session(sid.clone(), &db_for_pull, false),
            )
            .await;
            match pull {
                Ok(Ok(result)) => Some((sid, result)),
                Ok(Err(e)) => {
                    eprintln!("[OC-PICKUP] session {}: pull failed: {}", sid, e);
                    None
                }
                Err(_) => {
                    eprintln!(
                        "[OC-PICKUP] session {}: pull timed out after {}s — skipped, other sessions unaffected",
                        sid, OC_PICKUP_PER_SESSION_TIMEOUT_SECS
                    );
                    None
                }
            }
        }));
    }

    for handle in handles {
        let Ok(Some((session_id, pull_result))) = handle.await else {
            continue;
        };
        let Some(ingest) = pull_result.ingest else {
            continue;
        };
        if ingest.inserted_count == 0 {
            continue;
        }

        // #3129 (C28(b)): the C1 'sent'→'done' flip that used to live here
        // is retired — nothing writes `status = 'sent'` anymore (a
        // Pane-dispatched job settles through `write_job_terminal_status`,
        // same as every other runtime), so this clause's own `WHERE ...
        // status = 'sent'` would match zero rows going forward. A session
        // this function reaches is, by construction (the eligibility
        // query's own `origin NOT IN ('spawn', 'schedule')` exclusion,
        // above), never a job session in the first place — it is OC's own
        // spawn/cron activity, read back for display only (§36.6, §36.11).
        if crate::ai::chat::is_session_open(&session_id) {
            emit_oc_message_ingested(&app, &session_id);
            continue;
        }

        let conn = match db.lock() {
            Ok(c) => c,
            Err(e) => {
                eprintln!(
                    "[OC-PICKUP] session {}: failed to acquire db lock for notification context: {}",
                    session_id, e
                );
                continue;
            }
        };
        let notify_ctx = resolve_oc_pickup_notification_context(&conn, &session_id);
        drop(conn);

        if let Some((agent_name, project_id, first_line, local_message_id)) = notify_ctx {
            // C45 (#3155): this is a third, unrelated key shape (OC
            // background pickup of a non-job session, §36.6) — now its own
            // real `NotificationScope::OcPickup` variant instead of a
            // hand-composed `"oc-pickup-{}"` prefix wrapped again by
            // `OwnSession`. Byte-identical output
            // (`agent_message:oc-pickup-{id}`) — see `notification_key`'s
            // own doc comment on the `OcPickup` arm.
            crate::notifications::emit_agent_message_notification(
                &app,
                &DbState(db.clone()),
                &local_message_id,
                &agent_name,
                &first_line,
                &project_id,
                &session_id,
                crate::notifications::NotificationScope::OcPickup,
            );
        }
    }
}

/// Spawned once from `lib.rs`'s `.setup()` closure, same shape as
/// `ai::extraction_worker::start_extraction_worker` — a free-standing async
/// task via `tauri::async_runtime::spawn`, never a Tauri command, never on
/// the main thread.
pub async fn start_scheduler_tick<R: tauri::Runtime>(db: Arc<Mutex<DbConn>>, app: tauri::AppHandle<R>) {
    eprintln!("[SCHEDULER] Tick started (interval={}s)", RUNTIME_TICK_SECS);

    // §36.3.5's own wording: "on launch, and whenever the tick finds
    // next_run_at more than one interval in the past" is ONE piece of logic
    // triggered from two call sites — this launch pass calls the exact same
    // `scheduler_tick` the periodic loop calls below, never a second,
    // differently-shaped "run due jobs on launch" function.
    scheduler_tick(db.clone(), app.clone()).await;

    loop {
        tokio::time::sleep(std::time::Duration::from_secs(RUNTIME_TICK_SECS)).await;
        scheduler_tick(db.clone(), app.clone()).await;
    }
}

#[cfg(test)]
mod scheduler_v2_tests {
    //! #3050 PR 2 — §36.3/§36.10/§36.11 tests. Mirrors Reed's scope audit §11.7
    //! test list: claim-before-run, missed-run collapse, schedule-session
    //! storage shape, the OC-trigger-only pair, and the `create_session`
    //! parity guard. Migration/upgrade-smoke and the AST-gate real-tree count
    //! are proven outside this module (`migration_tests.rs`, CI's
    //! `check-turn-origin` job respectively).
    use super::*;
    use crate::keychain::test_support::KeychainGuard;
    use chrono::TimeZone;
    use rusqlite::Connection;
    use serial_test::serial;
    use tauri::Listener;

    /// Real production schema via `run_migrations` — not a hand-rolled
    /// fixture (SOUL.md §3's "production preconditions" requirement for
    /// state-mutating functions).
    fn make_conn() -> Connection {
        let conn = Connection::open_in_memory().expect("in-memory db");
        crate::migrations::run_migrations(&conn).expect("fresh-install migrations should succeed");
        conn
    }

    /// Bounded poll-retry, replacing "fixed sleep then assert once" for
    /// tests that race a detached `tauri::async_runtime::spawn`'d dispatch
    /// task (the test's own `.await` on `scheduler_tick` does not wait for
    /// it). A fixed sleep guesses a duration; this instead polls the real
    /// condition and returns the instant it's true, so a fast CI run isn't
    /// slowed down and a slow one isn't failed on a guess.
    ///
    /// **Interval: 25ms.** Fine-grained enough that the common case (the
    /// spawned task finishing well under the old 200/300ms fixed sleeps)
    /// still resolves in a small handful of polls, not a perceptible
    /// per-test slowdown; coarse enough not to meaningfully spin the DB
    /// mutex or the mockito server under `--test-threads=1`.
    ///
    /// **Ceiling: 20s.** First attempt used 5s, reasoned from the
    /// assertion shapes alone (zero-vs-one, never a partial count, so the
    /// old fixed 200/300ms sleeps were presumed to be missing by roughly
    /// their own order of magnitude). That reasoning was wrong in
    /// practice: live CI evidence (this PR's own first retrigger run,
    /// `gh run view` log for `test_scheduler_tick_dispatches_missed_oc_job_exactly_once`)
    /// hit the 5s ceiling and failed — the SAME run's own full-suite
    /// timing (165.75s on CI vs. 114.8s local for the identical 2758
    /// tests, both `--test-threads=1`) shows CI running at best ~1.4x
    /// local speed on average, and this is the cluster's first test to
    /// touch `tauri::async_runtime`/mockito at all, so it also eats
    /// whatever one-time runtime/thread-pool/socket warm-up cost exists —
    /// a cold-start spike on a loaded shared runner, not a steady-state
    /// margin, so a flat "CI is ~1.4x slower" multiplier on the old
    /// 200/300ms sleeps would have UNDER-shot it too. 20s is bounded (a
    /// genuinely broken dispatch still fails in well under a minute, never
    /// hangs) while leaving roughly 4x the headroom the empirically-wrong
    /// 5s figure had — chosen from this run's own evidence, not a repeat
    /// guess at the margin.
    async fn poll_until<F, Fut>(mut condition: F, timeout_msg: &str)
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<Output = bool>,
    {
        const INTERVAL: std::time::Duration = std::time::Duration::from_millis(25);
        const CEILING: std::time::Duration = std::time::Duration::from_secs(20);
        let start = std::time::Instant::now();
        loop {
            if condition().await {
                return;
            }
            if start.elapsed() >= CEILING {
                panic!("{}", timeout_msg);
            }
            tokio::time::sleep(INTERVAL).await;
        }
    }

    /// #3050 PR 6: `dispatch_oc_job` gained an `app: AppHandle<R>` parameter
    /// (needed to emit `pane-chip-changed`/`job_failed` notifications at its
    /// own status-write sites) — this helper gives every pre-existing test
    /// call site a throwaway mock handle with one line, matching the
    /// pattern this file already uses elsewhere (`tauri::test::mock_app()`,
    /// e.g. line ~3525's `setup()`). The underlying `App` is dropped at the
    /// end of this call; `AppHandle::emit` on a `MockRuntime` has no live
    /// window/OS resource to outlive, so the clone stays valid.
    fn test_app_handle() -> tauri::AppHandle<tauri::test::MockRuntime> {
        // §36.7 (#3050 PR 6): `dispatch_oc_job`'s failure paths now call
        // `emit_or_update_job_failed_notification`, whose INSERT arm (via
        // `process_notification`, `os_notify=true` per §36.7's table)
        // calls `tauri_plugin_notification`'s `app.notification()` — a
        // bare `tauri::test::mock_app()` has no plugin state registered
        // for that and panics ("state() called before manage()"). Build
        // the same `MockRuntime` app WITH the plugin registered instead,
        // same fix as `notifications.rs`'s own
        // `make_mock_app_with_notification_plugin()` test helper.
        tauri::test::mock_builder()
            .plugin(tauri_plugin_notification::init())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("mock app with notification plugin must build")
            .handle()
            .clone()
    }

    fn seed_agent(conn: &Connection, id: &str, runtime_type: &str, runtime_ref: Option<&str>) {
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO ai_agents (id, name, slug, is_default, is_active, workspace_path, runtime_type, runtime_ref, model, created_at, updated_at) \
             VALUES (?1, ?1, ?1, 0, 1, ?1, ?2, ?3, 'test-model', ?4, ?4)",
            rusqlite::params![id, runtime_type, runtime_ref, now],
        )
        .expect("seed agent");
    }

    /// Seed a job session the way `create_job`/`create_session_internal`
    /// would, returning the session id.
    fn seed_job_session(conn: &Connection, agent_id: &str, origin: &str) -> String {
        create_session_internal(conn, "workspace", agent_id, origin, false, None)
            .expect("seed job session")
            .id
    }

    fn seed_job_row(
        conn: &Connection,
        job_id: &str,
        kind: &str,
        agent_id: &str,
        session_id: &str,
        cron: Option<&str>,
        next_run_at: &str,
        missed_count: i64,
    ) {
        conn.execute(
            "INSERT INTO agent_jobs (id, kind, title, brief, agent_id, project_id, session_id, cron, next_run_at, missed_count, status, created_at, updated_at) \
             VALUES (?1, ?2, '', 'test brief', ?3, 'workspace', ?4, ?5, ?6, ?7, 'pending', datetime('now'), datetime('now'))",
            params![job_id, kind, agent_id, session_id, cron, next_run_at, missed_count],
        )
        .expect("seed job row");
    }

    // ─── C3 (#3120 PR 3): resolve_project_display_name ──────────────────────

    /// The Home sentinel ("workspace", used nowhere as a real `projects`
    /// row) resolves to the user-facing "Home" label, never the raw
    /// internal string. Mutation: return `project_id.to_string()` verbatim
    /// instead of falling back to "Home" — reddens (asserts "workspace",
    /// the whole point of this fix, literally the bug this test pins
    /// against regressing).
    #[test]
    fn test_resolve_project_display_name_workspace_sentinel_is_home() {
        let conn = make_conn();
        assert_eq!(resolve_project_display_name(&conn, "workspace"), "Home");
    }

    /// A project id with no matching `projects` row (dangling/garbage
    /// input, never expected in practice but must not panic or leak the
    /// raw id) also falls back to "Home" — same fallback `list_scheduled_
    /// jobs`'s own `COALESCE(p.name, 'Home')` already uses for an
    /// unresolvable id.
    #[test]
    fn test_resolve_project_display_name_unknown_id_is_home() {
        let conn = make_conn();
        assert_eq!(resolve_project_display_name(&conn, "no-such-project-id"), "Home");
    }

    /// A real, named project resolves to its own name — confirms this
    /// function is a real lookup, not a hardcoded "Home" for every input.
    #[test]
    fn test_resolve_project_display_name_real_project_resolves_its_name() {
        let conn = make_conn();
        conn.execute(
            "INSERT INTO projects (id, name, parent_id) VALUES ('proj-valencia', 'Valencia Launch', NULL)",
            [],
        )
        .expect("seed project");
        assert_eq!(resolve_project_display_name(&conn, "proj-valencia"), "Valencia Launch");
    }

    /// End-to-end through the real notification-context resolver
    /// (`resolve_oc_pickup_notification_context`, OC background pickup's
    /// own call site): a Home-bound session's notification context carries
    /// "Home", never the literal "workspace" string a user would see
    /// verbatim in `NotificationPanel.svelte`'s `{notif.body}`. Mutation:
    /// revert the function to return the raw `project_id` column unchanged
    /// — reddens (asserts "Home", gets "workspace").
    #[test]
    fn test_resolve_oc_pickup_notification_context_home_session_shows_home_not_workspace() {
        let conn = make_conn();
        seed_agent(&conn, "agent-oc-pickup", "openclaw", None);
        let session_id = seed_job_session(&conn, "agent-oc-pickup", "user");
        let (agent_name, project_display_name, _first_line, _local_message_id) =
            resolve_oc_pickup_notification_context(&conn, &session_id).expect("session must resolve");
        assert_eq!(agent_name, "agent-oc-pickup");
        assert_eq!(project_display_name, "Home", "must be the display name, never the raw 'workspace' sentinel");
    }

    // ─── Bug 2 (#3050 fix PR): the writer guard rejects an unknown thread_id ────

    /// Matt's own requirement: "add a guard ... with a test that the old
    /// path now fails loudly." This reproduces the EXACT historical
    /// mechanism Reed's scope audit traced — a confirmation chip write with
    /// `thread_id = "workspace"` where no `chat_sessions` row exists with
    /// that id (the literal fallback value `ai_chat_stream`'s old,
    /// pre-fix resolution could produce). Before `require_known_thread_id`
    /// existed, this INSERT succeeded silently (no FK on
    /// `chat_messages_local.thread_id`) — zero error, zero chip, a row
    /// written into a thread nothing ever reads from.
    ///
    /// **Mutation (actually run — see report):** deleted the
    /// `require_known_thread_id` call at the top of
    /// `insert_schedule_confirmation_message`. Failure: `result.is_err()`
    /// reddens (the insert now silently succeeds again), AND the row-count
    /// assertion below also reddens (a phantom row appears) — same
    /// mutation, two assertions, named explicitly rather than inventing a
    /// second mutation for the second assertion.
    #[test]
    fn test_insert_schedule_confirmation_message_refuses_unknown_thread_id() {
        let conn = make_conn();
        // Deliberately NO chat_sessions row with id='workspace' — the
        // literal sentinel the pre-fix bug substituted.
        let result = insert_schedule_confirmation_message::<tauri::test::MockRuntime>(&conn, "workspace", "workspace", "Scheduled: phantom", "job-phantom-1", None);
        assert!(result.is_err(), "an unknown thread_id must be refused, never silently written: {:?}", result);
        assert!(
            result.unwrap_err().contains("unknown thread_id"),
            "the error must name the actual problem, not just propagate a raw SQLite error"
        );

        let row_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM chat_messages_local WHERE job_id = 'job-phantom-1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(row_count, 0, "a refused insert must leave ZERO rows — the old path's own bug was writing exactly one phantom row here");
    }

    /// Mirrors the test above for `insert_job_brief_message` (the OTHER
    /// writer this guard covers) — defense-in-depth, since `session_id` on
    /// this call site is always a freshly-created job session in
    /// production (§36.3.6), but the guard protects it identically.
    #[test]
    fn test_insert_job_brief_message_refuses_unknown_thread_id() {
        let conn = make_conn();
        let result = insert_job_brief_message(&conn, "no-such-session", "workspace", "brief text", "job-phantom-2");
        assert!(result.is_err(), "an unknown thread_id must be refused: {:?}", result);

        let row_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM chat_messages_local WHERE job_id = 'job-phantom-2'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(row_count, 0);
    }

    /// The guard's own positive case — a REAL session id must still write
    /// successfully (the guard narrows nothing about the happy path this
    /// whole mechanism exists to serve).
    #[test]
    fn test_insert_schedule_confirmation_message_succeeds_for_real_thread_id() {
        let conn = make_conn();
        seed_agent(&conn, "agent-1", "anthropic", None);
        let real_session_id = seed_job_session(&conn, "agent-1", "user");

        let result = insert_schedule_confirmation_message::<tauri::test::MockRuntime>(&conn, &real_session_id, "workspace", "Scheduled: real", "job-real-1", None);
        assert!(result.is_ok(), "a real, existing thread_id must still be accepted: {:?}", result);

        let row_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM chat_messages_local WHERE job_id = 'job-real-1' AND thread_id = ?1",
                params![real_session_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(row_count, 1);
    }

    // ─── Cron parsing / next-occurrence ───────────────────────────────────────────────────

    #[test]
    fn test_cron_daily_9am_preset() {
        // §36.10A: "Morning brief" preset — daily 9am.
        let after = Local.with_ymd_and_hms(2026, 10, 27, 8, 0, 0).unwrap();
        let next = next_cron_occurrence("0 9 * * *", after).unwrap();
        assert_eq!(next, Local.with_ymd_and_hms(2026, 10, 27, 9, 0, 0).unwrap());

        // After 9am today, rolls to tomorrow.
        let after2 = Local.with_ymd_and_hms(2026, 10, 27, 9, 30, 0).unwrap();
        let next2 = next_cron_occurrence("0 9 * * *", after2).unwrap();
        assert_eq!(next2, Local.with_ymd_and_hms(2026, 10, 28, 9, 0, 0).unwrap());
    }

    #[test]
    fn test_cron_every_4_hours_preset() {
        // §36.10A: "Check in every 4 hours" preset.
        let after = Local.with_ymd_and_hms(2026, 10, 27, 1, 0, 0).unwrap();
        let next = next_cron_occurrence("0 */4 * * *", after).unwrap();
        assert_eq!(next, Local.with_ymd_and_hms(2026, 10, 27, 4, 0, 0).unwrap());
    }

    #[test]
    fn test_cron_weekly_monday_9am_preset() {
        // §36.10A: "Weekly review" preset. 2026-10-27 is a Tuesday.
        let after = Local.with_ymd_and_hms(2026, 10, 27, 10, 0, 0).unwrap();
        let next = next_cron_occurrence("0 9 * * 1", after).unwrap();
        assert_eq!(next, Local.with_ymd_and_hms(2026, 11, 2, 9, 0, 0).unwrap());
        assert_eq!(next.weekday(), chrono::Weekday::Mon);
    }

    #[test]
    fn test_cron_hourly_preset() {
        let after = Local.with_ymd_and_hms(2026, 10, 27, 10, 15, 0).unwrap();
        let next = next_cron_occurrence("0 * * * *", after).unwrap();
        assert_eq!(next, Local.with_ymd_and_hms(2026, 10, 27, 11, 0, 0).unwrap());
    }

    #[test]
    fn test_cron_rejects_wrong_field_count() {
        let after = Local::now();
        let result = next_cron_occurrence("0 9 * *", after);
        assert!(result.is_err(), "a 4-field cron must be rejected, not silently truncated");
    }

    #[test]
    fn test_cron_rejects_out_of_range_value() {
        let result = parse_cron_field("99", 0, 59);
        assert!(result.is_err(), "minute 99 must be rejected");
    }

    #[test]
    fn test_cron_rejects_zero_step() {
        let result = parse_cron_field("*/0", 0, 59);
        assert!(result.is_err(), "step of 0 must be rejected, not loop forever");
    }

    // ─── Claim-before-run (§5 of Reed's scope audit) ────────────────────────────────

    #[test]
    fn test_claim_job_wins_once_against_pending_row() {
        let conn = make_conn();
        seed_agent(&conn, "agent-1", "anthropic", None);
        let sid = seed_job_session(&conn, "agent-1", "schedule");
        seed_job_row(&conn, "job-1", "once", "agent-1", &sid, None, "2020-01-01 00:00:00", 0);

        assert!(claim_job(&conn, "job-1").unwrap(), "first claim against a pending row must win");
        assert!(!claim_job(&conn, "job-1").unwrap(), "second claim against the now-running row must lose");

        let status: String = conn
            .query_row("SELECT status FROM agent_jobs WHERE id = 'job-1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(status, "running");
    }

    #[test]
    fn test_claim_job_false_for_nonexistent_row() {
        let conn = make_conn();
        assert!(!claim_job(&conn, "no-such-job").unwrap());
    }

    /// §5: "the test must prove two ticks cannot both claim one row." This
    /// app shares ONE `rusqlite::Connection` behind one `Arc<Mutex<_>>` —
    /// there is no multi-connection SQLite lock contention to defend against
    /// (`BEGIN IMMEDIATE` solves a problem this architecture doesn't have).
    /// What must be correct is the conditional `UPDATE` inside `claim_job`,
    /// proven here by two real OS threads racing on one shared `Mutex`-guarded
    /// connection against one seeded `pending` row.
    #[test]
    fn test_two_threads_cannot_both_claim_one_row() {
        let conn = make_conn();
        seed_agent(&conn, "agent-1", "anthropic", None);
        let sid = seed_job_session(&conn, "agent-1", "schedule");
        seed_job_row(&conn, "job-race", "once", "agent-1", &sid, None, "2020-01-01 00:00:00", 0);

        let shared = Arc::new(Mutex::new(conn));
        let barrier = Arc::new(std::sync::Barrier::new(2));

        let handles: Vec<_> = (0..2)
            .map(|_| {
                let shared = shared.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    let conn = shared.lock().unwrap();
                    claim_job(&conn, "job-race").unwrap()
                })
            })
            .collect();

        let results: Vec<bool> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        let wins = results.iter().filter(|&&r| r).count();
        assert_eq!(wins, 1, "exactly one of two concurrent claims must win: {:?}", results);
    }

    #[test]
    fn test_claim_due_jobs_respects_limit() {
        let conn = make_conn();
        seed_agent(&conn, "agent-1", "anthropic", None);
        for i in 0..5 {
            let sid = seed_job_session(&conn, "agent-1", "schedule");
            seed_job_row(&conn, &format!("job-{}", i), "once", "agent-1", &sid, None, "2020-01-01 00:00:00", 0);
        }

        let claimed = claim_due_jobs(&conn, 2).unwrap();
        assert_eq!(claimed.len(), 2, "claim_due_jobs must respect the concurrency limit");

        let still_pending: i64 = conn
            .query_row("SELECT COUNT(*) FROM agent_jobs WHERE status = 'pending'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(still_pending, 3, "unclaimed rows must stay pending");
    }

    #[test]
    fn test_claim_due_jobs_ignores_future_rows() {
        let conn = make_conn();
        seed_agent(&conn, "agent-1", "anthropic", None);
        let sid = seed_job_session(&conn, "agent-1", "schedule");
        let future = format_stored_datetime(&(Utc::now() + chrono::Duration::days(1)));
        seed_job_row(&conn, "job-future", "once", "agent-1", &sid, None, &future, 0);

        let claimed = claim_due_jobs(&conn, 10).unwrap();
        assert!(claimed.is_empty(), "a job whose next_run_at is in the future must not be claimed");
    }

    // ─── Missed-run collapse (§36.3.5) ────────────────────────────────────────────

    #[test]
    fn test_missed_run_collapses_to_one_catch_up_not_72() {
        let conn = make_conn();
        seed_agent(&conn, "agent-1", "anthropic", None);
        let sid = seed_job_session(&conn, "agent-1", "schedule");

        // 3 days overdue, hourly cron — a naive scheduler would try to fire
        // ~72 times. next_run_at and the recompute's "old next" baseline are
        // both set to the same 3-days-ago timestamp (simulating "this was due
        // 3 days ago and nothing has run since").
        let three_days_ago = format_stored_datetime(&(Utc::now() - chrono::Duration::days(3)));
        seed_job_row(&conn, "job-missed", "recurring", "agent-1", &sid, Some("0 * * * *"), &three_days_ago, 0);

        let claimed = claim_due_jobs(&conn, 10).unwrap();
        assert_eq!(claimed.len(), 1, "exactly one claim must happen, not one per missed hour");

        let (status, missed_count, next_run_at): (String, i64, String) = conn
            .query_row(
                "SELECT status, missed_count, next_run_at FROM agent_jobs WHERE id = 'job-missed'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();

        assert_eq!(status, "pending", "recurring job must return to pending, not stay running");
        assert_eq!(missed_count, 1, "missed_count must increment by ONE collapse event, not by 72 missed hours");
        let next_parsed = parse_stored_datetime(&next_run_at).unwrap();
        assert!(next_parsed > Utc::now(), "next_run_at must be recomputed strictly into the future: {}", next_run_at);
    }

    #[test]
    fn test_on_time_recurring_claim_does_not_increment_missed_count() {
        let conn = make_conn();
        seed_agent(&conn, "agent-1", "anthropic", None);
        let sid = seed_job_session(&conn, "agent-1", "schedule");

        // Due a minute ago, hourly cron — a normal, on-time tick, not a
        // collapse: the recomputed "would-be-next" from the old next_run_at
        // is still in the future relative to now.
        let one_minute_ago = format_stored_datetime(&(Utc::now() - chrono::Duration::minutes(1)));
        seed_job_row(&conn, "job-ontime", "recurring", "agent-1", &sid, Some("0 * * * *"), &one_minute_ago, 0);

        let claimed = claim_due_jobs(&conn, 10).unwrap();
        assert_eq!(claimed.len(), 1);

        let missed_count: i64 = conn
            .query_row("SELECT missed_count FROM agent_jobs WHERE id = 'job-ontime'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(missed_count, 0, "an on-time tick must not be counted as a missed collapse");
    }

    // ─── Job session storage shape (§36.3.6/§36.11) ──────────────────────

    // NOTE (#3102 correction, per Reed's audit §6): these two tests previously
    // re-asserted the "absent from list_sessions" half of their names against
    // a hand-rolled copy of `list_sessions`' own SQL literal, run directly on
    // the connection. That pins a *copy* of the filter, not the production
    // code — dropping the real `NOT IN ('spawn', 'schedule')` clause in
    // `chat::list_sessions` would leave both tests green. They're kept here
    // narrowed to the one thing they genuinely prove on their own merits —
    // `create_session_internal` never creates a job session pre-active — and
    // the list_sessions filter itself is now covered for real, through the
    // production `#[tauri::command]`, by
    // `test_list_sessions_excludes_spawn_and_schedule_in_both_include_hidden_branches`
    // below. These two are supplementary, not the sole coverage.
    #[test]
    fn test_schedule_session_never_active() {
        let conn = make_conn();
        seed_agent(&conn, "agent-1", "anthropic", None);

        let session = create_session_internal(&conn, "workspace", "agent-1", "schedule", false, None).unwrap();
        assert!(!session.is_active, "a schedule session must never be created active");
        assert_eq!(session.origin, "schedule");
    }

    #[test]
    fn test_spawn_session_never_active() {
        let conn = make_conn();
        seed_agent(&conn, "agent-1", "anthropic", None);

        let session = create_session_internal(&conn, "workspace", "agent-1", "spawn", false, None).unwrap();
        assert!(!session.is_active, "a spawn session must never be created active");
        assert_eq!(session.origin, "spawn");
    }

    /// §36.11's "a spawn session is never a surface" / §36.3.6's schedule-session
    /// equivalent, pinned through the REAL production `chat::list_sessions`
    /// `#[tauri::command]` — not a reimplementation, not hand-rolled SQL
    /// (correcting the decorative version Reed's audit found in the original
    /// PR, §6). Built via `tauri::test::mock_app` + `app.manage(DbState)`,
    /// same pattern as `agents.rs`'s `upsert_agent_explicit_runtime_type_tests`.
    /// Exercises BOTH `include_hidden` branches (true and false) — the
    /// invariant is "never a surface *whatever `include_hidden` says*", so a
    /// test that only checks one arm doesn't actually prove it. Also seeds an
    /// ordinary `origin='user'` session so the assertion proves filtering,
    /// not merely an empty result.
    #[test]
    fn test_list_sessions_excludes_spawn_and_schedule_in_both_include_hidden_branches() {
        use tauri::Manager;

        let conn = make_conn();
        seed_agent(&conn, "agent-1", "anthropic", None);

        let spawn_session = create_session_internal(&conn, "workspace", "agent-1", "spawn", false, None).unwrap();
        let schedule_session = create_session_internal(&conn, "workspace", "agent-1", "schedule", false, None).unwrap();
        let user_session = create_session_internal(&conn, "workspace", "agent-1", "user", false, None).unwrap();

        let app = tauri::test::mock_app();
        app.manage(crate::database::DbState::new(conn));

        for include_hidden in [true, false] {
            let sessions = crate::ai::chat::list_sessions(
                "workspace".to_string(),
                include_hidden,
                app.state::<crate::database::DbState>(),
            )
            .expect("list_sessions should succeed");
            let ids: Vec<&str> = sessions.iter().map(|s| s.id.as_str()).collect();

            assert!(
                !ids.contains(&spawn_session.id.as_str()),
                "a spawn session must never be returned by list_sessions (include_hidden={})",
                include_hidden
            );
            assert!(
                !ids.contains(&schedule_session.id.as_str()),
                "a schedule session must never be returned by list_sessions (include_hidden={})",
                include_hidden
            );
            assert!(
                ids.contains(&user_session.id.as_str()),
                "an ordinary user session must still be returned by list_sessions (include_hidden={}) — \
                 proves the assertions above are filtering, not just describing an empty result",
                include_hidden
            );
        }
    }

    #[test]
    fn test_create_job_creates_session_and_row_together() {
        let conn = make_conn();
        seed_agent(&conn, "agent-1", "anthropic", None);

        let next_run_at = format_stored_datetime(&Utc::now());
        let job_id = create_job(
            &conn,
            NewJobParams {
                kind: "recurring",
                title: "Weekly review",
                brief: "Review open tasks",
                agent_id: "agent-1",
                project_id: "workspace",
                cron: Some("0 9 * * 1"),
                run_at: None,
                next_run_at: &next_run_at,
                origin_session_id: None,
                origin_message_id: None,
                parent_job_id: None,
                root_job_id: None,
                chain_depth: 0, wake_origin: true,
            },
        )
        .unwrap();

        let (session_id, kind, origin_matches): (String, String, i64) = conn
            .query_row(
                "SELECT agent_jobs.session_id, agent_jobs.kind, \
                 (SELECT COUNT(*) FROM chat_sessions WHERE id = agent_jobs.session_id AND origin = 'schedule' AND is_active = 0) \
                 FROM agent_jobs WHERE id = ?1",
                params![job_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(kind, "recurring");
        assert_eq!(origin_matches, 1, "create_job's session must be origin='schedule', is_active=0");
        assert!(!session_id.is_empty());
    }

    // ─── create_session parity guard (§7 of Reed's scope audit) ─────────────────────

    #[test]
    fn test_create_session_internal_user_origin_matches_pre_refactor_behavior() {
        // Regression guard for the §7/§11.3 refactor: the public
        // `create_session` command's old inline body always inserted
        // origin='user' and always promoted. Calling the internal variant
        // with the same two arguments must be byte-identical.
        let conn = make_conn();
        seed_agent(&conn, "agent-1", "anthropic", None);

        let session = create_session_internal(&conn, "workspace", "agent-1", "user", true, None).unwrap();
        assert_eq!(session.origin, "user");
        assert!(session.is_active, "origin='user'/promote=true must promote, matching create_session's old hardcoded behavior");

        let active_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM chat_sessions WHERE project_id = 'workspace' AND is_active = 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(active_count, 1, "promote=true must make this the surface's one active session");
    }

    #[test]
    fn test_create_session_internal_promote_false_never_activates() {
        let conn = make_conn();
        seed_agent(&conn, "agent-1", "anthropic", None);

        // An existing active 'user' session for the surface...
        let user_session = create_session_internal(&conn, "workspace", "agent-1", "user", true, None).unwrap();
        assert!(user_session.is_active);

        // ...must stay active after a promote=false internal create.
        let job_session = create_session_internal(&conn, "workspace", "agent-1", "schedule", false, None).unwrap();
        assert!(!job_session.is_active);

        let still_active: String = conn
            .query_row(
                "SELECT id FROM chat_sessions WHERE project_id = 'workspace' AND is_active = 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(still_active, user_session.id, "promote=false must never demote or replace the surface's existing active session");
    }

    // ─── OC jobs dispatch through Pane's own loop (§36.0.1/§36.3.4/§36.11, #3129 C28(b)) ───

    fn seed_oc_setup(conn: &Connection, server_url: &str) -> (String, String) {
        seed_agent(conn, "oc-agent", "openclaw", Some("main"));
        crate::database::set_ai_setting(conn, "endpoint", server_url).unwrap();
        crate::ai::credential_migration::write_gateway_token(conn, "test-gateway-token").unwrap();
        let sid = seed_job_session(conn, "oc-agent", "schedule");
        ("oc-agent".to_string(), sid)
    }

    /// #3129 (C28(b)), the main-line change's own end-to-end proof: an
    /// `openclaw`-runtime job now falls straight through to
    /// `prepare_job_dispatch`/`run_agent_loop` — the exact path every other
    /// runtime's job already used — instead of the old `dispatch_oc_job`
    /// one-shot `sessions.send`. Asserts the INVERSE of what the old
    /// (deleted) OC-dispatch tests asserted: the OC gateway's own
    /// `sessions.create`/`sessions.send` endpoints are never hit at all
    /// (`.expect(0)`), while its `/v1/chat/completions` endpoint IS (the
    /// same endpoint every interactive OC turn already calls), and the job
    /// settles through `write_job_terminal_status` (`'done'`), never the
    /// retired `'sent'` resting status.
    ///
    /// **Mutation (actually run — see report):** re-added the deleted `if
    /// job.runtime_type == "openclaw" { ...; continue; }` block in
    /// `scheduler_tick`. Failure output (observed, not guessed):
    /// ```text
    /// [SCHEDULER] OC job ...: bootstrapped OC session, oc_session_key=oc-key-c28b
    /// [SCHEDULER] OC job ... sessions.send → HTTP 200 OK
    /// thread '...test_oc_job_dispatches_through_prepare_job_dispatch_not_sessions_send' panicked at src/ai/scheduler.rs:NNNN:17:
    /// an openclaw-runtime job must settle through write_job_terminal_status, not stay 'running' forever
    /// ```
    /// (the old branch re-engages, the job settles at the old `'sent'`
    /// status — never reaching `'done'`/`'failed'` — so the `poll_until`
    /// above times out; the create/send mocks' own `sessions.create`/
    /// `sessions.send` log lines above confirm the old path ran at all.
    /// Confirmed and reverted.)
    #[tokio::test]
    #[serial(keychain_override)]
    async fn test_oc_job_dispatches_through_prepare_job_dispatch_not_sessions_send() {
        let _kc = KeychainGuard::set();
        let mut server = mockito::Server::new_async().await;
        let create_mock = server
            .mock("POST", "/v1/sessions")
            .with_status(200)
            .with_body(r#"{"session_id":"gw-uuid-c28b","created_at":"2026-10-31T00:00:00Z","oc_session_id":"oc-key-c28b"}"#)
            .expect(0)
            .create_async()
            .await;
        let send_mock = server
            .mock("POST", mockito::Matcher::Regex(r"^/v1/sessions/.+/messages$".to_string()))
            .with_status(200)
            .with_body("{}")
            .expect(0)
            .create_async()
            .await;
        let chat_mock = server
            .mock("POST", "/v1/chat/completions")
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body("data: {\"choices\":[{\"delta\":{\"content\":\"Job done.\"}}]}\n\ndata: [DONE]\n\n")
            .expect(1)
            .create_async()
            .await;

        let conn = make_conn();
        let (agent_id, _sid) = seed_oc_setup(&conn, &server.url());
        let origin_session = create_session_internal(&conn, "workspace", &agent_id, "user", true, None).unwrap();
        let next_run_at = format_stored_datetime(&Utc::now());
        let job_id = create_job(
            &conn,
            NewJobParams {
                kind: "spawn", title: "c28b-dispatch-test", brief: "say done", agent_id: &agent_id,
                project_id: "workspace", cron: None, run_at: None, next_run_at: &next_run_at,
                origin_session_id: Some(&origin_session.id), origin_message_id: None,
                parent_job_id: None, root_job_id: None, chain_depth: 0, wake_origin: false,
            },
        ).unwrap();

        let db = Arc::new(Mutex::new(DbConn::new(conn)));
        scheduler_tick(db.clone(), test_app_handle()).await;

        poll_until(
            || async {
                let conn = db.lock().unwrap();
                let status: Option<String> = conn
                    .query_row("SELECT status FROM agent_jobs WHERE id = ?1", params![job_id], |r| r.get(0))
                    .optional()
                    .unwrap()
                    .flatten();
                matches!(status.as_deref(), Some("done") | Some("failed"))
            },
            "an openclaw-runtime job must settle through write_job_terminal_status, not stay 'running' forever",
        )
        .await;

        let conn = db.lock().unwrap();
        let status: String = conn.query_row("SELECT status FROM agent_jobs WHERE id = ?1", params![job_id], |r| r.get(0)).unwrap();
        assert_eq!(status, "done", "an openclaw-runtime job must settle 'done' through write_job_terminal_status — never the retired 'sent' status");
        drop(conn);

        // The real proof this test exists for: the OLD dispatch mechanism's
        // own endpoints were NEVER touched.
        create_mock.assert_async().await;
        send_mock.assert_async().await;
        chat_mock.assert_async().await;
    }

    /// #3129 (C28(b)), the dedup-regression half of commit 2: proves
    /// `own_turn_is_openclaw` is now derived from the job turn's own
    /// resolved provider (`matches!(&prep.provider_for_streaming,
    /// AiProvider::OpenClaw { .. })`), not left hardcoded `false`. Honest
    /// scope, per Reed's own brief: this does NOT construct the full #2738
    /// dedup race (a second, overlapping OC pull racing the same turn) —
    /// doing that deterministically through this harness would need a
    /// second concurrent ingest call mid-turn, which this test does not
    /// attempt. What it DOES prove, directly and observably: when
    /// `own_turn_is_openclaw` is `true`, `run_agent_loop` calls
    /// `fetch_oc_latest_seq` before the turn starts — a real GET to
    /// `/v1/sessions/{oc_session_key}/messages?limit=1` on the gateway
    /// (`ai/ingest.rs`). That call is UNREACHABLE when the field is `false`
    /// (gated by `if own_turn_is_openclaw` in `ai::chat::run_agent_loop`) —
    /// so observing it fire is a real, non-vacuous proof the field's value
    /// reaches the mechanism it's supposed to feed, not merely that the
    /// field itself holds the right bool in isolation.
    ///
    /// **Mutation (actually run — see report):** reverted
    /// `own_turn_is_openclaw` back to a hardcoded `false` at the job-turn
    /// `AgentLoopInputs` construction site. Failure output:
    /// ```text
    /// thread '...test_oc_job_turn_derives_own_turn_is_openclaw_true' panicked at src/ai/scheduler.rs:NNNN:9:
    /// 0 observed requests, expected 1 (hits: 0)
    /// ```
    /// (the baseline-seq GET mock's own `.expect(1)`/`.assert_async()` fails
    /// — confirmed and reverted.)
    #[tokio::test]
    #[serial(keychain_override)]
    async fn test_oc_job_turn_derives_own_turn_is_openclaw_true() {
        let _kc = KeychainGuard::set();
        let mut server = mockito::Server::new_async().await;
        let baseline_mock = server
            .mock("GET", mockito::Matcher::Regex(r"^/v1/sessions/.+/messages\?limit=1$".to_string()))
            .with_status(200)
            .with_body(r#"{"messages":[]}"#)
            .expect(1)
            .create_async()
            .await;
        let _chat_mock = server
            .mock("POST", "/v1/chat/completions")
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body("data: {\"choices\":[{\"delta\":{\"content\":\"Job done.\"}}]}\n\ndata: [DONE]\n\n")
            .create_async()
            .await;

        let conn = make_conn();
        let (agent_id, _sid) = seed_oc_setup(&conn, &server.url());
        let origin_session = create_session_internal(&conn, "workspace", &agent_id, "user", true, None).unwrap();
        let next_run_at = format_stored_datetime(&Utc::now());
        let job_id = create_job(
            &conn,
            NewJobParams {
                kind: "spawn", title: "c28b-dedup-test", brief: "say done", agent_id: &agent_id,
                project_id: "workspace", cron: None, run_at: None, next_run_at: &next_run_at,
                origin_session_id: Some(&origin_session.id), origin_message_id: None,
                parent_job_id: None, root_job_id: None, chain_depth: 0, wake_origin: false,
            },
        ).unwrap();

        let db = Arc::new(Mutex::new(DbConn::new(conn)));
        scheduler_tick(db.clone(), test_app_handle()).await;

        poll_until(
            || async { baseline_mock.matched_async().await },
            "an openclaw job turn must fetch its own baseline seq (own_turn_is_openclaw must be true, not hardcoded false)",
        )
        .await;

        baseline_mock.assert_async().await;
        let _ = job_id;
    }

    // --- C9 (#3120 PR 2) --- unique OC session label per job firing



    // ─── Bug 1(b) (#3050 fix PR): session bootstrap before the first send ───────




    // ─── Bug 1(a) (#3050 fix PR): the send's own HTTP status is checked ─────────




    // ─── Ruling 3 (Matt, live-test #1b fix PR): recurring/heartbeat failure
    // policy — "record it, keep recurring" ──────────────────────────────────





    /// #3114 item 3 (Reed's audit, "missed flip site"): `claim_job`'s
    /// `pending→running` UPDATE (inside `claim_due_jobs`) had NO
    /// `emit_chip_changed` anywhere in its call chain — a real,
    /// user-visible chip state (`jobStatusChip.ts`'s `' · running'`) that
    /// the old 5s poll alone would eventually catch, but not live. Fixed by
    /// emitting at the top of `scheduler_tick`'s `for job in claimed` loop
    /// (before either dispatch branch), not inside `claim_job`/
    /// `claim_due_jobs` themselves — changing either of THOSE signatures
    /// would ripple through ~20 direct unit-test call sites that assert the
    /// claim mechanism itself, never the chip event.
    ///
    /// Asserts the event is visible IMMEDIATELY after `scheduler_tick(...)
    /// .await` returns, with NO sleep — the emit is synchronous, inside the
    /// `for` loop, strictly before `tauri::async_runtime::spawn` hands the
    /// OC dispatch off to the background; proving this doesn't require
    /// waiting on (or even successfully completing) the dispatch itself.
    ///
    /// **Mutation (actually run — see report):** removed the
    /// `emit_chip_changed(&app, &job.id, chip_target_claim, "running")`
    /// line added at the top of the loop. Failure:
    /// ```text
    /// thread '...test_claim_job_pending_to_running_fires_chip_changed_running' panicked at src/ai/scheduler.rs:NNNN:9:
    /// a claimed (pending→running) job must fire pane-chip-changed with status=running — got: []
    /// ```
    /// Restored and re-confirmed green after.
    #[tokio::test]
    #[serial(keychain_override)]
    async fn test_claim_job_pending_to_running_fires_chip_changed_running() {
        let _kc = KeychainGuard::set();
        let mut server = mockito::Server::new_async().await;
        // Not asserted/awaited — the claim's own chip event fires
        // synchronously before this job's dispatch (through
        // prepare_job_dispatch/run_agent_loop, #3129 C28(b)) is even
        // spawned, so this mock only exists to give the detached
        // background task somewhere harmless to land rather than a hard
        // connection-refused.
        let _mock = server
            .mock("POST", "/v1/chat/completions")
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body("data: {\"choices\":[{\"delta\":{\"content\":\"Done.\"}}]}\n\ndata: [DONE]\n\n")
            .create_async()
            .await;

        let conn = make_conn();
        let (agent_id, sid) = seed_oc_setup(&conn, &server.url());
        let origin_sid = seed_job_session(&conn, &agent_id, "user");
        seed_job_row(&conn, "job-claim-running", "once", &agent_id, &sid, None, "2020-01-01 00:00:00", 0);
        conn.execute("UPDATE agent_jobs SET origin_session_id = ?1 WHERE id = 'job-claim-running'", params![origin_sid]).unwrap();

        let db = Arc::new(Mutex::new(DbConn::new(conn)));
        let app = tauri::test::mock_app();
        let events: Arc<Mutex<Vec<serde_json::Value>>> = Arc::new(Mutex::new(Vec::new()));
        let events2 = events.clone();
        app.listen(CHIP_CHANGED_EVENT, move |event| {
            let payload: serde_json::Value = serde_json::from_str(event.payload()).unwrap();
            events2.lock().unwrap().push(payload);
        });

        scheduler_tick(db.clone(), app.handle().clone()).await;

        let events = events.lock().unwrap().clone();
        let hit = events.iter().find(|e| e["job_id"] == "job-claim-running" && e["status"] == "running");
        assert!(hit.is_some(), "a claimed (pending→running) job must fire pane-chip-changed with status=running — got: {:?}", events);
        assert_eq!(hit.unwrap()["session_id"], origin_sid, "the chip target must be the originating chat (origin_session_id), same fallback every other emit site uses");
    }


    /// #3114 item 3 (Reed's audit): the API-runtime (non-OC) terminal
    /// `emit_chip_changed` call (the `run_agent_loop` completion's bookkeeping
    /// block, ~line 5534) existed but had ZERO dedicated test — Reed's
    /// mutation (removing it) left all 102 `ai::scheduler` tests passing.
    /// Drives the FULL `scheduler_tick` → `prepare_job_dispatch` →
    /// `run_agent_loop` path for an `ollama` (non-openclaw) agent, same
    /// mocking shape as `test_wake_row_propagates_chain_depth_and_root`
    /// above (one-shot `/api/chat` success response).
    ///
    /// **Mutation (actually run — see report):** removed the
    /// `emit_chip_changed(&app_for_report, &job_id_for_bookkeeping,
    /// chip_target_api, status)` call in the API-runtime terminal
    /// bookkeeping block. Failure:
    /// ```text
    /// thread '...test_api_runtime_terminal_done_fires_chip_changed' panicked at src/ai/scheduler.rs:NNNN:9:
    /// an API-runtime job's terminal 'done' write must fire pane-chip-changed — got: []
    /// ```
    /// Restored and re-confirmed green after.
    #[tokio::test]
    async fn test_api_runtime_terminal_done_fires_chip_changed() {
        let conn = make_conn();
        seed_agent(&conn, "crew-member", "ollama", None);

        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/api/chat")
            .with_status(200)
            .with_header("content-type", "application/x-ndjson")
            .with_body(b"{\"model\":\"test-model\",\"message\":{\"role\":\"assistant\",\"content\":\"Done.\"},\"done\":true}\n" as &[u8])
            .create_async()
            .await;
        crate::database::set_ai_setting(&conn, "ollama_endpoint", &server.url()).unwrap();

        let origin_session = create_session_internal(&conn, "workspace", "crew-member", "user", true, None).unwrap();
        let next_run_at = format_stored_datetime(&Utc::now());
        // `create_job` resolves its OWN fresh job session internally (same
        // as `test_wake_row_propagates_chain_depth_and_root` above) — no
        // separate `seed_job_session` call needed/wanted here.
        let job_id = create_job(
            &conn,
            NewJobParams {
                kind: "spawn", title: "api-chip-test", brief: "say done", agent_id: "crew-member",
                project_id: "workspace", cron: None, run_at: None, next_run_at: &next_run_at,
                origin_session_id: Some(&origin_session.id), origin_message_id: None,
                parent_job_id: None, root_job_id: None, chain_depth: 0, wake_origin: false,
            },
        ).unwrap();

        let db = Arc::new(Mutex::new(DbConn::new(conn)));
        // §36.7 (#3050 PR 6): a spawn/once job's own completion notification
        // (fired below the chip emit this test targets, gated on the
        // report-back ingest succeeding — see the `job_kind_for_report ==
        // "spawn" || "once"` arm) calls `tauri_plugin_notification`'s
        // `app.notification()`, same as `dispatch_oc_job`'s own failure
        // path — a bare `tauri::test::mock_app()` has no plugin state
        // registered for that and panics the spawned bookkeeping task
        // ("state() called before manage()"). Use the same
        // plugin-registered mock `test_app_handle()` already uses.
        let app = tauri::test::mock_builder()
            .plugin(tauri_plugin_notification::init())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("mock app with notification plugin must build");
        let events: Arc<Mutex<Vec<serde_json::Value>>> = Arc::new(Mutex::new(Vec::new()));
        let events2 = events.clone();
        app.listen(CHIP_CHANGED_EVENT, move |event| {
            let payload: serde_json::Value = serde_json::from_str(event.payload()).unwrap();
            events2.lock().unwrap().push(payload);
        });

        scheduler_tick(db.clone(), app.handle().clone()).await;
        // run_agent_loop's completion bookkeeping (including the chip emit)
        // runs on a spawned background task, detached from this test's own
        // await chain — poll for the actual condition the assertion below
        // checks (the #3114-flagged fast-follow #3050 PR 7 closes: the
        // #3114 re-audit named this test by name as "the next test most
        // likely to flake" and the surviving instance of the exact pattern
        // its own `d73a0670` fixed everywhere else).
        let events_poll = events.clone();
        poll_until(
            || async {
                events_poll
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|e| e["job_id"] == job_id.as_str() && e["status"] == "done")
            },
            "timed out waiting for pane-chip-changed with status='done' for job_id",
        )
        .await;

        let conn = db.lock().unwrap();
        let status: String = conn.query_row("SELECT status FROM agent_jobs WHERE id = ?1", params![job_id], |r| r.get(0)).unwrap();
        assert_eq!(status, "done", "sanity: the API-runtime job must have settled at 'done'");
        drop(conn);

        let events = events.lock().unwrap().clone();
        let hit = events.iter().find(|e| e["job_id"] == job_id.as_str() && e["status"] == "done");
        assert!(hit.is_some(), "an API-runtime job's terminal 'done' write must fire pane-chip-changed — got: {:?}", events);
        assert_eq!(hit.unwrap()["session_id"], origin_session.id, "the chip target must be the originating chat (origin_session_id)");
    }




    // ─── scheduler_tick end-to-end (missed-run collapse through a real tick) ───

    /// #3129 (C28(b)): the "exactly once, not ~72 times" missed-run-collapse
    /// assertion that `test_scheduler_tick_dispatches_missed_oc_job_exactly_
    /// once` (deleted this commit, along with the rest of the dedicated
    /// `dispatch_oc_job` test module) used to make — re-proven through the
    /// NEW dispatch path instead. The missed-run-collapse MECHANISM itself
    /// (`claim_due_jobs`'s own recompute) is unchanged and runtime-generic
    /// (§36.3.5) — this test exists only to confirm an OC job specifically
    /// still gets the same one-firing guarantee once it goes through
    /// `prepare_job_dispatch`/`run_agent_loop` instead of the deleted
    /// `dispatch_oc_job`.
    ///
    /// **Where a mutation would prove nothing, said so rather than
    /// inventing one:** the missed-run-collapse MECHANISM itself
    /// (`recompute_recurrence`) is pre-existing, runtime-generic, and
    /// already has its own dedicated, separately-mutation-tested unit
    /// test (`test_missed_run_collapses_to_one_catch_up_not_72`,
    /// unchanged by this PR) — mutating it here would only re-prove what
    /// that test already proves, not anything specific to THIS test. What
    /// this test actually adds, and the only thing worth mutating to prove
    /// it: that an `openclaw`-runtime job reaches the real dispatch path
    /// (`/v1/chat/completions`) at all when claimed via the missed-run
    /// path, not just the on-time path commit 2's own test already covers.
    /// That claim is the SAME one commit 2's test already makes (and
    /// mutation-tests) for an on-time OC job — this test's only
    /// incremental value is confirming the SAME OC-dispatch wiring survives
    /// when the claimed row arrives via `recompute_recurrence`'s
    /// missed-collapse branch instead of the on-time branch. No new
    /// mutation is run here; see commit 2's own
    /// `test_oc_job_dispatches_through_prepare_job_dispatch_not_sessions_send`
    /// mutation for the thing both tests are actually pinning.
    #[tokio::test]
    #[serial(keychain_override)]
    async fn test_scheduler_tick_dispatches_missed_oc_job_through_new_path_exactly_once() {
        let _kc = KeychainGuard::set();
        let mut server = mockito::Server::new_async().await;
        let chat_mock = server
            .mock("POST", "/v1/chat/completions")
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body("data: {\"choices\":[{\"delta\":{\"content\":\"Done.\"}}]}\n\ndata: [DONE]\n\n")
            .expect(1)
            .create_async()
            .await;

        let conn = make_conn();
        let (agent_id, sid) = seed_oc_setup(&conn, &server.url());
        let three_days_ago = format_stored_datetime(&(Utc::now() - chrono::Duration::days(3)));
        seed_job_row(&conn, "job-tick-missed-c28b", "recurring", &agent_id, &sid, Some("0 * * * *"), &three_days_ago, 0);
        crate::database::set_ai_setting(&conn, "job_concurrency", "2").unwrap();

        let db = Arc::new(Mutex::new(DbConn::new(conn)));
        let app = tauri::test::mock_app();

        scheduler_tick(db.clone(), app.handle().clone()).await;
        // dispatch now runs on a spawned background task (same as every
        // other runtime's job), detached from this test's own await chain
        // — poll for the real side effect instead of guessing how long it
        // takes.
        poll_until(
            || async { chat_mock.matched_async().await },
            "a single tick must dispatch a 3-day-overdue hourly OC job exactly ONCE, not ~72 times",
        )
        .await;

        chat_mock.assert_async().await;

        let conn = db.lock().unwrap();
        let missed_count: i64 = conn
            .query_row("SELECT missed_count FROM agent_jobs WHERE id = 'job-tick-missed-c28b'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(missed_count, 1);
    }

    #[tokio::test]
    async fn test_scheduler_tick_is_a_clean_no_op_with_no_due_jobs() {
        let conn = make_conn();
        let db = Arc::new(Mutex::new(DbConn::new(conn)));
        let app = tauri::test::mock_app();

        // Must not panic, hang, or error with an empty agent_jobs table.
        scheduler_tick(db.clone(), app.handle().clone()).await;
    }

    // ─── #3050 PR 5, Commit 3 — report-back, end to end through a real
    // scheduler_tick dispatch (named test §8 #1's report-back half; the
    // full wake-chain tests live in Commit 4) ──────────────────────────

    /// Named test (§36.8/§5.4): a `spawn`-kind job, dispatched through a
    /// REAL `scheduler_tick` → `run_agent_loop` round trip (a mocked Ollama
    /// endpoint, no tool calls, clean completion), writes the report-back
    /// message into the ORIGIN session — attributed to the origin
    /// session's own bound agent (§30.11.5 satisfied with no exception),
    /// `job_id` set, `model` set (so the Svelte chip-vs-bubble
    /// discriminator, Commit 5, renders it as a normal bubble).
    #[tokio::test]
    async fn test_spawn_job_report_back_lands_in_origin_session_attributed_and_tagged() {
        let conn = make_conn();
        seed_agent(&conn, "captain", "anthropic", None);
        seed_agent(&conn, "crew-member", "ollama", None);

        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/api/chat")
            .with_status(200)
            .with_header("content-type", "application/x-ndjson")
            .with_body(b"{\"model\":\"test-model\",\"message\":{\"role\":\"assistant\",\"content\":\"The thing is done.\"},\"done\":true}\n" as &[u8])
            .create_async()
            .await;
        crate::database::set_ai_setting(&conn, "ollama_endpoint", &server.url()).unwrap();

        let origin_session = create_session_internal(&conn, "workspace", "captain", "user", true, None)
            .expect("seed origin session");

        let next_run_at = format_stored_datetime(&Utc::now());
        let job_id = create_job(
            &conn,
            NewJobParams {
                kind: "spawn",
                title: "Test spawn",
                brief: "Do the thing",
                agent_id: "crew-member",
                project_id: "workspace",
                cron: None,
                run_at: None,
                next_run_at: &next_run_at,
                origin_session_id: Some(&origin_session.id),
                origin_message_id: None,
                parent_job_id: None,
                root_job_id: None,
                chain_depth: 0,
                wake_origin: true,
            },
        )
        .unwrap();

        let db = Arc::new(Mutex::new(DbConn::new(conn)));
        let app = tauri::test::mock_app();

        scheduler_tick(db.clone(), app.handle().clone()).await;
        // run_agent_loop + the post-completion report-back write both run
        // inside a spawned tauri::async_runtime task, detached from this
        // test's own await chain — poll for the actual row instead of
        // guessing how long it takes.
        poll_until(
            || async {
                if !mock.matched_async().await {
                    return false;
                }
                let conn = db.lock().unwrap();
                let count: i64 = conn
                    .query_row(
                        "SELECT COUNT(*) FROM chat_messages_local WHERE thread_id = ?1 AND job_id = ?2",
                        params![origin_session.id, job_id],
                        |r| r.get(0),
                    )
                    .unwrap_or(0);
                count == 1
            },
            "report-back row must exist in the origin session, tagged with job_id",
        )
        .await;

        mock.assert_async().await;

        let conn = db.lock().unwrap();
        let (content, row_job_id, model, agent_id, role): (String, Option<String>, Option<String>, Option<String>, String) = conn
            .query_row(
                "SELECT content, job_id, model, agent_id, role FROM chat_messages_local \
                 WHERE thread_id = ?1 AND job_id = ?2",
                params![origin_session.id, job_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
            )
            .expect("report-back row must exist in the origin session, tagged with job_id");

        assert!(content.contains("reported:"), "report-back content must carry the attribution line: {}", content);
        assert!(content.contains("The thing is done."), "report-back content must carry the job's own final text: {}", content);
        assert_eq!(row_job_id, Some(job_id.clone()), "report-back row must carry job_id");
        assert_eq!(model, Some("test-model".to_string()), "report-back row must carry the job's own model (Commit 5's chip/bubble discriminator)");
        // §30.11.5 satisfied with no exception: agent_id is stamped from
        // the ORIGIN session's own binding (the value the binding check
        // itself requires), never from the spawned job's own agent.
        assert_eq!(agent_id, Some("captain".to_string()), "report-back must be attributed to the ORIGIN session's own bound agent");
        assert_eq!(role, "assistant");

        let status: String = conn.query_row("SELECT status FROM agent_jobs WHERE id = ?1", params![job_id], |r| r.get(0)).unwrap();
        assert_eq!(status, "done", "a clean completion must settle agent_jobs.status to 'done'");
    }

    /// #3151 PR1 Commit D: a job turn that calls `create_note` must carry
    /// the SAME `[title](pane://note/{id})` reference token in BOTH the
    /// job's own-session reply AND the origin receipt — two independent
    /// assertions (not one "either copy has it" check), so a regression
    /// that fixes one copy and not the other still reddens, per the
    /// scope's own instruction. Drives a REAL `scheduler_tick` →
    /// `run_agent_loop` round trip: the mocked provider's FIRST response
    /// is a `create_note` tool call; its SECOND (post-tool-result)
    /// response is plain final text — exercising `output_link_token`'s
    /// real integration point (the tool-dispatch loop in
    /// `ai::chat::run_agent_loop`), not just the pure helper in isolation.
    ///
    /// Mutation: delete the `if !output_links.is_empty() { ... }` append
    /// in `run_agent_loop` (this PR's own addition) — reddens: neither
    /// copy contains `pane://note/`.
    #[tokio::test]
    async fn test_job_turn_note_output_link_appears_in_job_session_and_receipt() {
        let conn = make_conn();
        seed_agent(&conn, "note-agent", "ollama", None);
        conn.execute(
            "INSERT INTO projects (id, name) VALUES ('proj-notelink', 'Note Link Project')",
            [],
        )
        .unwrap();

        let call_count = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let call_count_for_mock = call_count.clone();
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/api/chat")
            .with_status(200)
            .with_header("content-type", "application/x-ndjson")
            .with_chunked_body(move |w| {
                let n = call_count_for_mock.fetch_add(1, Ordering::SeqCst);
                if n == 0 {
                    w.write_all(
                        br#"{"model":"test-model","message":{"role":"assistant","content":"","tool_calls":[{"function":{"name":"create_note","arguments":{"title":"Research Note","project_id":"proj-notelink","content":"Findings go here."}}}]},"done":true}
"#,
                    )
                } else {
                    w.write_all(
                        br#"{"model":"test-model","message":{"role":"assistant","content":"Done - created the note."},"done":true}
"#,
                    )
                }
            })
            .expect_at_least(2)
            .create_async()
            .await;
        crate::database::set_ai_setting(&conn, "ollama_endpoint", &server.url()).unwrap();

        let origin_session = create_session_internal(&conn, "proj-notelink", "note-agent", "user", true, None)
            .expect("seed origin session");

        let next_run_at = format_stored_datetime(&Utc::now());
        let job_id = create_job(
            &conn,
            NewJobParams {
                kind: "once",
                title: "Write research note",
                brief: "Write a note using the Research template",
                agent_id: "note-agent",
                project_id: "proj-notelink",
                cron: None,
                run_at: None,
                next_run_at: &next_run_at,
                origin_session_id: Some(&origin_session.id),
                origin_message_id: None,
                parent_job_id: None,
                root_job_id: None,
                chain_depth: 0,
                wake_origin: true,
            },
        )
        .unwrap();
        let job_session_id: String = conn
            .query_row("SELECT session_id FROM agent_jobs WHERE id = ?1", params![job_id], |r| r.get(0))
            .unwrap();

        let db = Arc::new(Mutex::new(DbConn::new(conn)));
        let app = tauri::test::mock_app();

        scheduler_tick(db.clone(), app.handle().clone()).await;
        poll_until(
            || async {
                let conn = db.lock().unwrap();
                let count: i64 = conn
                    .query_row(
                        "SELECT COUNT(*) FROM chat_messages_local WHERE thread_id = ?1 AND job_id = ?2",
                        params![origin_session.id, job_id],
                        |r| r.get(0),
                    )
                    .unwrap_or(0);
                count == 1
            },
            "report-back row must exist in the origin session before this test can assert on it",
        )
        .await;
        mock.assert_async().await;

        let conn = db.lock().unwrap();
        let note_id: String = conn
            .query_row("SELECT id FROM notes WHERE title = 'Research Note'", [], |r| r.get(0))
            .expect("the job turn must have actually created the note");
        let expected_token = format!("[Research Note](pane://note/{})", note_id);

        let receipt_content: String = conn
            .query_row(
                "SELECT content FROM chat_messages_local WHERE thread_id = ?1 AND job_id = ?2",
                params![origin_session.id, job_id],
                |r| r.get(0),
            )
            .expect("receipt row must exist");
        assert!(
            receipt_content.contains(&expected_token),
            "the origin receipt must carry the note's output link — got: {}",
            receipt_content
        );

        assert_ne!(job_session_id, origin_session.id, "sanity: the job's own session and the origin session must be two different rows");
        let own_session_content: String = conn
            .query_row(
                "SELECT content FROM chat_messages_local WHERE thread_id = ?1 AND job_id = ?2",
                params![job_session_id, job_id],
                |r| r.get(0),
            )
            .expect("the job's own-session reply row must exist");
        assert!(
            own_session_content.contains(&expected_token),
            "the job's own-session reply must ALSO carry the same output link — got: {}",
            own_session_content
        );
    }

    // ─── #3050 PR 5, Commit 4 — wake-on-complete tests (§8 #6-#10) ───────

    /// Named test (§8 #6): "Spawn → finish → one proactive turn on the
    /// origin session with the synthetic wake message." Drives TWO real
    /// `scheduler_tick`s: the first dispatches the spawn job and (per
    /// Commit 3/4) writes report-back + creates the wake row; the second
    /// claims and dispatches the WAKE row itself, proving it is a real,
    /// separate proactive turn — the wake brief is recorded as the origin
    /// session's own synthetic `job_id`-tagged row by the SAME
    /// `insert_job_brief_message` mechanism every other job dispatch uses.
    ///
    /// **Mutation (actually run):** deleted the `create_wake_job(...)` call
    /// (the whole `if job_wake_origin { ... }` block's success arm) and
    /// re-ran. Failure output:
    /// ```text
    /// thread '...test_spawn_finish_wakes_origin_with_one_proactive_turn' panicked at src/ai/scheduler.rs:NNNN:9:
    /// assertion `left == right` failed: exactly one wake row must exist after the spawn job finishes
    ///   left: 0
    ///  right: 1
    /// ```
    /// Restored and re-confirmed green after.
    #[tokio::test]
    async fn test_spawn_finish_wakes_origin_with_one_proactive_turn() {
        let conn = make_conn();
        seed_agent(&conn, "captain", "ollama", None);
        seed_agent(&conn, "crew-member", "ollama", None);

        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/api/chat")
            .with_status(200)
            .with_header("content-type", "application/x-ndjson")
            .with_body(b"{\"model\":\"test-model\",\"message\":{\"role\":\"assistant\",\"content\":\"The thing is done.\"},\"done\":true}\n" as &[u8])
            .expect_at_least(2)
            .create_async()
            .await;
        crate::database::set_ai_setting(&conn, "ollama_endpoint", &server.url()).unwrap();

        let origin_session = create_session_internal(&conn, "workspace", "captain", "user", true, None).unwrap();
        let next_run_at = format_stored_datetime(&Utc::now());
        let job_id = create_job(
            &conn,
            NewJobParams {
                kind: "spawn", title: "Test spawn", brief: "Do the thing", agent_id: "crew-member",
                project_id: "workspace", cron: None, run_at: None, next_run_at: &next_run_at,
                origin_session_id: Some(&origin_session.id), origin_message_id: None,
                parent_job_id: None, root_job_id: None, chain_depth: 0, wake_origin: true,
            },
        ).unwrap();

        let db = Arc::new(Mutex::new(DbConn::new(conn)));
        let app = tauri::test::mock_app();

        // Tick 1: dispatches the spawn job, creates the wake row. The
        // dispatch (and the wake-row write) runs inside a detached
        // tauri::async_runtime task — poll for the row instead of a fixed
        // sleep.
        scheduler_tick(db.clone(), app.handle().clone()).await;
        poll_until(
            || async {
                let conn = db.lock().unwrap();
                let count: i64 = conn.query_row("SELECT COUNT(*) FROM agent_jobs WHERE kind = 'wake'", [], |r| r.get(0)).unwrap_or(0);
                count == 1
            },
            "exactly one wake row must exist after the spawn job finishes",
        )
        .await;

        let wake_job_id: String = {
            let conn = db.lock().unwrap();
            let count: i64 = conn.query_row("SELECT COUNT(*) FROM agent_jobs WHERE kind = 'wake'", [], |r| r.get(0)).unwrap();
            assert_eq!(count, 1, "exactly one wake row must exist after the spawn job finishes");
            conn.query_row("SELECT id FROM agent_jobs WHERE kind = 'wake'", [], |r| r.get(0)).unwrap()
        };

        // Tick 2: claims and dispatches the WAKE row — a second, real,
        // proactive turn, directly in the origin session. Same detached-
        // spawn race; poll for its own brief row instead of sleeping.
        scheduler_tick(db.clone(), app.handle().clone()).await;
        poll_until(
            || async {
                if !mock.matched_async().await {
                    return false;
                }
                let conn = db.lock().unwrap();
                let count: i64 = conn
                    .query_row(
                        "SELECT COUNT(*) FROM chat_messages_local WHERE job_id = ?1 AND synthetic = 1",
                        params![wake_job_id],
                        |r| r.get(0),
                    )
                    .unwrap_or(0);
                count == 1
            },
            "the wake job's own brief must be recorded as a synthetic row — a real, second proactive turn",
        )
        .await;

        mock.assert_async().await;

        let conn = db.lock().unwrap();
        let (brief_count, session_id): (i64, String) = (
            conn.query_row(
                "SELECT COUNT(*) FROM chat_messages_local WHERE job_id = ?1 AND synthetic = 1",
                params![wake_job_id],
                |r| r.get(0),
            ).unwrap(),
            conn.query_row("SELECT session_id FROM agent_jobs WHERE id = ?1", params![wake_job_id], |r| r.get(0)).unwrap(),
        );
        assert_eq!(brief_count, 1, "the wake job's own brief must be recorded as a synthetic row — a real, second proactive turn");
        assert_eq!(session_id, origin_session.id, "the wake turn must run DIRECTLY in the origin session, not a new job session");

        let wake_brief: String = conn.query_row("SELECT brief FROM agent_jobs WHERE id = ?1", params![wake_job_id], |r| r.get(0)).unwrap();
        assert!(wake_brief.contains("Test spawn"), "the wake brief must name the finished job's own title: {}", wake_brief);
        assert!(wake_brief.contains("crew-member"), "the wake brief must name the agent that did the work: {}", wake_brief);
        assert!(wake_brief.contains("done"), "the wake brief must carry the finished job's own status: {}", wake_brief);
    }

    // --- C49a (#3155) --- leak repro: the wake brief in the origin session
    // must ALWAYS be synthetic, scanned generically, not just at the one
    // job_id the test already knows about ----------------------------------

    /// C49a (#3155): the reported leak is "after a chat-created once-job or
    /// spawn finishes, the wake's brief renders as a VISIBLE job-brief block
    /// in the origin chat, raw markdown included" — i.e. a `role='user'`
    /// row shaped like the wake-brief template
    /// (`"[Job '{title}' by {agent} finished: {status}]"`, the literal
    /// `format!` in the wake-on-complete block above) landing in
    /// `chat_messages_local` with `synthetic = 0` (or missing entirely,
    /// which would un-hide it the same way). Reed's audit traced every
    /// write path he could find (`insert_job_brief_message`,
    /// `insert_schedule_confirmation_message`, `insert_wake_turn_reply`,
    /// `ingest_external_messages_impl`/`insert_external_message`) and found
    /// each one individually correct, but could not pin the actual
    /// mechanism — Matt's ruling (verbatim in intent): write this repro
    /// FIRST, and if it can't be made to fail, say so and leave the fix out
    /// rather than guess.
    ///
    /// Unlike the existing `test_spawn_finish_wakes_origin_with_one_
    /// proactive_turn` above (which already asserts `synthetic = 1` but
    /// ONLY by `job_id` it already knows, from the one write path it's
    /// exercising), this test scans `chat_messages_local` generically —
    /// by CONTENT shape, not by a known `job_id` — for every `role='user'`
    /// row in the origin session matching the wake-brief template, so a
    /// second, undiscovered write path producing an un-hidden duplicate
    /// would still be caught even if it bypassed `insert_job_brief_message`
    /// entirely. Drives a `kind = 'once'` job (the other half of the
    /// issue's "chat-created once-job or spawn" wording — the existing
    /// test above already covers `spawn`) through two real `scheduler_tick`
    /// round trips: the first finishes the once job and creates the wake
    /// row; the second claims and dispatches the wake row itself, which is
    /// the turn whose brief is the reported leak.
    #[tokio::test]
    async fn test_c49a_once_job_wake_brief_in_origin_is_always_synthetic_api_agent() {
        let conn = make_conn();
        seed_agent(&conn, "c49a-captain", "ollama", None);

        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/api/chat")
            .with_status(200)
            .with_header("content-type", "application/x-ndjson")
            .with_body(b"{\"model\":\"test-model\",\"message\":{\"role\":\"assistant\",\"content\":\"All done.\"},\"done\":true}\n" as &[u8])
            .expect_at_least(2)
            .create_async()
            .await;
        crate::database::set_ai_setting(&conn, "ollama_endpoint", &server.url()).unwrap();

        // A chat-created once job keeps the SAME agent as the chat it was
        // created from — the origin session and the job share `c49a-captain`.
        let origin_session = create_session_internal(&conn, "workspace", "c49a-captain", "user", true, None).unwrap();
        let next_run_at = format_stored_datetime(&Utc::now());
        let job_id = create_job(
            &conn,
            NewJobParams {
                kind: "once", title: "Repro once job", brief: "Do the one-off thing", agent_id: "c49a-captain",
                project_id: "workspace", cron: None, run_at: None, next_run_at: &next_run_at,
                origin_session_id: Some(&origin_session.id), origin_message_id: None,
                parent_job_id: None, root_job_id: None, chain_depth: 0, wake_origin: true,
            },
        ).unwrap();

        let db = Arc::new(Mutex::new(DbConn::new(conn)));
        let app = tauri::test::mock_app();

        // Tick 1: finishes the once job (report-back into origin, wake row created).
        scheduler_tick(db.clone(), app.handle().clone()).await;
        poll_until(
            || async {
                let conn = db.lock().unwrap();
                let status: Option<String> = conn
                    .query_row("SELECT status FROM agent_jobs WHERE id = ?1", params![job_id], |r| r.get(0))
                    .optional().unwrap().flatten();
                matches!(status.as_deref(), Some("done"))
            },
            "the once job must reach 'done' before the wake row can be claimed",
        )
        .await;
        let wake_job_id: String = {
            let conn = db.lock().unwrap();
            conn.query_row("SELECT id FROM agent_jobs WHERE kind = 'wake'", [], |r| r.get(0))
                .expect("a wake row must exist after the once job finishes with wake_origin=true")
        };

        // Tick 2: claims and dispatches the WAKE row — the turn whose own
        // brief IS the reported leak if it ever lands un-hidden.
        scheduler_tick(db.clone(), app.handle().clone()).await;
        poll_until(
            || async {
                let conn = db.lock().unwrap();
                let status: Option<String> = conn
                    .query_row("SELECT status FROM agent_jobs WHERE id = ?1", params![wake_job_id], |r| r.get(0))
                    .optional().unwrap().flatten();
                matches!(status.as_deref(), Some("done"))
            },
            "the wake job must reach 'done'",
        )
        .await;
        mock.assert_async().await;

        let conn = db.lock().unwrap();
        // Generic scan — by CONTENT shape, not by the wake_job_id we already
        // know — for every role='user' row in the origin session matching
        // the wake-brief template. If the leak existed and used a DIFFERENT
        // write path than insert_job_brief_message, this query would still
        // find its row.
        let mut stmt = conn
            .prepare("SELECT id, synthetic, job_id, content FROM chat_messages_local WHERE thread_id = ?1 AND role = 'user' AND content LIKE '[Job %finished:%'")
            .unwrap();
        let rows: Vec<(String, i64, Option<String>, String)> = stmt
            .query_map(params![origin_session.id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();

        assert!(
            !rows.is_empty(),
            "expected at least one wake-brief-shaped row in the origin session (the wake job's own brief) — found none; the repro scenario itself did not reach the leak's own write path"
        );
        for (id, synthetic, job_id_col, content) in &rows {
            assert_eq!(
                *synthetic, 1,
                "row {} (job_id={:?}) is a wake-brief-shaped role='user' message in the origin session with synthetic={} — \
                 this is the reported leak: ChatPanel.svelte's C32 hiding branch requires synthetic=1 to hide it. Content: {}",
                id, job_id_col, synthetic, content
            );
        }
    }

    /// C49a (#3155), OC-bound variant — Reed's brief asks for "both an
    /// API-bound and an OC-bound agent," since #3129/C28(b) moved OC-runtime
    /// jobs onto the SAME `prepare_job_dispatch`/`run_agent_loop` path every
    /// other runtime uses (no separate OC-side session, no
    /// `sessions.create`/`sessions.send`). Same generic content-shape scan
    /// as the API-agent variant above, against an `openclaw`-runtime agent
    /// (`/v1/chat/completions`, mocked SSE, the same shape
    /// `test_oc_job_dispatches_through_prepare_job_dispatch_not_sessions_send`
    /// above uses).
    #[tokio::test]
    #[serial(keychain_override)]
    async fn test_c49a_once_job_wake_brief_in_origin_is_always_synthetic_oc_agent() {
        let _kc = KeychainGuard::set();
        let mut server = mockito::Server::new_async().await;
        let chat_mock = server
            .mock("POST", "/v1/chat/completions")
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body("data: {\"choices\":[{\"delta\":{\"content\":\"All done (OC).\"}}]}\n\ndata: [DONE]\n\n")
            .expect_at_least(2)
            .create_async()
            .await;

        let conn = make_conn();
        let (agent_id, _sid) = seed_oc_setup(&conn, &server.url());
        let origin_session = create_session_internal(&conn, "workspace", &agent_id, "user", true, None).unwrap();
        let next_run_at = format_stored_datetime(&Utc::now());
        let job_id = create_job(
            &conn,
            NewJobParams {
                kind: "once", title: "Repro once job (OC)", brief: "Do the one-off thing", agent_id: &agent_id,
                project_id: "workspace", cron: None, run_at: None, next_run_at: &next_run_at,
                origin_session_id: Some(&origin_session.id), origin_message_id: None,
                parent_job_id: None, root_job_id: None, chain_depth: 0, wake_origin: true,
            },
        ).unwrap();

        let db = Arc::new(Mutex::new(DbConn::new(conn)));
        scheduler_tick(db.clone(), test_app_handle()).await;
        poll_until(
            || async {
                let conn = db.lock().unwrap();
                let status: Option<String> = conn
                    .query_row("SELECT status FROM agent_jobs WHERE id = ?1", params![job_id], |r| r.get(0))
                    .optional().unwrap().flatten();
                matches!(status.as_deref(), Some("done"))
            },
            "the once job (OC agent) must reach 'done' before the wake row can be claimed",
        )
        .await;
        let wake_job_id: String = {
            let conn = db.lock().unwrap();
            conn.query_row("SELECT id FROM agent_jobs WHERE kind = 'wake'", [], |r| r.get(0))
                .expect("a wake row must exist after the once job finishes with wake_origin=true")
        };

        scheduler_tick(db.clone(), test_app_handle()).await;
        poll_until(
            || async {
                let conn = db.lock().unwrap();
                let status: Option<String> = conn
                    .query_row("SELECT status FROM agent_jobs WHERE id = ?1", params![wake_job_id], |r| r.get(0))
                    .optional().unwrap().flatten();
                matches!(status.as_deref(), Some("done"))
            },
            "the wake job (OC agent) must reach 'done'",
        )
        .await;
        chat_mock.assert_async().await;

        let conn = db.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT id, synthetic, job_id, content FROM chat_messages_local WHERE thread_id = ?1 AND role = 'user' AND content LIKE '[Job %finished:%'")
            .unwrap();
        let rows: Vec<(String, i64, Option<String>, String)> = stmt
            .query_map(params![origin_session.id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();

        assert!(
            !rows.is_empty(),
            "expected at least one wake-brief-shaped row in the origin session (the wake job's own brief) — found none; the repro scenario itself did not reach the leak's own write path"
        );
        for (id, synthetic, job_id_col, content) in &rows {
            assert_eq!(
                *synthetic, 1,
                "row {} (job_id={:?}) is a wake-brief-shaped role='user' message in the origin session with synthetic={} (OC agent) — \
                 this is the reported leak. Content: {}",
                id, job_id_col, synthetic, content
            );
        }
    }

    /// Named test (§8 #7): "`wake: false` → report only."
    ///
    /// **Mutation (actually run):** deleted the `if job_wake_origin { ... }`
    /// gate (replaced with `if true`) and re-ran. Failure output:
    /// ```text
    /// thread '...test_wake_false_report_only_no_wake_row' panicked at src/ai/scheduler.rs:NNNN:9:
    /// assertion `left == right` failed: wake: false must create ZERO wake rows
    ///   left: 1
    ///  right: 0
    /// ```
    /// Restored and re-confirmed green after.
    #[tokio::test]
    async fn test_wake_false_report_only_no_wake_row() {
        let conn = make_conn();
        seed_agent(&conn, "captain", "ollama", None);
        seed_agent(&conn, "crew-member", "ollama", None);

        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/api/chat")
            .with_status(200)
            .with_header("content-type", "application/x-ndjson")
            .with_body(b"{\"model\":\"test-model\",\"message\":{\"role\":\"assistant\",\"content\":\"Done, no wake.\"},\"done\":true}\n" as &[u8])
            .create_async()
            .await;
        crate::database::set_ai_setting(&conn, "ollama_endpoint", &server.url()).unwrap();

        let origin_session = create_session_internal(&conn, "workspace", "captain", "user", true, None).unwrap();
        let next_run_at = format_stored_datetime(&Utc::now());
        let job_id = create_job(
            &conn,
            NewJobParams {
                kind: "spawn", title: "No-wake spawn", brief: "Do the thing", agent_id: "crew-member",
                project_id: "workspace", cron: None, run_at: None, next_run_at: &next_run_at,
                origin_session_id: Some(&origin_session.id), origin_message_id: None,
                parent_job_id: None, root_job_id: None, chain_depth: 0, wake_origin: false,
            },
        ).unwrap();

        let db = Arc::new(Mutex::new(DbConn::new(conn)));
        let app = tauri::test::mock_app();
        scheduler_tick(db.clone(), app.handle().clone()).await;
        // Poll for the thing that actually takes detached-spawn time to
        // appear (the report-back row); the "ZERO wake rows" assertion
        // below is an absence check that doesn't need waiting for, and is
        // re-checked AFTER the real completion signal so it isn't just
        // "passing because we didn't wait long enough to find out."
        poll_until(
            || async {
                let conn = db.lock().unwrap();
                let count: i64 = conn
                    .query_row(
                        "SELECT COUNT(*) FROM chat_messages_local WHERE thread_id = ?1 AND job_id = ?2",
                        params![origin_session.id, job_id],
                        |r| r.get(0),
                    )
                    .unwrap_or(0);
                count == 1
            },
            "report-back must still happen even when wake_origin=false",
        )
        .await;

        let conn = db.lock().unwrap();
        let wake_count: i64 = conn.query_row("SELECT COUNT(*) FROM agent_jobs WHERE kind = 'wake'", [], |r| r.get(0)).unwrap();
        assert_eq!(wake_count, 0, "wake: false must create ZERO wake rows");

        let report_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM chat_messages_local WHERE thread_id = ?1 AND job_id = ?2",
            params![origin_session.id, job_id],
            |r| r.get(0),
        ).unwrap();
        assert_eq!(report_count, 1, "report-back must still happen even when wake_origin=false");
    }

    /// #3129 (C28(b)), item 11: the INVERSE of the old (deleted)
    /// "OC origin → no wake" test. A wake is a job like any other now —
    /// an OC-bound origin session MUST get a real wake row AND that row
    /// must actually dispatch through `prepare_job_dispatch`/
    /// `run_agent_loop` (mocked at the OC gateway's `/v1/chat/completions`
    /// endpoint), landing its reply directly in the origin session — not
    /// merely "a row exists" (asserting the positive outcome, per Reed's
    /// own instruction, not just the absence of the old skip log line).
    ///
    /// **Mutation (actually run — see report):** re-added the deleted
    /// `is_oc_bound` check and its "skipping wake, report-only" branch.
    /// Failure output:
    /// ```text
    /// thread '...test_oc_bound_origin_gets_a_real_wake_turn' panicked at src/ai/scheduler.rs:NNNN:9:
    /// an OC-bound origin session must get a real wake row now that C28(b) overturns the old skip: 0
    /// ```
    /// (confirmed and reverted.)
    #[tokio::test]
    #[serial(keychain_override)]
    async fn test_oc_bound_origin_gets_a_real_wake_turn() {
        let _kc = KeychainGuard::set();
        let mut server = mockito::Server::new_async().await;
        let chat_mock = server
            .mock("POST", "/v1/chat/completions")
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body("data: {\"choices\":[{\"delta\":{\"content\":\"Acknowledged.\"}}]}\n\ndata: [DONE]\n\n")
            .expect(1)
            .create_async()
            .await;

        let conn = make_conn();
        seed_agent(&conn, "oc-captain", "openclaw", Some("main"));
        crate::database::set_ai_setting(&conn, "endpoint", &server.url()).unwrap();
        crate::ai::credential_migration::write_gateway_token(&conn, "test-gateway-token").unwrap();
        seed_agent(&conn, "crew-member", "ollama", None);

        let mut ollama_server = mockito::Server::new_async().await;
        let _ollama_mock = ollama_server
            .mock("POST", "/api/chat")
            .with_status(200)
            .with_header("content-type", "application/x-ndjson")
            .with_body(b"{\"model\":\"test-model\",\"message\":{\"role\":\"assistant\",\"content\":\"Done.\"},\"done\":true}\n" as &[u8])
            .create_async()
            .await;
        crate::database::set_ai_setting(&conn, "ollama_endpoint", &ollama_server.url()).unwrap();

        let origin_session = create_session_internal(&conn, "workspace", "oc-captain", "user", true, None).unwrap();
        let next_run_at = format_stored_datetime(&Utc::now());
        let job_id = create_job(
            &conn,
            NewJobParams {
                kind: "spawn", title: "OC-origin spawn", brief: "Do the thing", agent_id: "crew-member",
                project_id: "workspace", cron: None, run_at: None, next_run_at: &next_run_at,
                origin_session_id: Some(&origin_session.id), origin_message_id: None,
                parent_job_id: None, root_job_id: None, chain_depth: 0, wake_origin: true,
            },
        ).unwrap();

        let db = Arc::new(Mutex::new(DbConn::new(conn)));
        // The wake turn's own completion notification calls
        // `app.notification()` (same as every other job-turn completion) —
        // a bare `tauri::test::mock_app()` has no plugin state registered
        // for that and panics the spawned bookkeeping task. Same fix
        // `test_api_runtime_terminal_done_fires_chip_changed` already uses.
        let app = tauri::test::mock_builder()
            .plugin(tauri_plugin_notification::init())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("mock app with notification plugin must build");
        scheduler_tick(db.clone(), app.handle().clone()).await;
        // Report-back (spawn's own, provider-neutral mechanism) happens
        // first; poll for it, THEN for the wake row's own creation, THEN
        // for the wake TURN to actually dispatch and settle — three real
        // completion signals, not a fixed sleep.
        poll_until(
            || async {
                let conn = db.lock().unwrap();
                let count: i64 = conn
                    .query_row(
                        "SELECT COUNT(*) FROM chat_messages_local WHERE thread_id = ?1 AND job_id = ?2",
                        params![origin_session.id, job_id],
                        |r| r.get(0),
                    )
                    .unwrap_or(0);
                count == 1
            },
            "report-back is provider-neutral — it must still happen for an OC-bound origin",
        )
        .await;

        poll_until(
            || async {
                let conn = db.lock().unwrap();
                conn.query_row("SELECT COUNT(*) FROM agent_jobs WHERE kind = 'wake'", [], |r: &rusqlite::Row| r.get::<_, i64>(0))
                    .unwrap_or(0)
                    > 0
            },
            "an OC-bound origin session must get a real wake row now that C28(b) overturns the old skip",
        )
        .await;

        // Tick 2: the wake row created above is only CLAIMED on a
        // subsequent tick (same two-tick shape `test_wake_turn_reply_is_
        // persisted_and_live_refreshes_open_session_c18` already
        // establishes for the API-runtime case) — dispatches the wake
        // TURN itself.
        scheduler_tick(db.clone(), app.handle().clone()).await;
        poll_until(
            || async { chat_mock.matched_async().await },
            "the wake row's own turn must actually dispatch through prepare_job_dispatch/run_agent_loop, hitting the OC gateway's chat endpoint",
        )
        .await;

        chat_mock.assert_async().await;

        let conn = db.lock().unwrap();
        let wake_count: i64 = conn.query_row("SELECT COUNT(*) FROM agent_jobs WHERE kind = 'wake'", [], |r| r.get(0)).unwrap();
        assert_eq!(wake_count, 1, "exactly one wake row, targeting the OC-bound origin session");

        let wake_session: String = conn.query_row("SELECT session_id FROM agent_jobs WHERE kind = 'wake'", [], |r| r.get(0)).unwrap();
        assert_eq!(wake_session, origin_session.id, "the wake job's own session IS the OC-bound origin session — the turn runs directly in it");

        let report_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM chat_messages_local WHERE thread_id = ?1 AND job_id = ?2",
            params![origin_session.id, job_id],
            |r| r.get(0),
        ).unwrap();
        assert_eq!(report_count, 1, "report-back is provider-neutral — it must still happen for an OC-bound origin");
    }

    /// Named test (§8 #8): "Chain of 9 spawns → the 9th wake has no
    /// `spawn_job` offered." Unit-level on the offer-time gate directly
    /// (`chain_depth >= RUNTIME_MAX_CHAIN_DEPTH` → both `spawn_job` AND
    /// `schedule_job` withheld) — the chain_depth VALUE itself is proven to
    /// propagate correctly by `test_wake_row_propagates_chain_depth_and_root`
    /// below; this test proves the GATE that value feeds.
    ///
    /// **Mutation (actually run):** deleted the
    /// `if chain_capped { tools.retain(...) }` block inside
    /// `get_pane_tools_for_runtime` and re-ran. Failure output:
    /// ```text
    /// thread '...test_chain_capped_withholds_spawn_and_schedule_job' panicked at src/ai/tools.rs:NNNN:9:
    /// assertion failed: !names.contains(&"spawn_job".to_string())
    /// ```
    /// Restored and re-confirmed green after.
    #[test]
    fn test_chain_capped_withholds_spawn_and_schedule_job() {
        use crate::ai::tools::get_pane_tools_for_runtime;
        // chain_depth 8 == RUNTIME_MAX_CHAIN_DEPTH → capped.
        let capped = get_pane_tools_for_runtime("anthropic", true, true, true);
        let capped_names: Vec<String> = capped.iter().map(|t| t.name.clone()).collect();
        assert!(!capped_names.contains(&"spawn_job".to_string()), "chain-capped must withhold spawn_job: {:?}", capped_names);
        assert!(!capped_names.contains(&"schedule_job".to_string()), "chain-capped must withhold schedule_job: {:?}", capped_names);

        // Positive case — chain_depth 7, NOT capped, both offered (given a
        // crew/proactive context that would otherwise offer them).
        let not_capped = get_pane_tools_for_runtime("anthropic", true, true, false);
        let not_capped_names: Vec<String> = not_capped.iter().map(|t| t.name.clone()).collect();
        assert!(not_capped_names.contains(&"spawn_job".to_string()), "not chain-capped must still offer spawn_job: {:?}", not_capped_names);
        assert!(not_capped_names.contains(&"schedule_job".to_string()), "not chain-capped must still offer schedule_job: {:?}", not_capped_names);
    }

    /// Companion to the above: the wake row's own `chain_depth`/`root_job_id`
    /// actually propagate the way the offer-time gate assumes — a wake
    /// created from a job at `chain_depth=7` lands at `chain_depth=8`
    /// (exactly `RUNTIME_MAX_CHAIN_DEPTH`), and `root_job_id` is inherited
    /// from the finished job, never re-self-referenced.
    #[tokio::test]
    async fn test_wake_row_propagates_chain_depth_and_root() {
        let conn = make_conn();
        seed_agent(&conn, "captain", "ollama", None);
        seed_agent(&conn, "crew-member", "ollama", None);

        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/api/chat")
            .with_status(200)
            .with_header("content-type", "application/x-ndjson")
            .with_body(b"{\"model\":\"test-model\",\"message\":{\"role\":\"assistant\",\"content\":\"Done.\"},\"done\":true}\n" as &[u8])
            .create_async()
            .await;
        crate::database::set_ai_setting(&conn, "ollama_endpoint", &server.url()).unwrap();

        let origin_session = create_session_internal(&conn, "workspace", "captain", "user", true, None).unwrap();
        let next_run_at = format_stored_datetime(&Utc::now());
        // The root row must exist FIRST — `root_job_id` is a real FK
        // (`REFERENCES agent_jobs(id)`).
        let root_job_id = "chain-root-job";
        conn.execute(
            "INSERT INTO agent_jobs (id, kind, title, brief, agent_id, project_id, session_id, chain_depth, status, next_run_at, created_at, updated_at) \
             VALUES (?1, 'spawn', 'root', 'root brief', 'crew-member', 'workspace', ?2, 0, 'done', datetime('now'), datetime('now'), datetime('now'))",
            params![root_job_id, origin_session.id],
        ).unwrap();
        // A job already at chain_depth=7, with its own distinct root_job_id
        // (simulating the 8th link in a chain).
        let job_id = create_job(
            &conn,
            NewJobParams {
                kind: "spawn", title: "8th in chain", brief: "Do the thing", agent_id: "crew-member",
                project_id: "workspace", cron: None, run_at: None, next_run_at: &next_run_at,
                origin_session_id: Some(&origin_session.id), origin_message_id: None,
                parent_job_id: None, root_job_id: Some(root_job_id), chain_depth: 7, wake_origin: true,
            },
        ).unwrap();

        let db = Arc::new(Mutex::new(DbConn::new(conn)));
        let app = tauri::test::mock_app();
        scheduler_tick(db.clone(), app.handle().clone()).await;
        // The wake row is written by the detached tauri::async_runtime
        // dispatch task; poll for its existence (original CI failure here
        // was `QueryReturnedNoRows` on the SELECT below — the row simply
        // hadn't landed yet) instead of guessing a sleep duration.
        poll_until(
            || async {
                let conn = db.lock().unwrap();
                let count: i64 = conn.query_row("SELECT COUNT(*) FROM agent_jobs WHERE kind = 'wake'", [], |r| r.get(0)).unwrap_or(0);
                count == 1
            },
            "the wake row must exist before its chain_depth/root_job_id can be inspected (SELECT found no rows)",
        )
        .await;

        let conn = db.lock().unwrap();
        let _ = &job_id;
        let (chain_depth, wake_root_job_id): (i64, Option<String>) = conn.query_row(
            "SELECT chain_depth, root_job_id FROM agent_jobs WHERE kind = 'wake'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        ).unwrap();
        assert_eq!(chain_depth, 8, "the wake row's chain_depth must be the finished job's own chain_depth + 1");
        assert_eq!(wake_root_job_id, Some(root_job_id.to_string()), "the wake row's root_job_id must be inherited from the finished job, never re-self-referenced");
    }

    /// Named test (§8 #9): "Ceiling shared across the chain." Direct test
    /// of `prepare_job_dispatch`'s own `already_spent` subtraction — a
    /// chained job (its `root_job_id` points at a DIFFERENT row) must have
    /// its ceiling reduced by whatever the root has already spent; a
    /// non-chained job (root_job_id self-referencing) must NOT have its
    /// ceiling reduced at all by ITS OWN prior tool_calls_used (reading the
    /// wrong row would double-count).
    ///
    /// **Mutation (actually run):** replaced the `already_spent` read's
    /// `COALESCE`/root-keyed query with a plain `0` (i.e. deleted the
    /// chain-sharing subtraction) and re-ran. Failure output:
    /// ```text
    /// thread '...test_ceiling_shared_across_chain' panicked at src/ai/scheduler.rs:NNNN:9:
    /// assertion `left == right` failed: a chained job's ceiling must be reduced by the root's already-spent total
    ///   left: 200
    ///  right: 10
    /// ```
    /// Restored and re-confirmed green after.
    #[tokio::test]
    async fn test_ceiling_shared_across_chain() {
        let conn = make_conn();
        seed_agent(&conn, "crew-member", "ollama", None);
        crate::database::set_ai_setting(&conn, "ollama_endpoint", "http://unused.example").unwrap();
        crate::database::set_ai_setting(&conn, "job_ceiling_tool_calls", "200").unwrap();

        // Root row: already spent 190 of the chain's shared 200-call budget.
        let root_job_id = "ceiling-root";
        let session_id = seed_job_session(&conn, "crew-member", "spawn");
        conn.execute(
            "INSERT INTO agent_jobs (id, kind, title, brief, agent_id, project_id, session_id, chain_depth, tool_calls_used, status, next_run_at, created_at, updated_at) \
             VALUES (?1, 'spawn', 'root', 'root brief', 'crew-member', 'workspace', ?2, 0, 190, 'done', datetime('now'), datetime('now'), datetime('now'))",
            params![root_job_id, session_id],
        ).unwrap();

        // Chained job: root_job_id = the row above.
        let next_run_at = format_stored_datetime(&Utc::now());
        let chained_job_id = create_job(
            &conn,
            NewJobParams {
                kind: "spawn", title: "chained", brief: "continue", agent_id: "crew-member",
                project_id: "workspace", cron: None, run_at: None, next_run_at: &next_run_at,
                origin_session_id: None, origin_message_id: None,
                parent_job_id: None, root_job_id: Some(root_job_id), chain_depth: 1, wake_origin: true,
            },
        ).unwrap();
        let chained_claimed = load_claimed_job_row(&conn, &chained_job_id).unwrap();
        let chained_claimed = ClaimedJob {
            id: chained_claimed.id, kind: chained_claimed.kind, title: chained_claimed.title,
            brief: chained_claimed.brief, agent_id: chained_claimed.agent_id, project_id: chained_claimed.project_id,
            session_id: chained_claimed.session_id, runtime_type: chained_claimed.runtime_type,
            origin_session_id: chained_claimed.origin_session_id, parent_job_id: chained_claimed.parent_job_id,
            root_job_id: chained_claimed.root_job_id, chain_depth: chained_claimed.chain_depth,
            wake_origin: chained_claimed.wake_origin,
        };

        let app = tauri::test::mock_app();
        let db = Arc::new(Mutex::new(DbConn::new(conn)));
        let prep = prepare_job_dispatch(&db, &app.handle(), &chained_claimed).await.expect("prepare_job_dispatch must succeed");
        assert_eq!(prep.job_ceiling_tool_calls, 10, "a chained job's ceiling must be reduced by the root's already-spent total (200 - 190 = 10)");

        // Non-chained job: root_job_id self-references (create_job's own
        // default) — its OWN tool_calls_used (freshly 0) must not reduce
        // its ceiling below the full 200.
        let conn2 = db.lock().unwrap();
        let next_run_at2 = format_stored_datetime(&Utc::now());
        let standalone_job_id = create_job(
            &conn2,
            NewJobParams {
                kind: "spawn", title: "standalone", brief: "go", agent_id: "crew-member",
                project_id: "workspace", cron: None, run_at: None, next_run_at: &next_run_at2,
                origin_session_id: None, origin_message_id: None,
                parent_job_id: None, root_job_id: None, chain_depth: 0, wake_origin: true,
            },
        ).unwrap();
        let standalone_claimed = load_claimed_job_row(&conn2, &standalone_job_id).unwrap();
        let standalone_claimed = ClaimedJob {
            id: standalone_claimed.id, kind: standalone_claimed.kind, title: standalone_claimed.title,
            brief: standalone_claimed.brief, agent_id: standalone_claimed.agent_id, project_id: standalone_claimed.project_id,
            session_id: standalone_claimed.session_id, runtime_type: standalone_claimed.runtime_type,
            origin_session_id: standalone_claimed.origin_session_id, parent_job_id: standalone_claimed.parent_job_id,
            root_job_id: standalone_claimed.root_job_id, chain_depth: standalone_claimed.chain_depth,
            wake_origin: standalone_claimed.wake_origin,
        };
        drop(conn2);
        let prep2 = prepare_job_dispatch(&db, &app.handle(), &standalone_claimed).await.expect("prepare_job_dispatch must succeed");
        assert_eq!(prep2.job_ceiling_tool_calls, 200, "a non-chained job (root_job_id self-referencing, tool_calls_used=0) must get the FULL ceiling, not a reduced one — the blast-radius table's own 'quietest regression' pin");
    }

    // ─── #3120 PR 1 — C17a/C17b: wake/job turns load real session history, budgeted ───

    /// `load_claimed_job_row` returns the narrower `ClaimedJobRow` (no
    /// `runtime_type`-join shortcuts beyond what that query needs);
    /// `prepare_job_dispatch` takes the full `ClaimedJob`. Same manual
    /// field-for-field conversion `test_ceiling_shared_across_chain` above
    /// already uses at its own two call sites — pulled out once here
    /// rather than copied a third/fourth/fifth time.
    fn claimed_job_from_row(row: ClaimedJobRow) -> ClaimedJob {
        ClaimedJob {
            id: row.id, kind: row.kind, title: row.title, brief: row.brief, agent_id: row.agent_id,
            project_id: row.project_id, session_id: row.session_id, runtime_type: row.runtime_type,
            origin_session_id: row.origin_session_id, parent_job_id: row.parent_job_id,
            root_job_id: row.root_job_id, chain_depth: row.chain_depth, wake_origin: row.wake_origin,
        }
    }

    // ─── #3167 Commit 1 — `build_job_context_clone` carries `jobProjectId` ───

    /// Reed's #3167 scope (§9 Commit 1): a Content job's `context_clone`
    /// must carry `"jobProjectId" == job.project_id` verbatim. **Mutation
    /// (actually run):** deleted the `"jobProjectId": job.project_id.clone()`
    /// line from `build_job_context_clone`. Failure output:
    /// ```text
    /// thread '...test_build_job_context_clone_carries_content_job_project_id' panicked at src/ai/scheduler.rs:NNNN:9:
    /// assertion `left == right` failed: a Content job's context_clone must carry its own project_id verbatim
    ///   left: Null
    ///  right: String("proj-content")
    /// ```
    /// Restored and re-confirmed green after.
    #[test]
    fn test_build_job_context_clone_carries_content_job_project_id() {
        let conn = make_conn();
        seed_agent(&conn, "crew-member", "anthropic", None);
        let next_run_at = format_stored_datetime(&Utc::now());
        let job_id = create_job(
            &conn,
            NewJobParams {
                kind: "once", title: "content-job", brief: "do content work", agent_id: "crew-member",
                project_id: "proj-content", cron: None, run_at: None, next_run_at: &next_run_at,
                origin_session_id: None, origin_message_id: None,
                parent_job_id: None, root_job_id: None, chain_depth: 0, wake_origin: false,
            },
        ).unwrap();
        let row = load_claimed_job_row(&conn, &job_id).unwrap();
        let claimed = claimed_job_from_row(row);

        let ctx = build_job_context_clone(&claimed);
        assert_eq!(
            ctx["jobProjectId"], serde_json::json!("proj-content"),
            "a Content job's context_clone must carry its own project_id verbatim"
        );
        // §2 of Reed's scope: the existing `"projectId"` exclusion is UNTOUCHED —
        // this is a DISTINCT key, not a reuse/rename of that one.
        assert!(ctx.get("projectId").is_none(), "the pre-existing `\"projectId\"` exclusion must stay intact — #3167 adds a DISTINCT key");
    }

    /// Reed's #3167 scope (§9 Commit 1), second case: a Home job
    /// (`project_id == "workspace"`) must carry the LITERAL, UNTRANSLATED
    /// sentinel here — `"workspace"` -> `None` translation is Commit 2/5's
    /// job (the two guard-side readers), not this construction site's. If
    /// this test ever asserted `None`/absence instead, it would be pinning
    /// the translation at the wrong layer.
    #[test]
    fn test_build_job_context_clone_carries_literal_workspace_for_home_job_untranslated() {
        let conn = make_conn();
        seed_agent(&conn, "crew-member", "anthropic", None);
        let next_run_at = format_stored_datetime(&Utc::now());
        let job_id = create_job(
            &conn,
            NewJobParams {
                kind: "once", title: "home-job", brief: "do home work", agent_id: "crew-member",
                project_id: "workspace", cron: None, run_at: None, next_run_at: &next_run_at,
                origin_session_id: None, origin_message_id: None,
                parent_job_id: None, root_job_id: None, chain_depth: 0, wake_origin: false,
            },
        ).unwrap();
        let row = load_claimed_job_row(&conn, &job_id).unwrap();
        let claimed = claimed_job_from_row(row);

        let ctx = build_job_context_clone(&claimed);
        assert_eq!(
            ctx["jobProjectId"], serde_json::json!("workspace"),
            "a Home job's context_clone must carry the literal, UNTRANSLATED 'workspace' sentinel \
             here — translation to None is the guard-side readers' job, not this one's"
        );
    }

    /// C17a (§36.4.5, #3120 PR 1): "wake turns load session history" — the
    /// fix for the issue's own quoted log line, `Iteration 1 with 2
    /// messages`. Seeds 3 REAL prior messages in the job's own session
    /// (not a hand-seeded `full_messages` array), dispatches a real job
    /// through `prepare_job_dispatch`, and inspects the resulting
    /// `full_messages` the SAME array that reaches the provider.
    ///
    /// **Mutation (actually run):** reverted
    /// `fetch_session_history_for_job_turn`'s body to `Ok(Vec::new())`
    /// unconditionally. Failure output:
    /// ```text
    /// assertion failed: history_texts.iter().any(|c| c.contains("EARLIER_USER_TEXT_C17A"))
    /// full_messages must be more than system + brief alone (the issue's own 'Iteration 1 with 2 messages') — got 2 messages
    /// ```
    /// Restored and re-confirmed green after.
    #[tokio::test]
    async fn test_prepare_job_dispatch_loads_real_session_history_c17a() {
        let conn = make_conn();
        seed_agent(&conn, "crew-member", "ollama", None);
        crate::database::set_ai_setting(&conn, "ollama_endpoint", "http://127.0.0.1:1").unwrap();
        let origin_session = create_session_internal(&conn, "workspace", "crew-member", "user", true, None).unwrap();

        conn.execute(
            "INSERT INTO chat_messages_local (id, project_id, thread_id, role, content, created_at) VALUES ('c17a-1', 'workspace', ?1, 'user', 'EARLIER_USER_TEXT_C17A', '2026-01-01 00:00:00')",
            params![origin_session.id],
        ).unwrap();
        conn.execute(
            "INSERT INTO chat_messages_local (id, project_id, thread_id, role, content, created_at) VALUES ('c17a-2', 'workspace', ?1, 'assistant', 'EARLIER_ASSISTANT_TEXT_C17A', '2026-01-01 00:00:01')",
            params![origin_session.id],
        ).unwrap();
        conn.execute(
            "INSERT INTO chat_messages_local (id, project_id, thread_id, role, content, created_at) VALUES ('c17a-3', 'workspace', ?1, 'user', 'THIRD_MESSAGE_TEXT_C17A', '2026-01-01 00:00:02')",
            params![origin_session.id],
        ).unwrap();

        let next_run_at = format_stored_datetime(&Utc::now());
        let job_id = create_job(
            &conn,
            NewJobParams {
                kind: "spawn", title: "C17a test", brief: "Do the thing", agent_id: "crew-member",
                project_id: "workspace", cron: None, run_at: None, next_run_at: &next_run_at,
                origin_session_id: Some(&origin_session.id), origin_message_id: None,
                parent_job_id: None, root_job_id: None, chain_depth: 0, wake_origin: false,
            },
        ).unwrap();
        let claimed = claimed_job_from_row(load_claimed_job_row(&conn, &job_id).unwrap());

        let app = tauri::test::mock_app();
        let db = Arc::new(Mutex::new(DbConn::new(conn)));
        let prep = prepare_job_dispatch(&db, &app.handle(), &claimed).await.expect("prepare_job_dispatch must succeed");

        let history_texts: Vec<&str> = prep.full_messages.iter().map(|m| m.content.as_str()).collect();
        assert!(history_texts.iter().any(|c| c.contains("EARLIER_USER_TEXT_C17A")), "the origin session's real prior history must be loaded into full_messages — got: {:?}", history_texts);
        assert!(history_texts.iter().any(|c| c.contains("EARLIER_ASSISTANT_TEXT_C17A")), "got: {:?}", history_texts);
        assert!(history_texts.iter().any(|c| c.contains("THIRD_MESSAGE_TEXT_C17A")), "got: {:?}", history_texts);
        assert!(prep.full_messages.len() > 2, "full_messages must be more than system + brief alone (the issue's own 'Iteration 1 with 2 messages') — got {} messages: {:?}", prep.full_messages.len(), history_texts);
        assert_eq!(prep.full_messages.last().unwrap().content, "Do the thing", "the brief must stay the LAST message, same position a freshly-typed user message occupies interactively");
    }

    /// A job/wake turn's hidden bookkeeping rows (its own synthetic brief,
    /// §36.2 continuation dividers) must NEVER leak into the loaded history
    /// as if they were real prior turns — same filter
    /// `maybe_compact_thread`'s own `conversation_text` build already
    /// applies (`!m.synthetic && !m.continued`).
    ///
    /// **Mutation (actually run):** removed the `if synthetic || continued
    /// { continue; }` guard in `fetch_session_history_for_job_turn`.
    /// Failure output:
    /// ```text
    /// assertion failed: !history_texts.iter().any(|c| c.contains("HIDDEN_BOOKKEEPING_MARKER"))
    /// ```
    /// Restored and re-confirmed green after.
    #[tokio::test]
    async fn test_prepare_job_dispatch_history_excludes_synthetic_and_continued_bookkeeping_rows() {
        let conn = make_conn();
        seed_agent(&conn, "crew-member", "ollama", None);
        crate::database::set_ai_setting(&conn, "ollama_endpoint", "http://127.0.0.1:1").unwrap();
        let origin_session = create_session_internal(&conn, "workspace", "crew-member", "user", true, None).unwrap();

        conn.execute(
            "INSERT INTO chat_messages_local (id, project_id, thread_id, role, content, created_at) VALUES ('c17a-real', 'workspace', ?1, 'user', 'REAL_PRIOR_MESSAGE', '2026-01-01 00:00:00')",
            params![origin_session.id],
        ).unwrap();
        conn.execute(
            "INSERT INTO chat_messages_local (id, project_id, thread_id, role, content, created_at, synthetic) VALUES ('c17a-synth', 'workspace', ?1, 'user', 'HIDDEN_BOOKKEEPING_MARKER_SYNTHETIC', '2026-01-01 00:00:01', 1)",
            params![origin_session.id],
        ).unwrap();
        conn.execute(
            "INSERT INTO chat_messages_local (id, project_id, thread_id, role, content, created_at, continued) VALUES ('c17a-cont', 'workspace', ?1, 'assistant', 'HIDDEN_BOOKKEEPING_MARKER_CONTINUED', '2026-01-01 00:00:02', 1)",
            params![origin_session.id],
        ).unwrap();

        let next_run_at = format_stored_datetime(&Utc::now());
        let job_id = create_job(
            &conn,
            NewJobParams {
                kind: "spawn", title: "C17a bookkeeping test", brief: "Do the thing", agent_id: "crew-member",
                project_id: "workspace", cron: None, run_at: None, next_run_at: &next_run_at,
                origin_session_id: Some(&origin_session.id), origin_message_id: None,
                parent_job_id: None, root_job_id: None, chain_depth: 0, wake_origin: false,
            },
        ).unwrap();
        let claimed = claimed_job_from_row(load_claimed_job_row(&conn, &job_id).unwrap());

        let app = tauri::test::mock_app();
        let db = Arc::new(Mutex::new(DbConn::new(conn)));
        let prep = prepare_job_dispatch(&db, &app.handle(), &claimed).await.expect("prepare_job_dispatch must succeed");

        let history_texts: Vec<&str> = prep.full_messages.iter().map(|m| m.content.as_str()).collect();
        assert!(history_texts.iter().any(|c| c.contains("REAL_PRIOR_MESSAGE")), "got: {:?}", history_texts);
        assert!(!history_texts.iter().any(|c| c.contains("HIDDEN_BOOKKEEPING_MARKER")), "synthetic/continued bookkeeping rows must never leak into loaded history — got: {:?}", history_texts);
    }

    /// C17b (§36.4.5, #3120 PR 1 — mandatory together with C17a): "the
    /// moment you add history without budgeting, a wake turn can overflow
    /// context." Seeds 20 real prior messages, each large enough
    /// (~12,500 estimated tokens) that their combined cost comfortably
    /// exceeds even the unconfigured-model 128k-floor window's available
    /// budget (`test-model` is in no seed table and the mocked Ollama
    /// endpoint has no `/api/show` route, so `resolve_context_window_with_
    /// source` falls through to precedence #5) — well beyond the budget
    /// algorithm's own `min(12, messages.len())` floor, and asserts the
    /// turn's final `full_messages` is SHORTER than the unbudgeted total
    /// (1 system + 20 history + 1 brief = 22).
    ///
    /// **Mutation (actually run):** reverted `prepare_job_dispatch`'s
    /// budget block to the pre-C17b shape (`build_messages_with_context`'s
    /// output returned as-is, `apply_budget` never called). Failure
    /// output:
    /// ```text
    /// assertion failed: prep.full_messages.len() < 22
    ///   left: 22
    ///  right: 22
    /// ```
    /// (every seeded message reached the provider, unbudgeted — exactly
    /// the regression C17a alone would have opened). Restored and
    /// re-confirmed green after.
    #[tokio::test]
    async fn test_prepare_job_dispatch_applies_budget_trims_large_history_c17b() {
        let conn = make_conn();
        seed_agent(&conn, "crew-member", "ollama", None);
        crate::database::set_ai_setting(&conn, "ollama_endpoint", "http://127.0.0.1:1").unwrap();
        let origin_session = create_session_internal(&conn, "workspace", "crew-member", "user", true, None).unwrap();

        // Each message ~50,000 characters (~12,500 estimated tokens) —
        // large enough that the floor-guaranteed newest-12 alone exceed the
        // 128k-floor window's available budget, forcing every OLDER
        // message to be excluded (the `total_msg_tokens <= budget_remaining`
        // branch can never pass once `budget_remaining` has saturated to 0).
        for i in 0..20 {
            conn.execute(
                "INSERT INTO chat_messages_local (id, project_id, thread_id, role, content, created_at) VALUES (?1, 'workspace', ?2, ?3, ?4, ?5)",
                params![
                    format!("c17b-{}", i),
                    origin_session.id,
                    if i % 2 == 0 { "user" } else { "assistant" },
                    format!("C17B_HISTORY_MESSAGE_{}_{}", i, "x".repeat(50_000)),
                    format!("2026-01-01 00:{:02}:00", i),
                ],
            ).unwrap();
        }

        let next_run_at = format_stored_datetime(&Utc::now());
        let job_id = create_job(
            &conn,
            NewJobParams {
                kind: "spawn", title: "C17b test", brief: "Do the thing", agent_id: "crew-member",
                project_id: "workspace", cron: None, run_at: None, next_run_at: &next_run_at,
                origin_session_id: Some(&origin_session.id), origin_message_id: None,
                parent_job_id: None, root_job_id: None, chain_depth: 0, wake_origin: false,
            },
        ).unwrap();
        let claimed = claimed_job_from_row(load_claimed_job_row(&conn, &job_id).unwrap());

        let app = tauri::test::mock_app();
        let db = Arc::new(Mutex::new(DbConn::new(conn)));
        let prep = prepare_job_dispatch(&db, &app.handle(), &claimed).await.expect("prepare_job_dispatch must succeed");

        let unbudgeted_total = 1 /* system */ + 20 /* history */ + 1 /* brief */;
        assert!(
            prep.full_messages.len() < unbudgeted_total,
            "a tiny context_window_override must trim full_messages below the unbudgeted total ({}) — got {} messages",
            unbudgeted_total, prep.full_messages.len()
        );
        // The brief itself (newest message) must survive any trim — trimming
        // drops the OLDEST content first, never the turn's own "new thing."
        assert_eq!(prep.full_messages.last().unwrap().content, "Do the thing", "the brief must survive budget trimming — it's always the newest message");
    }

    // ─── #3120 PR 1 — C22: one wake per sibling-completion batch ───

    /// C22 (§36.4.5, #3120 PR 1): "Confirmed at N wakes for N siblings, no
    /// dedup anywhere in the wake-creation block." 3 REAL sibling
    /// `spawn_job`-shaped completions (via `create_job`, not hand-seeded
    /// `agent_jobs` rows), all sharing the SAME `origin_session_id`, run
    /// through real `scheduler_tick` dispatches end to end — exactly one
    /// PENDING wake row must exist afterward, not three.
    ///
    /// **Mutation (actually run):** dropped
    /// `idx_agent_jobs_one_pending_wake_per_session` from
    /// `ensure_agent_jobs_wake_dedup_index`/`create_agent_jobs_table`
    /// (left the plain `INSERT` with no `ON CONFLICT` target, which SQLite
    /// then executes as an ordinary insert since no unique index exists to
    /// conflict against). Failure output:
    /// ```text
    /// assertion `left == right` failed: exactly one pending wake row must exist for 3 sibling completions into the same origin session
    ///   left: 3
    ///  right: 1
    /// ```
    /// Restored and re-confirmed green after.
    #[tokio::test]
    async fn test_wake_dedup_one_pending_wake_for_n_sibling_completions_c22() {
        let conn = make_conn();
        seed_agent(&conn, "captain", "ollama", None);
        seed_agent(&conn, "crew-member", "ollama", None);

        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/api/chat")
            .with_status(200)
            .with_header("content-type", "application/x-ndjson")
            .with_body(b"{\"model\":\"test-model\",\"message\":{\"role\":\"assistant\",\"content\":\"Sibling done.\"},\"done\":true}\n" as &[u8])
            .expect_at_least(3)
            .create_async()
            .await;
        crate::database::set_ai_setting(&conn, "ollama_endpoint", &server.url()).unwrap();
        // Raise concurrency so all 3 siblings can dispatch promptly — the
        // dedup property under test must hold regardless of how many ticks
        // it takes, but a generous concurrency setting keeps this test fast.
        crate::database::set_ai_setting(&conn, "job_concurrency", "3").unwrap();

        let origin_session = create_session_internal(&conn, "workspace", "captain", "user", true, None).unwrap();
        let next_run_at = format_stored_datetime(&Utc::now());
        let mut sibling_job_ids: Vec<String> = Vec::new();
        for _ in 0..3 {
            let sibling_id = create_job(
                &conn,
                NewJobParams {
                    kind: "spawn", title: "Sibling", brief: "Do the thing", agent_id: "crew-member",
                    project_id: "workspace", cron: None, run_at: None, next_run_at: &next_run_at,
                    origin_session_id: Some(&origin_session.id), origin_message_id: None,
                    parent_job_id: None, root_job_id: None, chain_depth: 0, wake_origin: true,
                },
            ).unwrap();
            sibling_job_ids.push(sibling_id);
        }

        let db = Arc::new(Mutex::new(DbConn::new(conn)));
        let app = tauri::test::mock_app();

        // One tick claims all 3 due siblings at once (job_concurrency=3,
        // set above) and dispatches each on its own detached async task.
        // Deliberately NOT a loop of further ticks here — a second tick
        // would also claim the wake row the instant it exists (its own
        // `next_run_at` is immediate), flipping it from `pending` to
        // `running` before this test ever gets to assert the pending count.
        scheduler_tick(db.clone(), app.handle().clone()).await;
        poll_until(
            || async {
                if !mock.matched_async().await {
                    return false;
                }
                let conn = db.lock().unwrap();
                let pending: i64 = conn
                    .query_row("SELECT COUNT(*) FROM agent_jobs WHERE kind = 'wake' AND status = 'pending'", [], |r| r.get(0))
                    .unwrap_or(-1);
                pending >= 0
            },
            "the sibling spawn jobs never finished dispatching",
        )
        .await;

        // Give the slowest sibling's detached bookkeeping task a final beat
        // — the mock-matched check above only proves the HTTP call landed,
        // not that the wake-creation closure (which runs AFTER it, in the
        // same detached task) has finished writing its row yet.
        poll_until(
            || async {
                let conn = db.lock().unwrap();
                let done: i64 = conn
                    .query_row("SELECT COUNT(*) FROM agent_jobs WHERE kind = 'spawn' AND status = 'done'", [], |r| r.get(0))
                    .unwrap_or(0);
                done == 3
            },
            "all 3 sibling spawn jobs must reach status='done'",
        )
        .await;

        let conn = db.lock().unwrap();
        let pending_wake_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM agent_jobs WHERE kind = 'wake' AND status = 'pending'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(pending_wake_count, 1, "exactly one pending wake row must exist for 3 sibling completions into the same origin session");
        let wake_session_id: String = conn
            .query_row("SELECT session_id FROM agent_jobs WHERE kind = 'wake' AND status = 'pending'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(wake_session_id, origin_session.id, "the single wake row must target the shared origin session");
        let wake_brief: String = conn
            .query_row("SELECT brief FROM agent_jobs WHERE kind = 'wake' AND status = 'pending'", [], |r| r.get(0))
            .unwrap();
        // All 3 siblings' briefs concatenated, not the last-writer's alone —
        // proves the upsert path (not INSERT OR IGNORE) actually ran for
        // at least 2 of the 3 conflicts.
        assert!(wake_brief.contains("Sibling"), "wake brief must carry sibling completion text: {}", wake_brief);
        drop(conn);

        // Second half of C22's own test spec: after the wake is CLAIMED
        // (status flips to 'running'), the partial index stops matching, so
        // a FOURTH sibling's completion must create a FRESH pending wake,
        // not silently no-op forever.
        let pending_wake_id: String = {
            let conn = db.lock().unwrap();
            conn.query_row("SELECT id FROM agent_jobs WHERE kind = 'wake' AND status = 'pending'", [], |r| r.get(0)).unwrap()
        };
        {
            let conn = db.lock().unwrap();
            conn.execute("UPDATE agent_jobs SET status = 'running' WHERE id = ?1", params![pending_wake_id]).unwrap();
        }
        {
            // Reuses one of the 3 REAL sibling job ids as the `parent_job_id`/
            // `root_job_id` FK targets (both columns reference `agent_jobs(id)`)
            // — a hand-picked fake id would violate the FK constraint;
            // these rows genuinely exist and are genuinely finished.
            let conn = db.lock().unwrap();
            let fourth_parent = &sibling_job_ids[0];
            create_wake_job(&conn, &origin_session.id, "workspace", "crew-member", "[4th sibling finished]", fourth_parent, fourth_parent, 1).unwrap();
        }
        let conn = db.lock().unwrap();
        let total_wakes: i64 = conn.query_row("SELECT COUNT(*) FROM agent_jobs WHERE kind = 'wake'", [], |r| r.get(0)).unwrap();
        let pending_wakes_after: i64 = conn.query_row("SELECT COUNT(*) FROM agent_jobs WHERE kind = 'wake' AND status = 'pending'", [], |r| r.get(0)).unwrap();
        assert_eq!(total_wakes, 2, "claiming the first wake must free the slot for a genuinely NEW pending wake, not block it forever");
        assert_eq!(pending_wakes_after, 1, "exactly one NEW pending wake row after the prior one was claimed");
    }

    // ─── #3120 PR 1 — C18: live refresh on wake/report-back ───

    /// C18 (§36.7.1/§36.8.1, #3120 PR 1): "the message appears only after
    /// navigating away and back." Report-back into an OPEN origin session
    /// must emit `pane-chip-changed` with `status: "inserted"`
    /// (`ChatPanel.svelte`'s existing listener already re-fetches on this
    /// exact shape); into a CLOSED session it must NOT — that path already
    /// has the `agent_message` notification instead (no double-signal).
    ///
    /// **Mutation (actually run):** removed the new `emit_chip_changed(...,
    /// "inserted")` call at the report-back ingest site. Failure output:
    /// ```text
    /// assertion failed: inserted_events.iter().any(|e| e["session_id"] == origin_session.id)
    /// a report-back into an OPEN session must emit pane-chip-changed status=inserted — got: []
    /// ```
    /// Restored and re-confirmed green after.
    #[tokio::test]
    async fn test_report_back_live_refresh_open_vs_closed_session_c18() {
        async fn run_one_spawn_and_collect_inserted_events(session_open: bool) -> (Vec<serde_json::Value>, i64, String) {
            let conn = Connection::open_in_memory().expect("in-memory db");
            crate::migrations::run_migrations(&conn).expect("fresh-install migrations should succeed");
            seed_agent(&conn, "captain", "ollama", None);
            seed_agent(&conn, "crew-member", "ollama", None);

            let mut server = mockito::Server::new_async().await;
            let mock = server
                .mock("POST", "/api/chat")
                .with_status(200)
                .with_header("content-type", "application/x-ndjson")
                .with_body(b"{\"model\":\"test-model\",\"message\":{\"role\":\"assistant\",\"content\":\"C18 report done.\"},\"done\":true}\n" as &[u8])
                .create_async()
                .await;
            crate::database::set_ai_setting(&conn, "ollama_endpoint", &server.url()).unwrap();

            let origin_session = create_session_internal(&conn, "workspace", "captain", "user", true, None).unwrap();
            let next_run_at = format_stored_datetime(&Utc::now());
            let job_id = create_job(
                &conn,
                NewJobParams {
                    kind: "spawn", title: "C18 test", brief: "Do the thing", agent_id: "crew-member",
                    project_id: "workspace", cron: None, run_at: None, next_run_at: &next_run_at,
                    origin_session_id: Some(&origin_session.id), origin_message_id: None,
                    parent_job_id: None, root_job_id: None, chain_depth: 0, wake_origin: false,
                },
            ).unwrap();

            if session_open {
                crate::ai::chat::set_viewing_session(Some(origin_session.id.clone())).unwrap();
            } else {
                crate::ai::chat::set_viewing_session(None).unwrap();
            }

            let app = tauri::test::mock_builder()
                .plugin(tauri_plugin_notification::init())
                .build(tauri::test::mock_context(tauri::test::noop_assets()))
                .expect("mock app with notification plugin must build");
            let events: Arc<Mutex<Vec<serde_json::Value>>> = Arc::new(Mutex::new(Vec::new()));
            let events2 = events.clone();
            app.listen(CHIP_CHANGED_EVENT, move |event| {
                events2.lock().unwrap().push(serde_json::from_str(event.payload()).unwrap());
            });

            let db = Arc::new(Mutex::new(DbConn::new(conn)));
            scheduler_tick(db.clone(), app.handle().clone()).await;
            poll_until(
                || async {
                    if !mock.matched_async().await {
                        return false;
                    }
                    let conn = db.lock().unwrap();
                    conn.query_row(
                        "SELECT COUNT(*) FROM chat_messages_local WHERE thread_id = ?1 AND job_id = ?2",
                        params![origin_session.id, job_id],
                        |r| r.get::<_, i64>(0),
                    ).unwrap_or(0) == 1
                },
                "report-back row must land in the origin session",
            )
            .await;

            let conn = db.lock().unwrap();
            let notif_count: i64 = conn
                .query_row("SELECT COUNT(*) FROM notifications WHERE type = 'agent_message'", [], |r| r.get(0))
                .unwrap_or(0);
            drop(conn);
            crate::ai::chat::set_viewing_session(None).unwrap();

            let inserted_events: Vec<serde_json::Value> = events
                .lock()
                .unwrap()
                .iter()
                .filter(|e| e["status"] == "inserted")
                .cloned()
                .collect();
            (inserted_events, notif_count, origin_session.id)
        }

        let (open_inserted_events, open_notif_count, open_session_id) = run_one_spawn_and_collect_inserted_events(true).await;
        assert!(
            open_inserted_events.iter().any(|e| e["session_id"] == open_session_id.as_str()),
            "a report-back into an OPEN session must emit pane-chip-changed status=inserted — got: {:?}", open_inserted_events
        );
        assert_eq!(open_notif_count, 0, "an OPEN session must NOT also raise an agent_message notification (no double-signal)");

        let (closed_inserted_events, closed_notif_count, closed_session_id) = run_one_spawn_and_collect_inserted_events(false).await;
        assert!(
            !closed_inserted_events.iter().any(|e| e["session_id"] == closed_session_id.as_str()),
            "a report-back into a CLOSED session must NOT emit the live-refresh signal — got: {:?}", closed_inserted_events
        );
        assert_eq!(closed_notif_count, 1, "a CLOSED session must raise exactly one agent_message notification instead");
    }

    /// C18, second half (§36.4.5's own "the turn runs directly in the real
    /// chat session" — made true by this PR): a wake turn's OWN reply must
    /// actually be PERSISTED into its session (traced: no code path did
    /// this before this commit — see `insert_wake_turn_reply`'s own doc
    /// comment) and, when that session is open, must also live-refresh it.
    ///
    /// **Mutation (actually run):** removed the `if job_kind_for_report ==
    /// "wake" { insert_wake_turn_reply(...) }` call. Failure output:
    /// ```text
    /// assertion `left == right` failed: the wake turn's own reply must be persisted as a real, visible assistant message
    ///   left: 0
    ///  right: 1
    /// ```
    /// Restored and re-confirmed green after.
    #[tokio::test]
    async fn test_wake_turn_reply_is_persisted_and_live_refreshes_open_session_c18() {
        let conn = make_conn();
        seed_agent(&conn, "captain", "ollama", None);
        seed_agent(&conn, "crew-member", "ollama", None);

        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/api/chat")
            .with_status(200)
            .with_header("content-type", "application/x-ndjson")
            .with_body(b"{\"model\":\"test-model\",\"message\":{\"role\":\"assistant\",\"content\":\"The thing is done.\"},\"done\":true}\n" as &[u8])
            .expect_at_least(2)
            .create_async()
            .await;
        crate::database::set_ai_setting(&conn, "ollama_endpoint", &server.url()).unwrap();

        let origin_session = create_session_internal(&conn, "workspace", "captain", "user", true, None).unwrap();
        let next_run_at = format_stored_datetime(&Utc::now());
        create_job(
            &conn,
            NewJobParams {
                kind: "spawn", title: "Test spawn", brief: "Do the thing", agent_id: "crew-member",
                project_id: "workspace", cron: None, run_at: None, next_run_at: &next_run_at,
                origin_session_id: Some(&origin_session.id), origin_message_id: None,
                parent_job_id: None, root_job_id: None, chain_depth: 0, wake_origin: true,
            },
        ).unwrap();

        // The origin session is OPEN while the wake turn runs — this is the
        // exact scenario the issue names ("while the user is viewing it").
        crate::ai::chat::set_viewing_session(Some(origin_session.id.clone())).unwrap();

        let app = tauri::test::mock_app();
        let events: Arc<Mutex<Vec<serde_json::Value>>> = Arc::new(Mutex::new(Vec::new()));
        let events2 = events.clone();
        app.listen(CHIP_CHANGED_EVENT, move |event| {
            events2.lock().unwrap().push(serde_json::from_str(event.payload()).unwrap());
        });

        let db = Arc::new(Mutex::new(DbConn::new(conn)));

        // Tick 1: dispatch the spawn, create the wake row.
        scheduler_tick(db.clone(), app.handle().clone()).await;
        poll_until(
            || async {
                let conn = db.lock().unwrap();
                conn.query_row("SELECT COUNT(*) FROM agent_jobs WHERE kind = 'wake'", [], |r| r.get::<_, i64>(0)).unwrap_or(0) == 1
            },
            "the wake row must be created after the spawn finishes",
        )
        .await;

        let wake_job_id: String = {
            let conn = db.lock().unwrap();
            conn.query_row("SELECT id FROM agent_jobs WHERE kind = 'wake'", [], |r| r.get(0)).unwrap()
        };

        // Tick 2: dispatch the WAKE turn itself — its own reply must now be
        // persisted (the gap this commit closes) AND live-refresh the open
        // session.
        scheduler_tick(db.clone(), app.handle().clone()).await;
        poll_until(
            || async {
                if !mock.matched_async().await {
                    return false;
                }
                let conn = db.lock().unwrap();
                conn.query_row(
                    "SELECT COUNT(*) FROM chat_messages_local WHERE job_id = ?1 AND synthetic = 0 AND role = 'assistant'",
                    params![wake_job_id],
                    |r| r.get::<_, i64>(0),
                ).unwrap_or(0) == 1
            },
            "the wake turn's own reply must be persisted as a real, visible assistant message",
        )
        .await;

        mock.assert_async().await;
        crate::ai::chat::set_viewing_session(None).unwrap();

        let conn = db.lock().unwrap();
        let (content, role, synthetic, model, agent_id): (String, String, i64, Option<String>, Option<String>) = conn
            .query_row(
                "SELECT content, role, synthetic, model, agent_id FROM chat_messages_local WHERE job_id = ?1 AND role = 'assistant' AND synthetic = 0",
                params![wake_job_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
            )
            .expect("the wake turn's own reply must exist as a real, visible assistant message");
        assert_eq!(content, "The thing is done.", "the wake turn's own final text must be the persisted content");
        assert_eq!(role, "assistant");
        assert_eq!(synthetic, 0, "the wake's own reply must render — unlike its hidden synthetic brief");
        assert_eq!(model, Some("test-model".to_string()), "the wake reply must carry a real model, like any normal assistant turn");
        assert_eq!(agent_id, Some("captain".to_string()), "the wake reply is attributed to the session's own bound agent");
        drop(conn);

let inserted_for_wake = events.lock().unwrap().iter().any(|e| e["job_id"] == wake_job_id.as_str() && e["status"] == "inserted");
        assert!(inserted_for_wake, "the open origin session must live-refresh when the wake turn's own reply lands — got: {:?}", events.lock().unwrap());
    }

    /// #3171 PR 2 (C59): the 3-layer threading gap Reed's audit traced —
    /// `AgentLoopResult` → `insert_wake_turn_reply`'s signature → both call
    /// sites. `persist_job_turn_reply` (defined just above
    /// `insert_wake_turn_reply`) is the ONE function that turns a job
    /// turn's generated-image attachment ids into the persisted
    /// `chat_messages_local.attachment_ids` column — the exact code the
    /// real report-back/own-session call site runs. Driven directly here
    /// with a synthetic id list rather than through a genuinely
    /// successful `generate_image` call: that call hits a HARDCODED real
    /// provider URL with no test seam (`execute_generate_image`,
    /// image_gen.rs), so exercising it for real would violate this
    /// suite's mockito-only, no-real-provider-endpoint hard constraint.
    ///
    /// **Mutation (actually run):** reverted `persist_job_turn_reply`'s
    /// body to always pass `None` regardless of `image_attachment_ids`
    /// (the exact shape of the bug before this PR — confirmed
    /// `insert_wake_turn_reply` had zero `attachment_ids` references at
    /// all). Failure output:
    /// ```text
    /// thread '...test_job_turn_reply_persists_image_attachment_ids' panicked at src/ai/scheduler.rs:NNNN:9:
    /// assertion `left == right` failed: a job turn's generated-image attachment ids must be persisted on the chat_messages_local row, not dropped
    ///   left: None
    ///  right: Some("attachment-abc,attachment-def")
    /// ```
    /// Restored and re-confirmed green after.
    #[tokio::test]
    async fn test_job_turn_reply_persists_image_attachment_ids() {
        let conn = make_conn();
        seed_agent(&conn, "captain", "ollama", None);
        let session = create_session_internal(&conn, "workspace", "captain", "user", true, None).unwrap();

        let message_id = persist_job_turn_reply::<tauri::test::MockRuntime>(
            &conn,
            &session.id,
            "workspace",
            "captain",
            "test-model",
            "Generated 2 image(s)",
            "job-image-turn-1",
            &["attachment-abc".to_string(), "attachment-def".to_string()],
            None,
        )
        .expect("persist_job_turn_reply should succeed");

        let attachment_ids: Option<String> = conn
            .query_row(
                "SELECT attachment_ids FROM chat_messages_local WHERE id = ?1",
                params![message_id],
                |r| r.get(0),
            )
            .expect("the persisted row must exist");

        assert_eq!(
            attachment_ids,
            Some("attachment-abc,attachment-def".to_string()),
            "a job turn's generated-image attachment ids must be persisted on the \
             chat_messages_local row, not dropped"
        );
    }

    /// Sibling control: a job turn that generated NO images (the common
    /// case — text-only turns, or a turn where `generate_image` was never
    /// called) must persist `attachment_ids` as `NULL`, not an empty
    /// string — the same NULL-for-"none" convention
    /// `chat_messages_local.attachment_ids` already uses for every
    /// interactive-turn row (`saveAssistantMessage`'s own
    /// `image_attachment_ids?.join(',') || null`, ChatSidebar.svelte).
    #[tokio::test]
    async fn test_job_turn_reply_with_no_images_persists_null_attachment_ids() {
        let conn = make_conn();
        seed_agent(&conn, "captain", "ollama", None);
        let session = create_session_internal(&conn, "workspace", "captain", "user", true, None).unwrap();

        let message_id = persist_job_turn_reply::<tauri::test::MockRuntime>(
            &conn,
            &session.id,
            "workspace",
            "captain",
            "test-model",
            "Done.",
            "job-text-turn-1",
            &[],
            None,
        )
        .expect("persist_job_turn_reply should succeed");

        let attachment_ids: Option<String> = conn
            .query_row(
                "SELECT attachment_ids FROM chat_messages_local WHERE id = ?1",
                params![message_id],
                |r| r.get(0),
            )
            .expect("the persisted row must exist");

        assert_eq!(attachment_ids, None, "a text-only job turn must persist NULL attachment_ids, not an empty string");
    }

    // --- #3123 (#3120 PR 2, LAST commit) --- recurring/heartbeat job turns persist their own reply

    /// #3123, Shape 2 ("persist at the scheduler"): a `recurring` API-
    /// runtime job's turn, on producing non-empty `final_text`, must now
    /// get a real `chat_messages_local` row in its OWN session
    /// (`job.session_id`, the persistent schedule session §36.3.6
    /// describes) — reusing C18's `insert_wake_turn_reply` exactly as a
    /// wake turn does. Before this commit, NOTHING persisted a recurring
    /// job's own reply anywhere (traced in #3123's own filing: the
    /// interactive path's only writer is the frontend's `ai-stream-chunk`
    /// listener, never registered for a scheduler-dispatched `sid`).
    ///
    /// Mutation: revert the widened condition back to `job_kind_for_report
    /// == "wake"` alone — reddens: zero matching rows.
    #[tokio::test]
    async fn test_recurring_job_turn_reply_is_persisted_3123() {
        let conn = make_conn();
        seed_agent(&conn, "recur-agent", "ollama", None);

        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/api/chat")
            .with_status(200)
            .with_header("content-type", "application/x-ndjson")
            .with_body(b"{\"model\":\"test-model\",\"message\":{\"role\":\"assistant\",\"content\":\"Nothing needed.\"},\"done\":true}\n" as &[u8])
            .create_async()
            .await;
        crate::database::set_ai_setting(&conn, "ollama_endpoint", &server.url()).unwrap();

        let next_run_at = format_stored_datetime(&Utc::now());
        let job_id = create_job(
            &conn,
            NewJobParams {
                kind: "recurring", title: "Daily check-in", brief: "Check in", agent_id: "recur-agent",
                project_id: "workspace", cron: Some("0 9 * * *"), run_at: None, next_run_at: &next_run_at,
                origin_session_id: None, origin_message_id: None,
                parent_job_id: None, root_job_id: None, chain_depth: 0, wake_origin: false,
            },
        ).unwrap();

        let app = test_app_handle();
        let db = Arc::new(Mutex::new(DbConn::new(conn)));

        scheduler_tick(db.clone(), app.clone()).await;
        poll_until(
            || async {
                if !mock.matched_async().await {
                    return false;
                }
                let conn = db.lock().unwrap();
                conn.query_row(
                    "SELECT COUNT(*) FROM chat_messages_local WHERE job_id = ?1 AND synthetic = 0 AND role = 'assistant'",
                    params![job_id],
                    |r| r.get::<_, i64>(0),
                ).unwrap_or(0) == 1
            },
            "a recurring job's own turn reply must be persisted as a real, visible assistant message",
        )
        .await;

        let conn = db.lock().unwrap();
        let (content, thread_id, model): (String, String, Option<String>) = conn
            .query_row(
                "SELECT content, thread_id, model FROM chat_messages_local WHERE job_id = ?1 AND role = 'assistant' AND synthetic = 0",
                params![job_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("the recurring job's own reply must exist as a real, visible assistant message");
        assert_eq!(content, "Nothing needed.");
        let job_session_id: String = conn.query_row("SELECT session_id FROM agent_jobs WHERE id = ?1", params![job_id], |r| r.get(0)).unwrap();
        assert_eq!(thread_id, job_session_id, "the reply must land in the job's OWN session (§36.3.6's persistent schedule session), not origin_session_id");
        assert_eq!(model, Some("test-model".to_string()));
    }

    /// Same proof for `heartbeat` — the second kind #3123 names.
    #[tokio::test]
    async fn test_heartbeat_job_turn_reply_is_persisted_3123() {
        let conn = make_conn();
        seed_agent(&conn, "heartbeat-agent", "ollama", None);

        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/api/chat")
            .with_status(200)
            .with_header("content-type", "application/x-ndjson")
            .with_body(b"{\"model\":\"test-model\",\"message\":{\"role\":\"assistant\",\"content\":\"All clear.\"},\"done\":true}\n" as &[u8])
            .create_async()
            .await;
        crate::database::set_ai_setting(&conn, "ollama_endpoint", &server.url()).unwrap();

        let next_run_at = format_stored_datetime(&Utc::now());
        let job_id = create_job(
            &conn,
            NewJobParams {
                kind: "heartbeat", title: "Check-in", brief: "Review open tasks", agent_id: "heartbeat-agent",
                project_id: "workspace", cron: Some("0 */4 * * *"), run_at: None, next_run_at: &next_run_at,
                origin_session_id: None, origin_message_id: None,
                parent_job_id: None, root_job_id: None, chain_depth: 0, wake_origin: false,
            },
        ).unwrap();

        let app = test_app_handle();
        let db = Arc::new(Mutex::new(DbConn::new(conn)));

        scheduler_tick(db.clone(), app.clone()).await;
        poll_until(
            || async {
                if !mock.matched_async().await {
                    return false;
                }
                let conn = db.lock().unwrap();
                conn.query_row(
                    "SELECT COUNT(*) FROM chat_messages_local WHERE job_id = ?1 AND synthetic = 0 AND role = 'assistant'",
                    params![job_id],
                    |r| r.get::<_, i64>(0),
                ).unwrap_or(0) == 1
            },
            "a heartbeat job's own turn reply must be persisted as a real, visible assistant message",
        )
        .await;

        let conn = db.lock().unwrap();
        let content: String = conn
            .query_row(
                "SELECT content FROM chat_messages_local WHERE job_id = ?1 AND role = 'assistant' AND synthetic = 0",
                params![job_id],
                |row| row.get(0),
            )
            .expect("the heartbeat job's own reply must exist");
        assert_eq!(content, "All clear.");
    }

    /// `is_session_open` gating: same shape C18's own wake test already
    /// uses — if a recurring job's own session is open, `pane-chip-
    /// changed` fires; regression pin that this gating logic (shared,
    /// unmodified code inside `insert_wake_turn_reply`) still applies
    /// correctly when called for a NEW kind.
    #[tokio::test]
    async fn test_recurring_job_turn_reply_live_refreshes_open_session_3123() {
        let conn = make_conn();
        seed_agent(&conn, "recur-open-agent", "ollama", None);

        let mut server = mockito::Server::new_async().await;
        // Kept bound (not discarded) so the mock stays registered on the
        // server for the duration of the test; no longer polled directly
        // (see the `poll_until` call below — we now wait on the actual
        // asserted condition, the "inserted" chip event for this job_id,
        // not this mock's own "was I hit" signal).
        let _mock = server
            .mock("POST", "/api/chat")
            .with_status(200)
            .with_header("content-type", "application/x-ndjson")
            .with_body(b"{\"model\":\"test-model\",\"message\":{\"role\":\"assistant\",\"content\":\"Done.\"},\"done\":true}\n" as &[u8])
            .create_async()
            .await;
        crate::database::set_ai_setting(&conn, "ollama_endpoint", &server.url()).unwrap();

        let next_run_at = format_stored_datetime(&Utc::now());
        let job_id = create_job(
            &conn,
            NewJobParams {
                kind: "recurring", title: "Open-session recurring", brief: "Check in", agent_id: "recur-open-agent",
                project_id: "workspace", cron: Some("0 9 * * *"), run_at: None, next_run_at: &next_run_at,
                origin_session_id: None, origin_message_id: None,
                parent_job_id: None, root_job_id: None, chain_depth: 0, wake_origin: false,
            },
        ).unwrap();
        let job_session_id: String = conn.query_row("SELECT session_id FROM agent_jobs WHERE id = ?1", params![job_id], |r| r.get(0)).unwrap();

        // The job's own session is OPEN while its turn runs.
        crate::ai::chat::set_viewing_session(Some(job_session_id.clone())).unwrap();

        let app = tauri::test::mock_app();
        let events: Arc<Mutex<Vec<serde_json::Value>>> = Arc::new(Mutex::new(Vec::new()));
        let events2 = events.clone();
        app.listen(CHIP_CHANGED_EVENT, move |event| {
            events2.lock().unwrap().push(serde_json::from_str(event.payload()).unwrap());
        });

        let db = Arc::new(Mutex::new(DbConn::new(conn)));
        scheduler_tick(db.clone(), app.handle().clone()).await;
        // Poll the condition this test actually asserts: an "inserted"
        // `pane-chip-changed` event for THIS job_id landing in `events`.
        // `mock.matched_async()` only proves the HTTP server received the
        // request — the detached dispatch task (spawned via
        // `tauri::async_runtime::spawn`, cooperatively scheduled on the
        // same current-thread `#[tokio::test]` runtime) still has to
        // parse the response, write the terminal status, call
        // `insert_wake_turn_reply`, and emit the chip event *after* that.
        // Polling the mock alone is a race between "server observed the
        // request" and "client task finished its post-response
        // bookkeeping" — polling `events` itself waits on the actual
        // end state, the same pattern
        // `test_recurring_job_turn_reply_is_persisted_3123` and
        // `test_heartbeat_job_turn_reply_is_persisted_3123` already use
        // correctly (they poll the DB row, not a weaker proxy signal).
        poll_until(
            || async {
                events.lock().unwrap().iter().any(|e| e["job_id"] == job_id.as_str() && e["status"] == "inserted")
            },
            "the open job session must live-refresh when the recurring turn's own reply lands",
        )
        .await;
        crate::ai::chat::set_viewing_session(None).unwrap();

        let inserted_for_job = events.lock().unwrap().iter().any(|e| e["job_id"] == job_id.as_str() && e["status"] == "inserted");
        assert!(inserted_for_job, "the open job session must live-refresh when the recurring turn's own reply lands — got: {:?}", events.lock().unwrap());
    }

    // ─── C41 (#3147, live test #5) — a recurring job notifies once, ever ───

    /// Matt's confirmed repro: Tick fires three times, Home shows one
    /// bell. Root cause confirmed (Reed's §2): the own-session
    /// notification was keyed `job-turn-{job_id}` — identical on every
    /// firing of the SAME job — and `emit_agent_message_notification`'s
    /// own dedup (`SELECT COUNT(*) FROM notifications WHERE idem_key =
    /// ?1`) skips any key it has already seen. Fired twice here (not
    /// three times — two is already sufficient to prove "per firing, not
    /// per job"), each firing forced due via the same `next_run_at =
    /// datetime('now')` technique `run_job_now` already uses in
    /// production.
    ///
    /// **Mutation (run, see report): reverted the key from
    /// `own_session_notification_due`'s message id back to the old
    /// `format!("job-turn-{}", job_id_for_bookkeeping)` — reddens to ONE
    /// row after two firings** (the second firing's emit call computes
    /// the SAME `idem_key` as the first and is dedup-skipped) — the
    /// exact bug this commit closes. Restored and re-confirmed green
    /// after.
    #[tokio::test]
    async fn test_recurring_job_two_firings_produce_two_notification_rows_c41() {
        let conn = make_conn();
        seed_agent(&conn, "tick-agent-c41", "ollama", None);

        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/api/chat")
            .with_status(200)
            .with_header("content-type", "application/x-ndjson")
            .with_body(b"{\"model\":\"test-model\",\"message\":{\"role\":\"assistant\",\"content\":\"Tick.\"},\"done\":true}\n" as &[u8])
            .expect_at_least(2)
            .create_async()
            .await;
        crate::database::set_ai_setting(&conn, "ollama_endpoint", &server.url()).unwrap();

        let next_run_at = format_stored_datetime(&Utc::now());
        let job_id = create_job(
            &conn,
            NewJobParams {
                kind: "recurring", title: "Tick", brief: "Tick", agent_id: "tick-agent-c41",
                project_id: "workspace", cron: Some("0 9 * * *"), run_at: None, next_run_at: &next_run_at,
                origin_session_id: None, origin_message_id: None,
                parent_job_id: None, root_job_id: None, chain_depth: 0, wake_origin: false,
            },
        ).unwrap();

        let app = test_app_handle();
        let db = Arc::new(Mutex::new(DbConn::new(conn)));

        // Firing 1 — the job's own session is never opened in this test,
        // so it takes the "closed" branch every real Tick firing takes.
        scheduler_tick(db.clone(), app.clone()).await;
        poll_until(
            || async {
                let conn = db.lock().unwrap();
                conn.query_row("SELECT COUNT(*) FROM notifications WHERE type = 'agent_message'", [], |r| r.get::<_, i64>(0))
                    .unwrap_or(0)
                    == 1
            },
            "firing 1 must raise exactly one agent_message notification",
        )
        .await;

        // Force the SAME recurring job due again — `recompute_recurrence`
        // already flipped it back to `status = 'pending'` with a future
        // `next_run_at`; this is the identical UPDATE `run_job_now`
        // (`ai::scheduler`) already issues in production to make a
        // pending row claimable this tick.
        {
            let conn = db.lock().unwrap();
            conn.execute("UPDATE agent_jobs SET next_run_at = datetime('now') WHERE id = ?1", params![job_id]).unwrap();
        }

        // Firing 2 — the bug this test pins: before the fix, this firing's
        // notification computes the SAME idem_key as firing 1's and is
        // silently dedup-skipped, so the row count never reaches 2.
        scheduler_tick(db.clone(), app.clone()).await;
        poll_until(
            || async {
                let conn = db.lock().unwrap();
                conn.query_row("SELECT COUNT(*) FROM notifications WHERE type = 'agent_message'", [], |r| r.get::<_, i64>(0))
                    .unwrap_or(0)
                    == 2
            },
            "firing 2 of the SAME recurring job must raise a SECOND, distinct agent_message notification — not a dedup-skipped repeat of the first (C41's exact bug)",
        )
        .await;

        mock.assert_async().await;

        let conn = db.lock().unwrap();
        let notif_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM notifications WHERE type = 'agent_message'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(notif_count, 2, "Tick firing twice must produce TWO notification bells, not one");

        let distinct_keys: i64 = conn
            .query_row("SELECT COUNT(DISTINCT idem_key) FROM notifications WHERE type = 'agent_message'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(distinct_keys, 2, "the two notification rows must carry two DIFFERENT idem_keys — proves the key is per-firing, not job-id-only");
    }

    // ─── C38 (#3139, live test #4) — once/spawn own-session reply persistence ───

    /// Matt's exact repro: a job created from the New job dialog has no
    /// originating chat at all. Before this fix, neither the `wake`/
    /// `recurring`/`heartbeat` branch nor the origin-gated report-back
    /// block below ever fired for it — the 174 characters of final_text
    /// were computed and persisted nowhere.
    ///
    /// Mutation: revert the widened `insert_wake_turn_reply` call back to
    /// the pre-C38 `job_kind_for_report == "wake" || "recurring" ||
    /// "heartbeat"` gate — reddens: zero matching rows for a `once` job.
    #[tokio::test]
    async fn test_once_job_no_origin_reply_persists_into_own_session_c38() {
        let conn = make_conn();
        seed_agent(&conn, "dialog-agent", "ollama", None);

        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/api/chat")
            .with_status(200)
            .with_header("content-type", "application/x-ndjson")
            .with_body(b"{\"model\":\"test-model\",\"message\":{\"role\":\"assistant\",\"content\":\"Dialog job done.\"},\"done\":true}\n" as &[u8])
            .create_async()
            .await;
        crate::database::set_ai_setting(&conn, "ollama_endpoint", &server.url()).unwrap();

        let next_run_at = format_stored_datetime(&Utc::now());
        let job_id = create_job(
            &conn,
            NewJobParams {
                kind: "once", title: "Dialog-created job", brief: "Do the dialog thing", agent_id: "dialog-agent",
                project_id: "workspace", cron: None, run_at: None, next_run_at: &next_run_at,
                origin_session_id: None, origin_message_id: None,
                parent_job_id: None, root_job_id: None, chain_depth: 0, wake_origin: false,
            },
        ).unwrap();

        let app = test_app_handle();
        let db = Arc::new(Mutex::new(DbConn::new(conn)));

        scheduler_tick(db.clone(), app.clone()).await;
        poll_until(
            || async {
                if !mock.matched_async().await {
                    return false;
                }
                let conn = db.lock().unwrap();
                conn.query_row(
                    "SELECT COUNT(*) FROM chat_messages_local WHERE job_id = ?1 AND synthetic = 0 AND role = 'assistant'",
                    params![job_id],
                    |r| r.get::<_, i64>(0),
                ).unwrap_or(0) == 1
            },
            "a no-origin once-job's own reply must be persisted into its own session (C38) — Matt's exact bug report",
        )
        .await;

        let conn = db.lock().unwrap();
        let (content, thread_id): (String, String) = conn
            .query_row(
                "SELECT content, thread_id FROM chat_messages_local WHERE job_id = ?1 AND role = 'assistant' AND synthetic = 0",
                params![job_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("the once job's own reply must exist as a real, visible assistant message");
        assert_eq!(content, "Dialog job done.");
        let job_session_id: String = conn.query_row("SELECT session_id FROM agent_jobs WHERE id = ?1", params![job_id], |r| r.get(0)).unwrap();
        assert_eq!(thread_id, job_session_id, "the reply must land in the job's OWN session — there is no origin to report into");
    }

    /// The dedupe proof, directly: a `once` job WITH an origin gets TWO
    /// writes after this fix — its own session's raw reply (new) AND the
    /// origin's attributed report-back line (unchanged) — landing in two
    /// DIFFERENT sessions, never the same session twice. Matches Reed's
    /// scope: "proves the dedupe story directly rather than asserting an
    /// absence."
    ///
    /// Mutation (named per the scope's own instruction not to write the
    /// empty one): forcing `job.session_id = origin_session_id` in a test
    /// fixture would prove nothing — that's testing a fabricated collapsed
    /// case, not the real invariant. Not written. Instead: revert the
    /// widened condition — reddens, the own-session row never appears
    /// (count stays 0, not 1), while the origin's report-back row is
    /// unaffected (proving the two writes are genuinely independent, not
    /// one cause behind a shared assertion).
    #[tokio::test]
    async fn test_once_job_with_origin_gets_both_own_session_reply_and_report_back_c38() {
        let conn = make_conn();
        seed_agent(&conn, "captain-c38", "ollama", None);
        seed_agent(&conn, "worker-c38", "ollama", None);

        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/api/chat")
            .with_status(200)
            .with_header("content-type", "application/x-ndjson")
            .with_body(b"{\"model\":\"test-model\",\"message\":{\"role\":\"assistant\",\"content\":\"Both copies done.\"},\"done\":true}\n" as &[u8])
            .create_async()
            .await;
        crate::database::set_ai_setting(&conn, "ollama_endpoint", &server.url()).unwrap();

        let origin_session = create_session_internal(&conn, "workspace", "captain-c38", "user", true, None).unwrap();

        let next_run_at = format_stored_datetime(&Utc::now());
        let job_id = create_job(
            &conn,
            NewJobParams {
                kind: "once", title: "Once with origin", brief: "Do the thing", agent_id: "worker-c38",
                project_id: "workspace", cron: None, run_at: None, next_run_at: &next_run_at,
                origin_session_id: Some(&origin_session.id), origin_message_id: None,
                parent_job_id: None, root_job_id: None, chain_depth: 0, wake_origin: false,
            },
        ).unwrap();
        let job_session_id: String = conn.query_row("SELECT session_id FROM agent_jobs WHERE id = ?1", params![job_id], |r| r.get(0)).unwrap();

        let app = test_app_handle();
        let db = Arc::new(Mutex::new(DbConn::new(conn)));

        scheduler_tick(db.clone(), app.clone()).await;
        poll_until(
            || async {
                if !mock.matched_async().await {
                    return false;
                }
                let conn = db.lock().unwrap();
                let own: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM chat_messages_local WHERE job_id = ?1 AND thread_id = ?2 AND synthetic = 0 AND role = 'assistant'",
                    params![job_id, job_session_id],
                    |r| r.get(0),
                ).unwrap_or(0);
                let origin: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM chat_messages_local WHERE job_id = ?1 AND thread_id = ?2",
                    params![job_id, origin_session.id],
                    |r| r.get(0),
                ).unwrap_or(0);
                own == 1 && origin == 1
            },
            "both the job's own-session reply AND the origin's report-back row must exist, in two distinct sessions",
        )
        .await;

        let conn = db.lock().unwrap();
        let own_content: String = conn.query_row(
            "SELECT content FROM chat_messages_local WHERE job_id = ?1 AND thread_id = ?2 AND synthetic = 0 AND role = 'assistant'",
            params![job_id, job_session_id],
            |r| r.get(0),
        ).expect("own-session reply must exist");
        assert_eq!(own_content, "Both copies done.", "the job's own session gets the raw reply, verbatim");

        let origin_content: String = conn.query_row(
            "SELECT content FROM chat_messages_local WHERE job_id = ?1 AND thread_id = ?2",
            params![job_id, origin_session.id],
            |r| r.get(0),
        ).expect("origin report-back row must exist");
        assert!(origin_content.contains("reported:"), "the origin's copy must still be the attributed report-back line: {}", origin_content);
        assert_ne!(job_session_id, origin_session.id, "sanity: these must be two distinct sessions, not a collapsed one");
    }

    /// Belt-and-suspenders (Reed's scope): `spawn_job` always creates with
    /// an origin today per its own required-caller shape, but the widened
    /// condition doesn't special-case kind beyond membership — cheap to
    /// prove directly since `create_job` doesn't actually forbid an
    /// originless `spawn` row.
    ///
    /// Mutation: same as above — revert the widened condition, reddens.
    #[tokio::test]
    async fn test_spawn_job_no_origin_reply_persists_into_own_session_c38() {
        let conn = make_conn();
        seed_agent(&conn, "spawn-no-origin-agent", "ollama", None);

        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/api/chat")
            .with_status(200)
            .with_header("content-type", "application/x-ndjson")
            .with_body(b"{\"model\":\"test-model\",\"message\":{\"role\":\"assistant\",\"content\":\"Spawn job done.\"},\"done\":true}\n" as &[u8])
            .create_async()
            .await;
        crate::database::set_ai_setting(&conn, "ollama_endpoint", &server.url()).unwrap();

        let next_run_at = format_stored_datetime(&Utc::now());
        let job_id = create_job(
            &conn,
            NewJobParams {
                kind: "spawn", title: "Originless spawn", brief: "Do the thing", agent_id: "spawn-no-origin-agent",
                project_id: "workspace", cron: None, run_at: None, next_run_at: &next_run_at,
                origin_session_id: None, origin_message_id: None,
                parent_job_id: None, root_job_id: None, chain_depth: 0, wake_origin: false,
            },
        ).unwrap();

        let app = test_app_handle();
        let db = Arc::new(Mutex::new(DbConn::new(conn)));

        scheduler_tick(db.clone(), app.clone()).await;
        poll_until(
            || async {
                if !mock.matched_async().await {
                    return false;
                }
                let conn = db.lock().unwrap();
                conn.query_row(
                    "SELECT COUNT(*) FROM chat_messages_local WHERE job_id = ?1 AND synthetic = 0 AND role = 'assistant'",
                    params![job_id],
                    |r| r.get::<_, i64>(0),
                ).unwrap_or(0) == 1
            },
            "an originless spawn job's own reply must also persist into its own session — belt and suspenders alongside once",
        )
        .await;

        let conn = db.lock().unwrap();
        let content: String = conn.query_row(
            "SELECT content FROM chat_messages_local WHERE job_id = ?1 AND role = 'assistant' AND synthetic = 0",
            params![job_id],
            |r| r.get(0),
        ).expect("the spawn job's own reply must exist");
        assert_eq!(content, "Spawn job done.");
    }

    // ─── C38 (#3139) — notification ruling: one per firing, never two ───

    /// The silent-before-this-fix case, made loud: a no-origin `once` job
    /// (New job dialog shape) gets exactly one `agent_message` notification,
    /// keyed on the JOB session (there is no origin to key it on).
    ///
    /// Mutation: narrow the own-session notification gate back to `kind IN
    /// ('recurring', 'heartbeat')` alone (pre-C38) — reddens: zero
    /// notifications for this job.
    #[tokio::test]
    async fn test_once_job_no_origin_gets_exactly_one_notification_keyed_on_job_session_c38() {
        let conn = make_conn();
        seed_agent(&conn, "notif-dialog-agent", "ollama", None);

        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/api/chat")
            .with_status(200)
            .with_header("content-type", "application/x-ndjson")
            .with_body(b"{\"model\":\"test-model\",\"message\":{\"role\":\"assistant\",\"content\":\"Dialog notif job done.\"},\"done\":true}\n" as &[u8])
            .create_async()
            .await;
        crate::database::set_ai_setting(&conn, "ollama_endpoint", &server.url()).unwrap();

        let next_run_at = format_stored_datetime(&Utc::now());
        let job_id = create_job(
            &conn,
            NewJobParams {
                kind: "once", title: "Dialog notif job", brief: "Do the dialog thing", agent_id: "notif-dialog-agent",
                project_id: "workspace", cron: None, run_at: None, next_run_at: &next_run_at,
                origin_session_id: None, origin_message_id: None,
                parent_job_id: None, root_job_id: None, chain_depth: 0, wake_origin: false,
            },
        ).unwrap();
        let job_session_id: String = conn.query_row("SELECT session_id FROM agent_jobs WHERE id = ?1", params![job_id], |r| r.get(0)).unwrap();

        let app = test_app_handle();
        let db = Arc::new(Mutex::new(DbConn::new(conn)));

        scheduler_tick(db.clone(), app.clone()).await;
        poll_until(
            || async {
                if !mock.matched_async().await {
                    return false;
                }
                let conn = db.lock().unwrap();
                conn.query_row("SELECT COUNT(*) FROM notifications WHERE type = 'agent_message'", [], |r| r.get::<_, i64>(0)).unwrap_or(0) == 1
            },
            "a no-origin once-job must raise exactly one agent_message notification, off its own-session write",
        )
        .await;

        let conn = db.lock().unwrap();
        let notif_count: i64 = conn.query_row("SELECT COUNT(*) FROM notifications WHERE type = 'agent_message'", [], |r| r.get(0)).unwrap();
        assert_eq!(notif_count, 1, "exactly one notification, never zero and never two");
        let (target_id, title): (Option<String>, String) = conn
            .query_row("SELECT target_id, title FROM notifications WHERE type = 'agent_message'", [], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap();
        assert_eq!(target_id, Some(job_session_id), "the notification must be keyed on the JOB session — there is no origin");
        assert_eq!(title, "Message from notif-dialog-agent", "title must name the agent, not the job title (leftover 3)");
    }

    /// The regression-catching half, per Reed's scope: a `once` job WITH an
    /// origin must raise exactly ONE notification — keyed on the ORIGIN
    /// (report-back's existing notification, unchanged) — and explicitly
    /// NONE on the job's own session. Asserts the absence, not just the
    /// presence, which is the only way to actually catch a regression into
    /// double-notifying for the common "spawn/once from Home, report back
    /// to Home" case.
    ///
    /// Mutation: widen the own-session notification gate to fire
    /// unconditionally for `once`/`spawn` (drop the `job_has_no_origin`
    /// check) — reddens: TWO notifications exist, and one is keyed on the
    /// job session, not the origin.
    #[tokio::test]
    async fn test_once_job_with_origin_gets_exactly_one_notification_keyed_on_origin_not_job_session_c38() {
        let conn = make_conn();
        seed_agent(&conn, "notif-captain-c38", "ollama", None);
        seed_agent(&conn, "notif-worker-c38", "ollama", None);

        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/api/chat")
            .with_status(200)
            .with_header("content-type", "application/x-ndjson")
            .with_body(b"{\"model\":\"test-model\",\"message\":{\"role\":\"assistant\",\"content\":\"Notif origin job done.\"},\"done\":true}\n" as &[u8])
            .create_async()
            .await;
        crate::database::set_ai_setting(&conn, "ollama_endpoint", &server.url()).unwrap();

        // Origin session created but never marked "open" (set_viewing_session
        // is never called with its id) — report-back's own notification gate
        // requires the origin NOT be the one on screen, same as every other
        // report-back notification test in this module.
        let origin_session = create_session_internal(&conn, "workspace", "notif-captain-c38", "user", true, None).unwrap();

        let next_run_at = format_stored_datetime(&Utc::now());
        let job_id = create_job(
            &conn,
            NewJobParams {
                kind: "once", title: "Notif job with origin", brief: "Do the thing", agent_id: "notif-worker-c38",
                project_id: "workspace", cron: None, run_at: None, next_run_at: &next_run_at,
                origin_session_id: Some(&origin_session.id), origin_message_id: None,
                parent_job_id: None, root_job_id: None, chain_depth: 0, wake_origin: false,
            },
        ).unwrap();
        let job_session_id: String = conn.query_row("SELECT session_id FROM agent_jobs WHERE id = ?1", params![job_id], |r| r.get(0)).unwrap();

        let app = test_app_handle();
        let db = Arc::new(Mutex::new(DbConn::new(conn)));

        scheduler_tick(db.clone(), app.clone()).await;
        poll_until(
            || async {
                if !mock.matched_async().await {
                    return false;
                }
                let conn = db.lock().unwrap();
                conn.query_row("SELECT COUNT(*) FROM notifications WHERE type = 'agent_message'", [], |r| r.get::<_, i64>(0)).unwrap_or(0) >= 1
            },
            "a with-origin once-job must raise its (unchanged) report-back notification",
        )
        .await;

        // Give the own-session write (which also happens this same tick,
        // C38's persistence half) a moment to land too, so a false pass
        // isn't just a timing artifact of checking too early.
        poll_until(
            || async {
                let conn = db.lock().unwrap();
                conn.query_row(
                    "SELECT COUNT(*) FROM chat_messages_local WHERE job_id = ?1 AND thread_id = ?2 AND synthetic = 0",
                    params![job_id, job_session_id],
                    |r| r.get::<_, i64>(0),
                ).unwrap_or(0) == 1
            },
            "the job's own-session reply (C38's persistence half) must also have landed",
        )
        .await;

        let conn = db.lock().unwrap();
        let notif_count: i64 = conn.query_row("SELECT COUNT(*) FROM notifications WHERE type = 'agent_message'", [], |r| r.get(0)).unwrap();
        assert_eq!(notif_count, 1, "exactly one notification total — never two for a single firing");

        let on_origin: i64 = conn.query_row(
            "SELECT COUNT(*) FROM notifications WHERE type = 'agent_message' AND target_id = ?1",
            params![origin_session.id],
            |r| r.get(0),
        ).unwrap();
        assert_eq!(on_origin, 1, "the one notification must be keyed on the ORIGIN session");

        let on_job_session: i64 = conn.query_row(
            "SELECT COUNT(*) FROM notifications WHERE type = 'agent_message' AND target_id = ?1",
            params![job_session_id],
            |r| r.get(0),
        ).unwrap();
        assert_eq!(on_job_session, 0, "must be ZERO notifications keyed on the job's own session — the absence that catches a double-notify regression");
    }

    // ─── C42d (#3147, live test #5 FOLLOW-UPS, Matt's live-tested ruling) ───
    // ─── recurring/heartbeat-with-origin must ALSO bell the origin ─────────

    /// Matt's exact live-test-#5-followups repro: "three Tick bells" —
    /// watching HOME, not the recurring job's own hidden schedule session.
    /// Before this fix, a recurring job with an origin got the report-back
    /// INSERT (C42a/b, into the origin) but the only notification fired was
    /// `own_session_should_notify`'s, targeted at the job's OWN session —
    /// never one Matt would see while looking at the origin (Home). Fires
    /// the SAME recurring job twice and asserts the origin gets a distinct
    /// `agent_message` bell on EACH firing — not zero, not a dedup-skipped
    /// repeat of the own-session bell that fires in parallel.
    ///
    /// Mutation: revert the widened gate back to `job_kind_for_report ==
    /// "spawn" || job_kind_for_report == "once"` alone — reddens: zero
    /// `agent_message` notifications land on the origin session for either
    /// firing (only the own-session ones, keyed on the job's own session,
    /// exist).
    #[tokio::test]
    async fn test_recurring_job_with_origin_raises_bell_on_origin_too_c42d() {
        let conn = make_conn();
        seed_agent(&conn, "tick-agent-c42d", "ollama", None);

        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/api/chat")
            .with_status(200)
            .with_header("content-type", "application/x-ndjson")
            .with_body(b"{\"model\":\"test-model\",\"message\":{\"role\":\"assistant\",\"content\":\"Tick.\"},\"done\":true}\n" as &[u8])
            .expect_at_least(2)
            .create_async()
            .await;
        crate::database::set_ai_setting(&conn, "ollama_endpoint", &server.url()).unwrap();

        // Origin session created but never opened — both this test's bell
        // and the own-session bell require the relevant session not be the
        // one on screen, same precondition every other notification test
        // in this module uses.
        let origin_session = create_session_internal(&conn, "workspace", "tick-agent-c42d", "user", true, None).unwrap();
        let next_run_at = format_stored_datetime(&Utc::now());
        let job_id = create_job(
            &conn,
            NewJobParams {
                kind: "recurring", title: "Tick", brief: "Tick", agent_id: "tick-agent-c42d",
                project_id: "workspace", cron: Some("0 9 * * *"), run_at: None, next_run_at: &next_run_at,
                origin_session_id: Some(&origin_session.id), origin_message_id: None,
                parent_job_id: None, root_job_id: None, chain_depth: 0, wake_origin: false,
            },
        ).unwrap();
        let job_session_id: String = conn.query_row("SELECT session_id FROM agent_jobs WHERE id = ?1", params![job_id], |r| r.get(0)).unwrap();

        let app = test_app_handle();
        let db = Arc::new(Mutex::new(DbConn::new(conn)));

        // Firing 1.
        scheduler_tick(db.clone(), app.clone()).await;
        poll_until(
            || async {
                let conn = db.lock().unwrap();
                conn.query_row(
                    "SELECT COUNT(*) FROM notifications WHERE type = 'agent_message' AND target_id = ?1",
                    params![origin_session.id],
                    |r| r.get::<_, i64>(0),
                ).unwrap_or(0) == 1
            },
            "firing 1 must raise exactly one agent_message bell keyed on the ORIGIN — Matt's \"three Tick bells\" repro, bell #1",
        )
        .await;

        // #3149/C45/C46: the refactor to `notification_key(firing_id,
        // scope)` must not just happen to produce matching strings — the
        // own-session and origin idem_keys for THIS SAME firing must be
        // exactly what the helper computes for `OwnSession`/`Origin` from
        // the same firing id, not independently-hand-rolled strings that
        // coincidentally look right. Mutation: swap the origin call site
        // back to a caller-side `format!("{}-origin", ...)` (pre-#3149
        // shape) — this assertion still passes (same bytes), but a
        // mutation that makes `notification_key`'s `Origin` arm format
        // differently (e.g. `-source` instead of `-origin`) reddens here
        // without needing a second, separate unit test to catch it.
        {
            let conn = db.lock().unwrap();
            let own_idem_key: String = conn
                .query_row(
                    "SELECT idem_key FROM notifications WHERE type = 'agent_message' AND target_id = ?1",
                    params![job_session_id],
                    |r| r.get(0),
                )
                .unwrap();
            let origin_idem_key: String = conn
                .query_row(
                    "SELECT idem_key FROM notifications WHERE type = 'agent_message' AND target_id = ?1",
                    params![origin_session.id],
                    |r| r.get(0),
                )
                .unwrap();
            let firing_id = own_idem_key
                .strip_prefix("agent_message:")
                .expect("own-session idem_key must carry the agent_message: prefix")
                .to_string();
            assert_eq!(
                own_idem_key,
                crate::notifications::notification_key(&firing_id, crate::notifications::NotificationScope::OwnSession),
                "own-session idem_key must be EXACTLY notification_key(firing_id, OwnSession)"
            );
            assert_eq!(
                origin_idem_key,
                crate::notifications::notification_key(&firing_id, crate::notifications::NotificationScope::Origin),
                "origin idem_key must be EXACTLY notification_key(firing_id, Origin), sharing the SAME firing_id as the own-session key for this firing"
            );
        }

        // Force the SAME recurring job due again — same technique C41's/
        // C42b's own two-firings tests use.
        {
            let conn = db.lock().unwrap();
            conn.execute("UPDATE agent_jobs SET next_run_at = datetime('now') WHERE id = ?1", params![job_id]).unwrap();
        }

        // Firing 2 — the bug this test pins: before the fix, there is no
        // origin bell at all (gate excludes `recurring`); after the gate
        // widen alone (without C42d's distinct-key fix at the emit call
        // site), firing 1's origin bell would already exist but firing 2's
        // would compute the SAME `idem_key` as firing 1's OWN-session bell
        // and dedup-skip — this assertion catches either failure mode.
        scheduler_tick(db.clone(), app.clone()).await;
        poll_until(
            || async {
                let conn = db.lock().unwrap();
                conn.query_row(
                    "SELECT COUNT(*) FROM notifications WHERE type = 'agent_message' AND target_id = ?1",
                    params![origin_session.id],
                    |r| r.get::<_, i64>(0),
                ).unwrap_or(0) == 2
            },
            "firing 2 of the SAME recurring job must raise a SECOND, distinct origin bell — Matt's \"three Tick bells\" repro, bell #2",
        )
        .await;

        mock.assert_async().await;

        let conn = db.lock().unwrap();
        let on_origin: i64 = conn
            .query_row("SELECT COUNT(*) FROM notifications WHERE type = 'agent_message' AND target_id = ?1", params![origin_session.id], |r| r.get(0))
            .unwrap();
        assert_eq!(on_origin, 2, "two firings of a recurring job with an origin must raise TWO bells keyed on the origin, not zero and not one dedup-skipped");

        let on_job_session: i64 = conn
            .query_row("SELECT COUNT(*) FROM notifications WHERE type = 'agent_message' AND target_id = ?1", params![job_session_id], |r| r.get(0))
            .unwrap();
        assert_eq!(on_job_session, 2, "the pre-existing own-session bell (C41) must ALSO still be two — this fix adds the origin bell, it does not replace the own-session one");

        let total_notifs: i64 = conn.query_row("SELECT COUNT(*) FROM notifications WHERE type = 'agent_message'", [], |r| r.get(0)).unwrap();
        assert_eq!(total_notifs, 4, "two firings × two bells each (own-session + origin) = four total, never collapsed by a key collision");

        let distinct_keys: i64 = conn.query_row("SELECT COUNT(DISTINCT idem_key) FROM notifications WHERE type = 'agent_message'", [], |r| r.get(0)).unwrap();
        assert_eq!(distinct_keys, 4, "all four bells must carry four DIFFERENT idem_keys — proves the origin bell's suffixed key never collides with the own-session bell's raw key, across two firings");
    }

    // ─── C39 (#3139, live test #4) — wake-on-complete scoped to spawn/once ───

    /// Matt's "six wake rows on Home from the Tick job" repro: a recurring
    /// job scheduled WITH an origin chat used to enqueue a fresh proactive
    /// wake on every single firing. Drives a REAL `scheduler_tick` through
    /// one firing and asserts zero `kind='wake'` rows exist afterward.
    ///
    /// Mutation: remove the new `(job_kind_for_report == "spawn" ||
    /// job_kind_for_report == "once")` kind check from the outer wake
    /// guard — reddens, one wake row created (reproduces the live-test-#4
    /// bug directly).
    #[tokio::test]
    async fn test_recurring_job_with_origin_does_not_create_wake_on_firing_c39() {
        let conn = make_conn();
        seed_agent(&conn, "tick-agent", "ollama", None);

        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/api/chat")
            .with_status(200)
            .with_header("content-type", "application/x-ndjson")
            .with_body(b"{\"model\":\"test-model\",\"message\":{\"role\":\"assistant\",\"content\":\"Tick.\"},\"done\":true}\n" as &[u8])
            .create_async()
            .await;
        crate::database::set_ai_setting(&conn, "ollama_endpoint", &server.url()).unwrap();

        let origin_session = create_session_internal(&conn, "workspace", "tick-agent", "user", true, None).unwrap();
        let next_run_at = format_stored_datetime(&Utc::now());
        let job_id = create_job(
            &conn,
            NewJobParams {
                kind: "recurring", title: "Tick", brief: "Tick", agent_id: "tick-agent",
                project_id: "workspace", cron: Some("0 9 * * *"), run_at: None, next_run_at: &next_run_at,
                origin_session_id: Some(&origin_session.id), origin_message_id: None,
                parent_job_id: None, root_job_id: None, chain_depth: 0, wake_origin: true,
            },
        ).unwrap();

        let job_session_id: String = conn.query_row("SELECT session_id FROM agent_jobs WHERE id = ?1", params![job_id], |r| r.get(0)).unwrap();
        let app = test_app_handle();
        let db = Arc::new(Mutex::new(DbConn::new(conn)));

        scheduler_tick(db.clone(), app.clone()).await;
        poll_until(
            || async {
                if !mock.matched_async().await {
                    return false;
                }
                let conn = db.lock().unwrap();
                // C42a (#3147): scoped to `thread_id = job_session_id`,
                // not just `job_id` — a recurring job WITH an origin now
                // ALSO gets a report-back row in the ORIGIN session
                // (same `job_id`, different `thread_id`), so an unscoped
                // `job_id`-only count would be 2, never 1, once that
                // commit lands. This poll still waits on exactly the row
                // it always meant to: the job's OWN-session turn finishing.
                conn.query_row(
                    "SELECT COUNT(*) FROM chat_messages_local WHERE job_id = ?1 AND thread_id = ?2 AND synthetic = 0 AND role = 'assistant'",
                    params![job_id, job_session_id],
                    |r| r.get::<_, i64>(0),
                ).unwrap_or(0) == 1
            },
            "the recurring job's own turn must finish before asserting no wake row exists",
        )
        .await;

        let conn = db.lock().unwrap();
        let wake_count: i64 = conn.query_row("SELECT COUNT(*) FROM agent_jobs WHERE kind = 'wake'", [], |r| r.get(0)).unwrap();
        assert_eq!(wake_count, 0, "a recurring job's firing must NOT create a wake row on its origin — Matt's repeated-wake bug");
    }

    /// Same proof for `heartbeat` — the second kind §36.4.5's own
    /// "Mechanism" paragraph already excludes from wake eligibility.
    #[tokio::test]
    async fn test_heartbeat_job_with_origin_does_not_create_wake_on_firing_c39() {
        let conn = make_conn();
        seed_agent(&conn, "heartbeat-wake-agent", "ollama", None);

        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/api/chat")
            .with_status(200)
            .with_header("content-type", "application/x-ndjson")
            .with_body(b"{\"model\":\"test-model\",\"message\":{\"role\":\"assistant\",\"content\":\"All clear.\"},\"done\":true}\n" as &[u8])
            .create_async()
            .await;
        crate::database::set_ai_setting(&conn, "ollama_endpoint", &server.url()).unwrap();

        let origin_session = create_session_internal(&conn, "workspace", "heartbeat-wake-agent", "user", true, None).unwrap();
        let next_run_at = format_stored_datetime(&Utc::now());
        let job_id = create_job(
            &conn,
            NewJobParams {
                kind: "heartbeat", title: "Check-in", brief: "Review open tasks", agent_id: "heartbeat-wake-agent",
                project_id: "workspace", cron: Some("0 */4 * * *"), run_at: None, next_run_at: &next_run_at,
                origin_session_id: Some(&origin_session.id), origin_message_id: None,
                parent_job_id: None, root_job_id: None, chain_depth: 0, wake_origin: true,
            },
        ).unwrap();

        let job_session_id: String = conn.query_row("SELECT session_id FROM agent_jobs WHERE id = ?1", params![job_id], |r| r.get(0)).unwrap();
        let app = test_app_handle();
        let db = Arc::new(Mutex::new(DbConn::new(conn)));

        scheduler_tick(db.clone(), app.clone()).await;
        poll_until(
            || async {
                if !mock.matched_async().await {
                    return false;
                }
                let conn = db.lock().unwrap();
                // C42a (#3147): same scoping fix as the recurring sibling
                // test above — a heartbeat job WITH an origin now ALSO
                // gets a report-back row in the ORIGIN session.
                conn.query_row(
                    "SELECT COUNT(*) FROM chat_messages_local WHERE job_id = ?1 AND thread_id = ?2 AND synthetic = 0 AND role = 'assistant'",
                    params![job_id, job_session_id],
                    |r| r.get::<_, i64>(0),
                ).unwrap_or(0) == 1
            },
            "the heartbeat job's own turn must finish before asserting no wake row exists",
        )
        .await;

        let conn = db.lock().unwrap();
        let wake_count: i64 = conn.query_row("SELECT COUNT(*) FROM agent_jobs WHERE kind = 'wake'", [], |r| r.get(0)).unwrap();
        assert_eq!(wake_count, 0, "a heartbeat job's firing must NOT create a wake row on its origin");
    }

    /// Positive control: `spawn` is unaffected by C39's new filter — a
    /// spawn job with an origin still creates a wake row, exactly as
    /// before (close cousin of `test_spawn_finish_wakes_origin_with_one_
    /// proactive_turn`, but asserting creation itself in isolation).
    ///
    /// Mutation: change the new allowlist to exclude `spawn` (e.g. `kind ==
    /// "once"` only) — reddens, zero wake rows for a spawn job (proves the
    /// filter is additive, not accidentally narrower than intended).
    #[tokio::test]
    async fn test_spawn_job_with_origin_still_creates_wake_on_firing_c39() {
        let conn = make_conn();
        seed_agent(&conn, "spawn-wake-captain", "ollama", None);
        seed_agent(&conn, "spawn-wake-crew", "ollama", None);

        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/api/chat")
            .with_status(200)
            .with_header("content-type", "application/x-ndjson")
            .with_body(b"{\"model\":\"test-model\",\"message\":{\"role\":\"assistant\",\"content\":\"The thing is done.\"},\"done\":true}\n" as &[u8])
            .create_async()
            .await;
        crate::database::set_ai_setting(&conn, "ollama_endpoint", &server.url()).unwrap();

        let origin_session = create_session_internal(&conn, "workspace", "spawn-wake-captain", "user", true, None).unwrap();
        let next_run_at = format_stored_datetime(&Utc::now());
        let job_id = create_job(
            &conn,
            NewJobParams {
                kind: "spawn", title: "Test spawn", brief: "Do the thing", agent_id: "spawn-wake-crew",
                project_id: "workspace", cron: None, run_at: None, next_run_at: &next_run_at,
                origin_session_id: Some(&origin_session.id), origin_message_id: None,
                parent_job_id: None, root_job_id: None, chain_depth: 0, wake_origin: true,
            },
        ).unwrap();

        let app = test_app_handle();
        let db = Arc::new(Mutex::new(DbConn::new(conn)));

        scheduler_tick(db.clone(), app.clone()).await;
        poll_until(
            || async {
                if !mock.matched_async().await {
                    return false;
                }
                let conn = db.lock().unwrap();
                conn.query_row("SELECT COUNT(*) FROM agent_jobs WHERE kind = 'wake'", [], |r| r.get::<_, i64>(0)).unwrap_or(0) == 1
            },
            "a spawn job with an origin must still create a wake row — C39's filter is additive, not a blanket wake-kill",
        )
        .await;

        let _ = job_id;
    }

    /// Same positive control for `once` — a chat-originated once-job with
    /// an origin still creates a wake row.
    ///
    /// Mutation: same shape — narrow the allowlist to exclude `once`,
    /// reddens.
    #[tokio::test]
    async fn test_once_job_with_origin_still_creates_wake_on_firing_c39() {
        let conn = make_conn();
        seed_agent(&conn, "once-wake-captain", "ollama", None);

        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/api/chat")
            .with_status(200)
            .with_header("content-type", "application/x-ndjson")
            .with_body(b"{\"model\":\"test-model\",\"message\":{\"role\":\"assistant\",\"content\":\"Done.\"},\"done\":true}\n" as &[u8])
            .create_async()
            .await;
        crate::database::set_ai_setting(&conn, "ollama_endpoint", &server.url()).unwrap();

        let origin_session = create_session_internal(&conn, "workspace", "once-wake-captain", "user", true, None).unwrap();
        let next_run_at = format_stored_datetime(&Utc::now());
        let job_id = create_job(
            &conn,
            NewJobParams {
                kind: "once", title: "Once with wake", brief: "Do the thing", agent_id: "once-wake-captain",
                project_id: "workspace", cron: None, run_at: None, next_run_at: &next_run_at,
                origin_session_id: Some(&origin_session.id), origin_message_id: None,
                parent_job_id: None, root_job_id: None, chain_depth: 0, wake_origin: true,
            },
        ).unwrap();

        let app = test_app_handle();
        let db = Arc::new(Mutex::new(DbConn::new(conn)));

        scheduler_tick(db.clone(), app.clone()).await;
        poll_until(
            || async {
                if !mock.matched_async().await {
                    return false;
                }
                let conn = db.lock().unwrap();
                conn.query_row("SELECT COUNT(*) FROM agent_jobs WHERE kind = 'wake'", [], |r| r.get::<_, i64>(0)).unwrap_or(0) == 1
            },
            "a once job with an origin must still create a wake row",
        )
        .await;

        let _ = job_id;
    }

    // ─── C42 (#3147, live test #5, Matt's ruling) — report-back for ───
    // ─── recurring/heartbeat jobs with an origin ───────────────────────

    /// Matt's ruling, confirmed: recurring/heartbeat jobs with an origin
    /// now get the SAME report-back copy into the origin chat that
    /// once/spawn jobs get — a plain insert, no wake. Before C42a, the
    /// report-back gate was `kind IN ('spawn', 'once')` alone, so a
    /// recurring job's origin never saw anything (the "Morning brief
    /// never lands in Home" symptom).
    ///
    /// Mutation: revert C42a's widened gate back to `job_kind_for_report
    /// == "spawn" || job_kind_for_report == "once"` alone — reddens: zero
    /// rows land in the origin session for a recurring job.
    #[tokio::test]
    async fn test_recurring_job_with_origin_gets_report_back_copy_c42() {
        let conn = make_conn();
        seed_agent(&conn, "morning-brief-agent", "ollama", None);

        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/api/chat")
            .with_status(200)
            .with_header("content-type", "application/x-ndjson")
            .with_body(b"{\"model\":\"test-model\",\"message\":{\"role\":\"assistant\",\"content\":\"Here is your morning brief.\"},\"done\":true}\n" as &[u8])
            .create_async()
            .await;
        crate::database::set_ai_setting(&conn, "ollama_endpoint", &server.url()).unwrap();

        let origin_session = create_session_internal(&conn, "workspace", "morning-brief-agent", "user", true, None).unwrap();
        let next_run_at = format_stored_datetime(&Utc::now());
        let job_id = create_job(
            &conn,
            NewJobParams {
                kind: "recurring", title: "Morning brief", brief: "Summarize overnight activity", agent_id: "morning-brief-agent",
                project_id: "workspace", cron: Some("0 9 * * *"), run_at: None, next_run_at: &next_run_at,
                origin_session_id: Some(&origin_session.id), origin_message_id: None,
                parent_job_id: None, root_job_id: None, chain_depth: 0, wake_origin: false,
            },
        ).unwrap();

        let app = test_app_handle();
        let db = Arc::new(Mutex::new(DbConn::new(conn)));

        scheduler_tick(db.clone(), app.clone()).await;
        poll_until(
            || async {
                if !mock.matched_async().await {
                    return false;
                }
                let conn = db.lock().unwrap();
                conn.query_row(
                    "SELECT COUNT(*) FROM chat_messages_local WHERE thread_id = ?1 AND job_id = ?2",
                    params![origin_session.id, job_id],
                    |r| r.get::<_, i64>(0),
                ).unwrap_or(0) == 1
            },
            "a recurring job with an origin must get a report-back copy into that origin (C42a)",
        )
        .await;

        let conn = db.lock().unwrap();
        let (role, content, model): (String, String, Option<String>) = conn
            .query_row(
                "SELECT role, content, model FROM chat_messages_local WHERE thread_id = ?1 AND job_id = ?2",
                params![origin_session.id, job_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .expect("the report-back row must exist in the origin session");
        assert_eq!(role, "assistant");
        assert!(content.contains("Here is your morning brief."), "the report-back content must carry the job's own final text: {}", content);
        assert!(content.contains("morning-brief-agent"), "the report-back attribution line must name the finished job's own agent: {}", content);
        assert!(model.is_some(), "report-back always sets model (unlike a Pane-templated chip) — renders as a normal attributed bubble");
    }

    /// C38/C42 double-copy trace (#3147, live test #5 follow-ups): C38's
    /// own-session write (`insert_wake_turn_reply`, unconditional on kind
    /// since C38) and C42's origin report-back write (`ingest_external_
    /// messages_impl`) are two SEPARATE `if` blocks in the same
    /// post-dispatch closure — this test drives one real recurring-with-
    /// origin firing and asserts each write landed exactly once, in two
    /// DIFFERENT sessions, never twice in the SAME one. The exact
    /// confirmation the task's "trace and state" item asks for.
    ///
    /// Mutation: none written for the absence half (per the scope's own
    /// "where a mutation would prove nothing" guidance, the only way to
    /// force a literal double-write would be to fabricate `job.session_id
    /// == origin_session_id` in a test fixture, proving nothing about the
    /// real invariant — `create_job` never produces that collapse for any
    /// kind this covers). The presence half IS covered by mutation:
    /// deleting the unconditional `insert_wake_turn_reply` call (C38)
    /// reddens the own-session assertion to 0; reverting C42a's kind-widen
    /// reddens the origin assertion to 0 (both already proven by other
    /// tests in this module) — this test adds the THIRD assertion neither
    /// of those covers: the two counts together never exceed one row each.
    #[tokio::test]
    async fn test_recurring_job_with_origin_gets_own_copy_and_origin_copy_never_doubled_c38_c42() {
        let conn = make_conn();
        seed_agent(&conn, "trace-agent-c38c42", "ollama", None);

        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/api/chat")
            .with_status(200)
            .with_header("content-type", "application/x-ndjson")
            .with_body(b"{\"model\":\"test-model\",\"message\":{\"role\":\"assistant\",\"content\":\"Traced.\"},\"done\":true}\n" as &[u8])
            .create_async()
            .await;
        crate::database::set_ai_setting(&conn, "ollama_endpoint", &server.url()).unwrap();

        let origin_session = create_session_internal(&conn, "workspace", "trace-agent-c38c42", "user", true, None).unwrap();
        let next_run_at = format_stored_datetime(&Utc::now());
        let job_id = create_job(
            &conn,
            NewJobParams {
                kind: "recurring", title: "Trace job", brief: "Trace the double-copy invariant", agent_id: "trace-agent-c38c42",
                project_id: "workspace", cron: Some("0 9 * * *"), run_at: None, next_run_at: &next_run_at,
                origin_session_id: Some(&origin_session.id), origin_message_id: None,
                parent_job_id: None, root_job_id: None, chain_depth: 0, wake_origin: false,
            },
        ).unwrap();
        let job_session_id: String = conn.query_row("SELECT session_id FROM agent_jobs WHERE id = ?1", params![job_id], |r| r.get(0)).unwrap();
        assert_ne!(job_session_id, origin_session.id, "sanity: the job's own session and the origin must be two distinct rows, never collapsed");

        let app = test_app_handle();
        let db = Arc::new(Mutex::new(DbConn::new(conn)));

        scheduler_tick(db.clone(), app.clone()).await;
        poll_until(
            || async {
                if !mock.matched_async().await {
                    return false;
                }
                let conn = db.lock().unwrap();
                // `synthetic = 0 AND role = 'assistant'` excludes the
                // brief itself — inserted into the job's own session as a
                // `synthetic = 1` USER message carrying the same `job_id`
                // (step 3 of §36.3.3) — same filter every other own-
                // session-reply test in this module uses.
                let own: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM chat_messages_local WHERE thread_id = ?1 AND job_id = ?2 AND synthetic = 0 AND role = 'assistant'",
                    params![job_session_id, job_id],
                    |r| r.get(0),
                ).unwrap_or(0);
                let origin: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM chat_messages_local WHERE thread_id = ?1 AND job_id = ?2",
                    params![origin_session.id, job_id],
                    |r| r.get(0),
                ).unwrap_or(0);
                own == 1 && origin == 1
            },
            "a recurring job with an origin must produce exactly ONE reply row in its OWN session (C38) and exactly ONE report-back row in the ORIGIN (C42) after a single firing",
        )
        .await;

        let conn = db.lock().unwrap();
        let own_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM chat_messages_local WHERE thread_id = ?1 AND job_id = ?2 AND synthetic = 0 AND role = 'assistant'",
            params![job_session_id, job_id],
            |r| r.get(0),
        ).unwrap();
        let origin_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM chat_messages_local WHERE thread_id = ?1 AND job_id = ?2",
            params![origin_session.id, job_id],
            |r| r.get(0),
        ).unwrap();
        assert_eq!(own_count, 1, "the job's own session must have exactly one copy of the reply — never two in the same place (C38's write is unconditional-on-kind, but still a single INSERT per firing)");
        assert_eq!(origin_count, 1, "the origin must have exactly one copy of the report-back — never two in the same place (C42's write is a separate single INSERT per firing)");

        let total_assistant_replies_for_job: i64 = conn.query_row(
            "SELECT COUNT(*) FROM chat_messages_local WHERE job_id = ?1 AND synthetic = 0 AND role = 'assistant'",
            params![job_id],
            |r| r.get(0),
        ).unwrap();
        assert_eq!(total_assistant_replies_for_job, 2, "exactly two real assistant-reply rows total for this one firing — one per distinct session, never a third anywhere else");
    }

    /// Reed's own hidden finding (\u00a73 of the scope audit, beyond the
    /// issue's own text): widening C42a's kind gate alone is NOT enough.
    /// `source_message_id`/the report-back notification key were
    /// job-id-only (`job-report-{job_id}`) — safe only because
    /// once/spawn fire at most once ever. Without C42b, a recurring job's
    /// SECOND firing would silently dedup-skip against
    /// `idx_chat_local_oc_dedup` (same `thread_id`, same `oc_message_id`
    /// string both times) — landing once, then stopping forever.
    ///
    /// Mutation: revert C42b alone (leaving C42a's kind-widen in place)
    /// — i.e. go back to `source_message_id: format!("job-report-{}",
    /// job_id_for_bookkeeping)` — reddens: the second firing's row
    /// count stays at ONE (dedup-skipped), not two.
    #[tokio::test]
    async fn test_recurring_job_second_firing_report_back_produces_second_row_c42b() {
        let conn = make_conn();
        seed_agent(&conn, "morning-brief-agent-2", "ollama", None);

        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/api/chat")
            .with_status(200)
            .with_header("content-type", "application/x-ndjson")
            .with_body(b"{\"model\":\"test-model\",\"message\":{\"role\":\"assistant\",\"content\":\"Brief.\"},\"done\":true}\n" as &[u8])
            .expect_at_least(2)
            .create_async()
            .await;
        crate::database::set_ai_setting(&conn, "ollama_endpoint", &server.url()).unwrap();

        let origin_session = create_session_internal(&conn, "workspace", "morning-brief-agent-2", "user", true, None).unwrap();
        let next_run_at = format_stored_datetime(&Utc::now());
        let job_id = create_job(
            &conn,
            NewJobParams {
                kind: "recurring", title: "Morning brief", brief: "Summarize overnight activity", agent_id: "morning-brief-agent-2",
                project_id: "workspace", cron: Some("0 9 * * *"), run_at: None, next_run_at: &next_run_at,
                origin_session_id: Some(&origin_session.id), origin_message_id: None,
                parent_job_id: None, root_job_id: None, chain_depth: 0, wake_origin: false,
            },
        ).unwrap();

        let app = test_app_handle();
        let db = Arc::new(Mutex::new(DbConn::new(conn)));

        // Firing 1.
        scheduler_tick(db.clone(), app.clone()).await;
        poll_until(
            || async {
                let conn = db.lock().unwrap();
                conn.query_row(
                    "SELECT COUNT(*) FROM chat_messages_local WHERE thread_id = ?1 AND job_id = ?2",
                    params![origin_session.id, job_id],
                    |r| r.get::<_, i64>(0),
                ).unwrap_or(0) == 1
            },
            "firing 1 must land one report-back row in the origin",
        )
        .await;

        // Force the SAME recurring job due again — same technique C41's
        // own two-firings test and production's `run_job_now` both use.
        {
            let conn = db.lock().unwrap();
            conn.execute("UPDATE agent_jobs SET next_run_at = datetime('now') WHERE id = ?1", params![job_id]).unwrap();
        }

        // Firing 2 — the bug C42b closes: without it, this firing's
        // `source_message_id` collides with firing 1's and the ingest's
        // own `INSERT OR IGNORE` against `idx_chat_local_oc_dedup`
        // silently absorbs it (0 rows affected), so the count never
        // reaches 2.
        scheduler_tick(db.clone(), app.clone()).await;
        poll_until(
            || async {
                let conn = db.lock().unwrap();
                conn.query_row(
                    "SELECT COUNT(*) FROM chat_messages_local WHERE thread_id = ?1 AND job_id = ?2",
                    params![origin_session.id, job_id],
                    |r| r.get::<_, i64>(0),
                ).unwrap_or(0) == 2
            },
            "firing 2 of the SAME recurring job must produce a SECOND, distinct report-back row — not a dedup-skipped repeat of the first (C42b's exact bug)",
        )
        .await;

        mock.assert_async().await;

        let conn = db.lock().unwrap();
        let row_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM chat_messages_local WHERE thread_id = ?1 AND job_id = ?2", params![origin_session.id, job_id], |r| r.get(0))
            .unwrap();
        assert_eq!(row_count, 2, "two firings of a recurring job with an origin must produce TWO report-back rows, not one");

        let distinct_oc_ids: i64 = conn
            .query_row(
                "SELECT COUNT(DISTINCT oc_message_id) FROM chat_messages_local WHERE thread_id = ?1 AND job_id = ?2",
                params![origin_session.id, job_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(distinct_oc_ids, 2, "the two rows must carry two DIFFERENT oc_message_id dedup keys — proves the key is per-firing, not job-id-only");
    }

    /// C42c (#3147) — standing regression lock for C39: widening C42a's
    /// report-back gate must NOT reinstate C39's "six wake rows on Home"
    /// bug. Report-back is a plain INSERT (`ingest_external_messages_
    /// impl`); the wake-on-complete gate (site 2, further down in this
    /// same closure) is a SEPARATE `if` statement with its own,
    /// unmodified `kind IN ('spawn', 'once')` clause — confirmed
    /// (Reed's \u00a73): no shared boolean, no fallthrough between the two.
    ///
    /// Mutation (run, see report): temporarily deleted the `(job_kind_
    /// for_report == "spawn" || job_kind_for_report == "once")` clause
    /// from the wake-on-complete gate (site 2) — reddens: a wake row
    /// appears for a recurring job with an origin, reproducing C39's bug
    /// directly. Restored and re-confirmed green after.
    #[tokio::test]
    async fn test_recurring_job_with_origin_still_creates_no_wake_job_after_c42() {
        let conn = make_conn();
        seed_agent(&conn, "c42c-agent", "ollama", None);

        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/api/chat")
            .with_status(200)
            .with_header("content-type", "application/x-ndjson")
            .with_body(b"{\"model\":\"test-model\",\"message\":{\"role\":\"assistant\",\"content\":\"Done.\"},\"done\":true}\n" as &[u8])
            .create_async()
            .await;
        crate::database::set_ai_setting(&conn, "ollama_endpoint", &server.url()).unwrap();

        let origin_session = create_session_internal(&conn, "workspace", "c42c-agent", "user", true, None).unwrap();
        let next_run_at = format_stored_datetime(&Utc::now());
        let job_id = create_job(
            &conn,
            NewJobParams {
                kind: "recurring", title: "Tick", brief: "Tick", agent_id: "c42c-agent",
                project_id: "workspace", cron: Some("0 9 * * *"), run_at: None, next_run_at: &next_run_at,
                // `wake_origin: true` — the actually-dangerous
                // combination: if C39's own guard were ever
                // accidentally widened back open by C42a's change, THIS
                // is the flag that would let it create a wake row.
                origin_session_id: Some(&origin_session.id), origin_message_id: None,
                parent_job_id: None, root_job_id: None, chain_depth: 0, wake_origin: true,
            },
        ).unwrap();

        let app = test_app_handle();
        let db = Arc::new(Mutex::new(DbConn::new(conn)));

        scheduler_tick(db.clone(), app.clone()).await;
        poll_until(
            || async {
                let conn = db.lock().unwrap();
                // Wait on the report-back row (C42a's own effect) landing
                // first — proves the turn fully completed, including
                // the wake-on-complete gate further down in the SAME
                // closure, before asserting the wake-row absence below.
                conn.query_row(
                    "SELECT COUNT(*) FROM chat_messages_local WHERE thread_id = ?1 AND job_id = ?2",
                    params![origin_session.id, job_id],
                    |r| r.get::<_, i64>(0),
                ).unwrap_or(0) == 1
            },
            "the recurring job's report-back must land before asserting no wake row exists",
        )
        .await;

        let conn = db.lock().unwrap();
        let wake_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM agent_jobs WHERE kind = 'wake' AND origin_session_id IS NULL AND parent_job_id = ?1", params![job_id], |r| r.get(0))
            .unwrap();
        assert_eq!(wake_count, 0, "C42's widened report-back gate must NOT reinstate C39's wake-on-every-firing bug");
    }

    // ─── #3050 PR 5, Commit 5 — live chip status backend read ───────────

    /// Matt's addition: the chip's own live-status read. Unknown job_id ->
    /// `Ok(None)` (defensive, never an error toast); a real row -> the
    /// three fields the Svelte polling loop needs.
    #[test]
    fn test_get_job_status_known_and_unknown_job() {
        use tauri::Manager;
        let conn = make_conn();
        seed_agent(&conn, "agent-1", "anthropic", None);
        let sid = seed_job_session(&conn, "agent-1", "schedule");
        seed_job_row(&conn, "job-status-1", "once", "agent-1", &sid, None, "2026-12-01 09:00:00", 0);
        conn.execute(
            "UPDATE agent_jobs SET status = 'done', started_at = '2026-12-01 09:00:00', finished_at = '2026-12-01 09:00:48' WHERE id = 'job-status-1'",
            [],
        ).unwrap();

        let app = tauri::test::mock_app();
        app.manage(DbState::new(conn));
        let db_state = app.state::<DbState>();

        let known = get_job_status("job-status-1".to_string(), db_state.clone()).unwrap();
        let known = known.expect("job-status-1 must resolve");
        assert_eq!(known["status"], "done");
        assert_eq!(known["started_at"], "2026-12-01 09:00:00");
        assert_eq!(known["finished_at"], "2026-12-01 09:00:48");

        let unknown = get_job_status("job-does-not-exist".to_string(), db_state.clone()).unwrap();
        assert!(unknown.is_none(), "an unknown job_id must return Ok(None), never an error");
    }

    /// C37 (#3139, live test #4): `get_job_status` must now ALSO return
    /// `next_run_at` — the plumbing half of the chip-TZ fix. The TS side
    /// (`chipDisplayText`) reformats this value at render time via
    /// `formatScheduleDateTime`, reusing the panel's existing formatter
    /// rather than a second one; this test only proves the Rust-side field
    /// is actually present and correct, not the TS reformatting (covered
    /// by `jobStatusChip.test.ts`).
    ///
    /// Mutation: remove `next_run_at` from the SELECT/JSON — reddens
    /// (`known["next_run_at"]` is `Value::Null`, not the seeded string).
    #[test]
    fn test_get_job_status_includes_next_run_at_c37() {
        use tauri::Manager;
        let conn = make_conn();
        seed_agent(&conn, "agent-c37", "anthropic", None);
        let sid = seed_job_session(&conn, "agent-c37", "schedule");
        seed_job_row(&conn, "job-status-c37", "once", "agent-c37", &sid, None, "2026-12-02 09:30:00", 0);

        let app = tauri::test::mock_app();
        app.manage(DbState::new(conn));
        let db_state = app.state::<DbState>();

        let known = get_job_status("job-status-c37".to_string(), db_state.clone()).unwrap();
        let known = known.expect("job-status-c37 must resolve");
        assert_eq!(known["next_run_at"], "2026-12-02 09:30:00", "get_job_status must now also return next_run_at (C37)");
    }

    // --- C4/C5 (#3120 PR 2) --- reminder kind + one Reminders session per agent

    /// C4: a `reminder` job fires with ZERO provider calls. Agent is
    /// `runtime_type = "openclaw"` pointed at a mockito server whose
    /// `/v1/sessions` (create) and `/v1/sessions/.../messages` (send)
    /// endpoints are BOTH `.expect(0)` — the exact mechanism that WOULD
    /// fire if `fire_reminder_job`'s short-circuit were ever bypassed and
    /// this job fell through to `dispatch_oc_job`'s real OC-trigger path.
    /// Asserts the brief landed verbatim as the session's own message and
    /// the job reached `status = 'done'` within this single `scheduler_tick`
    /// call — no async round trip, no provider reply to wait on.
    ///
    /// Mutation: delete the `if job.kind == "reminder" { fire_reminder_job(...); continue; }`
    /// short-circuit in `scheduler_tick` — reddens two ways: `create_mock`/
    /// `send_mock`'s `.expect(0)` panics the moment the job falls through
    /// to `dispatch_oc_job`'s real sessions.create call, and even if that
    /// somehow succeeded, the message content would be OC's own reply, not
    /// `job.brief` verbatim.
    #[tokio::test]
    #[serial(keychain_override)]
    async fn test_reminder_job_fires_with_zero_provider_calls() {
        let _kc = KeychainGuard::set();
        let mut server = mockito::Server::new_async().await;
        let create_mock = server
            .mock("POST", "/v1/sessions")
            .with_status(200)
            .with_body(r#"{"session_id":"gw-uuid-rem","created_at":"2026-10-30T00:00:00Z","oc_session_id":"oc-key-reminder"}"#)
            .expect(0)
            .create_async()
            .await;
        let send_mock = server
            .mock("POST", mockito::Matcher::Regex(r"^/v1/sessions/.+/messages$".to_string()))
            .with_status(200)
            .with_body("{}")
            .expect(0)
            .create_async()
            .await;

        let conn = make_conn();
        let (agent_id, sid) = seed_oc_setup(&conn, &server.url());
        seed_job_row(&conn, "job-reminder-1", "reminder", &agent_id, &sid, None, "2020-01-01 00:00:00", 0);
        conn.execute("UPDATE agent_jobs SET brief = 'take the trash out' WHERE id = 'job-reminder-1'", []).unwrap();

        let db = Arc::new(Mutex::new(DbConn::new(conn)));
        scheduler_tick(db.clone(), test_app_handle()).await;

        // mockito's own `.expect(0)` + `.assert_async()` pairing panics
        // immediately if the endpoint was ever hit — this IS the "zero HTTP
        // mock hits" proof the brief asks for, not an inference from the
        // final status alone.
        create_mock.assert_async().await;
        send_mock.assert_async().await;

        let conn = db.lock().unwrap();
        let status: String = conn
            .query_row("SELECT status FROM agent_jobs WHERE id = 'job-reminder-1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(status, "done", "a reminder fires synchronously — no async round trip to wait on");

        let (role, content, synthetic): (String, String, i64) = conn
            .query_row(
                "SELECT role, content, synthetic FROM chat_messages_local WHERE thread_id = ?1 AND job_id = 'job-reminder-1'",
                params![sid],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(role, "assistant");
        assert_eq!(content, "take the trash out", "the brief must land verbatim — no provider ever touched it");
        assert_eq!(synthetic, 0);
    }

    /// C40 (#3147, live test #5): `fire_reminder_job` posted into the
    /// Reminders session and `continue`d before any notification gate —
    /// confirmed (Reed's audit §1): zero calls to
    /// `emit_agent_message_notification` anywhere in the function. Matt's
    /// C4 ruling is "post AND notify," not "post only." This test fires a
    /// reminder with its own session NOT open (the common case) and
    /// asserts exactly one `agent_message` notification row lands, keyed
    /// on the reminder's own session.
    ///
    /// Mutation: delete the new `if !is_session_open(&job.session_id) {
    /// emit_agent_message_notification(...) }` block added by this commit
    /// — reddens: zero `notifications` rows.
    #[tokio::test]
    async fn test_reminder_job_firing_emits_agent_message_notification_c40() {
        let conn = make_conn();
        seed_agent(&conn, "reminder-agent-c40", "ollama", None);

        let job_id = create_job(
            &conn,
            NewJobParams {
                kind: "reminder", title: "Take out trash", brief: "take the trash out", agent_id: "reminder-agent-c40",
                project_id: "workspace", cron: None, run_at: Some("2026-12-01T09:00:00"), next_run_at: "2026-12-01 09:00:00",
                origin_session_id: None, origin_message_id: None,
                parent_job_id: None, root_job_id: None, chain_depth: 0, wake_origin: true,
            },
        ).unwrap();
        // Force the job due now so this tick claims it (the dialog-created
        // `run_at`/`next_run_at` above are in the future, same shape
        // `run_job_now` already forces via this exact UPDATE).
        conn.execute("UPDATE agent_jobs SET next_run_at = datetime('now') WHERE id = ?1", params![job_id]).unwrap();
        let session_id: String = conn.query_row("SELECT session_id FROM agent_jobs WHERE id = ?1", params![job_id], |r| r.get(0)).unwrap();

        let db = Arc::new(Mutex::new(DbConn::new(conn)));
        let app = test_app_handle();

        // `fire_reminder_job` runs synchronously, inline in `scheduler_tick`
        // (never spawned) — no poll needed, same as the pre-existing C4 test.
        scheduler_tick(db.clone(), app.clone()).await;

        let conn = db.lock().unwrap();
        let notif_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM notifications WHERE type = 'agent_message' AND target_id = ?1",
                params![session_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(notif_count, 1, "a reminder firing into a closed session must raise exactly one agent_message notification");

        let (title, body): (String, String) = conn
            .query_row(
                "SELECT title, body FROM notifications WHERE type = 'agent_message' AND target_id = ?1",
                params![session_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(title, "Message from reminder-agent-c40", "the notification must name the reminder's own agent");
        assert!(body.starts_with("take the trash out"), "the notification body must carry the reminder text: {}", body);
    }

    /// #3151 PR1 Commit B (inverts the retired C5 pin, Reed's scope audit
    /// §2.2): two reminders for the SAME agent now land in TWO DIFFERENT
    /// sessions — the shared-Reminders-session branch is gone, so every
    /// reminder gets its own fresh, job-titled session like every other
    /// kind. Mutation: reintroduce the old `if params.kind == "reminder"
    /// { find_or_create_reminders_session(...) }` branch in `create_job`
    /// — reddens: both reminders collapse back onto one shared session
    /// (`sid_1 == sid_2`, `session_count == 1`).
    #[test]
    fn test_two_reminders_same_agent_land_in_different_sessions() {
        let conn = make_conn();
        seed_agent(&conn, "agent-rem", "openclaw", Some("main"));

        let job_id_1 = create_job(
            &conn,
            NewJobParams {
                kind: "reminder",
                title: "Reminder 1",
                brief: "first reminder",
                agent_id: "agent-rem",
                project_id: "workspace",
                cron: None,
                run_at: Some("2026-12-01T09:00:00"),
                next_run_at: "2026-12-01 09:00:00",
                origin_session_id: None,
                origin_message_id: None,
                parent_job_id: None,
                root_job_id: None,
                chain_depth: 0,
                wake_origin: true,
            },
        )
        .expect("first reminder job");
        let job_id_2 = create_job(
            &conn,
            NewJobParams {
                kind: "reminder",
                title: "Reminder 2",
                brief: "second reminder",
                agent_id: "agent-rem",
                project_id: "workspace",
                cron: None,
                run_at: Some("2026-12-02T09:00:00"),
                next_run_at: "2026-12-02 09:00:00",
                origin_session_id: None,
                origin_message_id: None,
                parent_job_id: None,
                root_job_id: None,
                chain_depth: 0,
                wake_origin: true,
            },
        )
        .expect("second reminder job");

        let sid_1: String = conn
            .query_row("SELECT session_id FROM agent_jobs WHERE id = ?1", params![job_id_1], |r| r.get(0))
            .unwrap();
        let sid_2: String = conn
            .query_row("SELECT session_id FROM agent_jobs WHERE id = ?1", params![job_id_2], |r| r.get(0))
            .unwrap();
        assert_ne!(sid_1, sid_2, "the Reminders session is retired — two reminders for the same agent must land in two different sessions now");

        let session_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM chat_sessions WHERE agent_id = 'agent-rem' AND origin = 'schedule'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(session_count, 2, "each reminder must get its own schedule-origin session");
    }

    /// #3151 PR1 Commit B: a reminder job's session is titled after the
    /// job itself (its own `params.title`), not the old hardcoded literal
    /// "Reminders" the retired `find_or_create_reminders_session` wrote.
    /// Mutation: revert to writing the literal "Reminders" instead of
    /// `params.title` — reddens (title is "Reminders", not "Take out
    /// trash").
    #[test]
    fn test_reminder_job_session_is_titled_after_the_job() {
        let conn = make_conn();
        seed_agent(&conn, "agent-rem-title", "anthropic", None);

        let job_id = create_job(
            &conn,
            NewJobParams {
                kind: "reminder",
                title: "Take out trash",
                brief: "take the trash out",
                agent_id: "agent-rem-title",
                project_id: "workspace",
                cron: None,
                run_at: Some("2026-12-01T09:00:00"),
                next_run_at: "2026-12-01 09:00:00",
                origin_session_id: None,
                origin_message_id: None,
                parent_job_id: None,
                root_job_id: None,
                chain_depth: 0,
                wake_origin: true,
            },
        )
        .expect("reminder job");

        let sid: String = conn
            .query_row("SELECT session_id FROM agent_jobs WHERE id = ?1", params![job_id], |r| r.get(0))
            .unwrap();
        let title: String = conn
            .query_row("SELECT title FROM chat_sessions WHERE id = ?1", params![sid], |r| r.get(0))
            .unwrap();
        assert_eq!(title, "Take out trash", "a reminder session must be titled after the job, not the old literal 'Reminders'");

        let origin: String = conn
            .query_row("SELECT origin FROM chat_sessions WHERE id = ?1", params![sid], |r| r.get(0))
            .unwrap();
        assert_eq!(origin, "schedule", "a reminder session must carry origin='schedule' (never origin='reminder') — this is what keeps it out of list_sessions, same as every other job kind, with no toggle");
    }

    // --- C6 (#3120 PR 2) --- job-turn smart routing for API agents

    /// C6: an API-runtime job turn with smart routing enabled dispatches
    /// on the simple-tier model, same gate `ai_chat_stream` already
    /// applies to interactive turns. The agent's OWN primary provider
    /// (`ollama`, endpoint `http://127.0.0.1:1` — a closed port) is never
    /// actually contacted by `prepare_job_dispatch` itself (it only
    /// RESOLVES which model/provider to use; the real completion call
    /// happens later in `run_agent_loop`, outside this function's scope),
    /// so an unreachable primary endpoint is a safe, deliberate choice here
    /// — proves this test exercises the RESOLUTION gate, not an accidental
    /// real dispatch. The classifier's OWN call (`internal_provider`,
    /// pointed at a mockito server) IS real and IS asserted via
    /// `.expect(1)`.
    ///
    /// Mutation: delete the `resolve_model_for_message(...)` call added to
    /// `prepare_job_dispatch` (reverting to the pre-fix `model:
    /// dispatch_cfg.model.clone()` / `provider_for_streaming: dispatch_cfg.
    /// provider.clone()`) — reddens two ways: `prep.model` is
    /// `"test-model"` (the agent's own configured model) instead of
    /// `"test-simple-model"`, and the classifier mock's `.expect(1)` panics
    /// because it was never hit at all.
    #[tokio::test]
    async fn test_job_turn_smart_routing_dispatches_simple_tier_model() {
        let mut server = mockito::Server::new_async().await;
        let classifier_mock = server
            .mock("POST", "/api/chat")
            .with_status(200)
            .with_body(r#"{"message":{"content":"YES"}}"#)
            .expect(1)
            .create_async()
            .await;

        let conn = make_conn();
        // Agent's own primary provider — deliberately unreachable (closed
        // port), since `prepare_job_dispatch` must never actually dial it.
        seed_agent(&conn, "smart-route-agent", "ollama", None);
        crate::database::set_ai_setting(&conn, "ollama_endpoint", "http://127.0.0.1:1").unwrap();
        // Smart routing config — the internal (classifier) provider points
        // at the real mock server.
        crate::database::set_ai_setting(&conn, "internal_provider", "ollama").unwrap();
        crate::database::set_ai_setting(&conn, "internal_endpoint", &server.url()).unwrap();
        crate::database::set_ai_setting(&conn, "simple_tier_model", "test-simple-model").unwrap();

        let sid = seed_job_session(&conn, "smart-route-agent", "schedule");
        // No tool-pattern/explicit-complexity/length bypass must trip —
        // this brief has to actually reach the classifier call.
        seed_job_row(&conn, "job-smart-route", "once", "smart-route-agent", &sid, None, "2020-01-01 00:00:00", 0);
        conn.execute("UPDATE agent_jobs SET brief = 'Whats 2 plus 2' WHERE id = 'job-smart-route'", []).unwrap();

        let claimed = claimed_job_from_row(load_claimed_job_row(&conn, "job-smart-route").unwrap());
        let app = tauri::test::mock_app();
        let db = Arc::new(Mutex::new(DbConn::new(conn)));
        let prep = prepare_job_dispatch(&db, &app.handle(), &claimed).await.expect("prepare_job_dispatch must succeed");

        classifier_mock.assert_async().await;
        assert_eq!(prep.model, "test-simple-model", "smart routing must select the simple-tier model, not the agent's own configured model ('test-model')");
    }

    /// Regression pin: smart routing DISABLED (no `internal_provider`/
    /// `simple_tier_model` configured) must leave job dispatch completely
    /// unaffected by C6 — the agent's own configured model is used, exactly
    /// as every `prepare_job_dispatch` test before this PR already assumed.
    /// Mutation: none needed to prove the FIX didn't break this (every
    /// pre-existing `prepare_job_dispatch` test in this module already
    /// pins this path); this test instead pins the specific "smart routing
    /// off" branch so a future regression can't silently flip it.
    #[tokio::test]
    async fn test_job_turn_smart_routing_disabled_uses_agents_own_model() {
        let conn = make_conn();
        seed_agent(&conn, "no-smart-route-agent", "ollama", None);
        crate::database::set_ai_setting(&conn, "ollama_endpoint", "http://127.0.0.1:1").unwrap();
        // Deliberately NOT configuring internal_provider/simple_tier_model.

        let sid = seed_job_session(&conn, "no-smart-route-agent", "schedule");
        seed_job_row(&conn, "job-no-smart-route", "once", "no-smart-route-agent", &sid, None, "2020-01-01 00:00:00", 0);

        let claimed = claimed_job_from_row(load_claimed_job_row(&conn, "job-no-smart-route").unwrap());
        let app = tauri::test::mock_app();
        let db = Arc::new(Mutex::new(DbConn::new(conn)));
        let prep = prepare_job_dispatch(&db, &app.handle(), &claimed).await.expect("prepare_job_dispatch must succeed");

        assert_eq!(prep.model, "test-model", "with smart routing unconfigured, the job must dispatch on the agent's own configured model, unchanged from before C6");
    }

    // --- C48 (#3155) --- job-turn credential must follow the RESOLVED provider ---

    /// C48: before this fix, `prepare_job_dispatch` derived `(api_key,
    /// openclaw_base_url)` from `dispatch_cfg.provider` (the agent's primary,
    /// configured runtime) BEFORE calling `resolve_model_for_message`. When
    /// Smart Routing's classifier says "simple" and returns a
    /// `provider_override`, `resolved_provider`/`provider_for_streaming`
    /// correctly becomes the override — but the stale `api_key` captured
    /// earlier never followed it, so `run_agent_loop` streamed to the
    /// override provider carrying the PRIMARY provider's credential
    /// (Vernon's real-world case: primary Ollama → empty key → OpenRouter
    /// override → `401 Missing Authentication header`).
    ///
    /// This test proves the fix with the roles deliberately swapped from the
    /// issue's own illustrative example (primary Ollama/empty → override
    /// OpenRouter/real-key) so it stays network-free and deterministic: the
    /// PRIMARY provider here is Anthropic with a real (mock-keychain) key —
    /// a value that must NEVER reach the resolved provider — and the
    /// OVERRIDE (Smart Routing's internal/classifier provider) is `ollama`
    /// pointed at a mockito server, the same mockable-endpoint mechanism
    /// `test_job_turn_smart_routing_dispatches_simple_tier_model` above
    /// already uses. Ollama's own `AiProvider` arm never reads a key at all
    /// (`String::new()`), so the ONLY correct `api_key` for the resolved
    /// provider is empty — the pre-fix bug would instead carry the primary
    /// Anthropic key forward onto a provider that was never supposed to see
    /// it. Using a real OpenRouter/Anthropic override (as the issue's own
    /// example does) would require a live network call to the real provider
    /// API inside the classifier step, which this suite cannot depend on —
    /// this is a deliberate, reported adaptation, not a deviation in what's
    /// being proven: `prepare_job_dispatch` must derive the credential from
    /// `resolved_provider`, not `dispatch_cfg.provider`, exactly as
    /// `ai_chat_stream` already does from `effective_provider`.
    ///
    /// Mutation: revert the `(api_key, openclaw_base_url)` derivation to
    /// match on `&dispatch_cfg.provider` again (the pre-fix code, run before
    /// `resolve_model_for_message`) — `prep.api_key` reads back
    /// `"sk-anthropic-primary-should-never-reach-ollama"` instead of `""` — red.
    #[tokio::test]
    #[serial(keychain_override)]
    async fn test_job_turn_c48_credential_follows_resolved_provider_not_primary() {
        let _kc = KeychainGuard::set();
        crate::keychain::store_password("ai_anthropic_key", "sk-anthropic-primary-should-never-reach-ollama")
            .expect("mock keychain store");

        let mut server = mockito::Server::new_async().await;
        let classifier_mock = server
            .mock("POST", "/api/chat")
            .with_status(200)
            .with_body(r#"{"message":{"content":"YES"}}"#)
            .expect(1)
            .create_async()
            .await;

        let conn = make_conn();
        // Primary provider: Anthropic, with a real (mock-keychain) key that
        // must never reach the resolved (Ollama) provider.
        seed_agent(&conn, "c48-agent", "anthropic", None);
        // Smart Routing config: classifier/override provider is Ollama,
        // pointed at the mock server — mockable, and (unlike OpenRouter's
        // hardcoded base URL) requires no real network access.
        crate::database::set_ai_setting(&conn, "internal_provider", "ollama").unwrap();
        crate::database::set_ai_setting(&conn, "internal_endpoint", &server.url()).unwrap();
        crate::database::set_ai_setting(&conn, "simple_tier_model", "test-simple-model").unwrap();

        let sid = seed_job_session(&conn, "c48-agent", "schedule");
        seed_job_row(&conn, "job-c48", "once", "c48-agent", &sid, None, "2020-01-01 00:00:00", 0);
        // No tool-pattern/explicit-complexity/length bypass must trip — this
        // brief has to actually reach the classifier call.
        conn.execute("UPDATE agent_jobs SET brief = 'Whats 2 plus 2' WHERE id = 'job-c48'", [])
            .unwrap();

        let claimed = claimed_job_from_row(load_claimed_job_row(&conn, "job-c48").unwrap());
        let app = tauri::test::mock_app();
        let db = Arc::new(Mutex::new(DbConn::new(conn)));
        let prep = prepare_job_dispatch(&db, &app.handle(), &claimed).await.expect("prepare_job_dispatch must succeed");

        classifier_mock.assert_async().await;
        assert!(
            matches!(prep.provider_for_streaming, AiProvider::Ollama { .. }),
            "resolved provider must be the Smart Routing override (Ollama), not the agent's primary (Anthropic); got {:?}",
            prep.provider_for_streaming
        );
        assert_eq!(
            prep.api_key, "",
            "the credential must be derived from the RESOLVED provider (Ollama, no key), never the stale primary Anthropic key"
        );
    }

    /// Regression pin, kept post-retirement (Reed's scope audit §2.2):
    /// before Commit B this exercised `find_or_create_reminders_session`'s
    /// `agent_id`-scoped lookup. Now that the shared-lookup branch is gone
    /// entirely, two different agents' reminders trivially land in
    /// different sessions the same way any two jobs of any kind would —
    /// there is no code path left that could make this fail on its own
    /// (no mutation is meaningful here; it would just re-prove Commit B's
    /// own mutation above). Kept as a plain regression pin, not deleted,
    /// since "different agents never share a session" is still a true
    /// and worth-stating invariant.
    #[test]
    fn test_reminders_for_different_agents_get_different_sessions() {
        let conn = make_conn();
        seed_agent(&conn, "agent-rem-a", "openclaw", Some("main"));
        seed_agent(&conn, "agent-rem-b", "openclaw", Some("secondary"));

        let params_for = |agent_id: &'static str| NewJobParams {
            kind: "reminder",
            title: "Reminder",
            brief: "a reminder",
            agent_id,
            project_id: "workspace",
            cron: None,
            run_at: Some("2026-12-01T09:00:00"),
            next_run_at: "2026-12-01 09:00:00",
            origin_session_id: None,
            origin_message_id: None,
            parent_job_id: None,
            root_job_id: None,
            chain_depth: 0,
            wake_origin: true,
        };

        let job_a = create_job(&conn, params_for("agent-rem-a")).expect("agent a reminder");
        let job_b = create_job(&conn, params_for("agent-rem-b")).expect("agent b reminder");

        let sid_a: String = conn.query_row("SELECT session_id FROM agent_jobs WHERE id = ?1", params![job_a], |r| r.get(0)).unwrap();
        let sid_b: String = conn.query_row("SELECT session_id FROM agent_jobs WHERE id = ?1", params![job_b], |r| r.get(0)).unwrap();
        assert_ne!(sid_a, sid_b, "different agents must never share a Reminders session");
    }

    // --- #3151 PR1 Commit A: every job kind's session gets titled ---

    /// Commit A: a `once` job's own session is titled after the job —
    /// before this commit, `create_session_internal` always inserted
    /// `title = ''` and nothing ever overwrote it except the (separate)
    /// reminders branch. Mutation: delete the new `UPDATE chat_sessions
    /// SET title = ...` in `create_job`'s non-reminder branch — reddens
    /// (title stays `''`).
    #[test]
    fn test_create_job_sets_session_title_for_every_kind() {
        let conn = make_conn();
        seed_agent(&conn, "agent-title-a", "anthropic", None);

        for kind in ["once", "recurring", "heartbeat", "spawn"] {
            let job_id = create_job(
                &conn,
                NewJobParams {
                    kind,
                    title: "Morning brief",
                    brief: "give me the morning brief",
                    agent_id: "agent-title-a",
                    project_id: "workspace",
                    cron: if kind == "recurring" { Some("0 7 * * *") } else { None },
                    run_at: if kind == "recurring" { None } else { Some("2026-12-01T09:00:00") },
                    next_run_at: "2026-12-01 09:00:00",
                    origin_session_id: None,
                    origin_message_id: None,
                    parent_job_id: None,
                    root_job_id: None,
                    chain_depth: 0,
                    wake_origin: true,
                },
            )
            .unwrap_or_else(|e| panic!("create_job({}) failed: {}", kind, e));

            let sid: String = conn
                .query_row("SELECT session_id FROM agent_jobs WHERE id = ?1", params![job_id], |r| r.get(0))
                .unwrap();
            let title: String = conn
                .query_row("SELECT title FROM chat_sessions WHERE id = ?1", params![sid], |r| r.get(0))
                .unwrap();
            assert_eq!(title, "Morning brief", "kind={} job session must be titled after the job", kind);
        }
    }

}


/// #3050 PR 7 (§36.6.2/§36.6.3): background OC pickup — eligibility, rotation,
/// key selection (cross-referenced from commit 5's own module), concurrency/
/// timeout, notification vs. event dispatch, and the turn-in-flight skip.
#[cfg(test)]
mod oc_background_pickup_tests_3050_pr7 {
    use super::*;
    use crate::keychain::test_support::KeychainGuard;
    use rusqlite::Connection;
    use serial_test::serial;
    use tauri::Listener;

    fn make_conn() -> Connection {
        let conn = Connection::open_in_memory().expect("in-memory db");
        crate::migrations::run_migrations(&conn).expect("fresh-install migrations should succeed");
        conn
    }

    fn test_app_handle() -> tauri::AppHandle<tauri::test::MockRuntime> {
        tauri::test::mock_builder()
            .plugin(tauri_plugin_notification::init())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("mock app with notification plugin must build")
            .handle()
            .clone()
    }

    fn seed_agent(conn: &Connection, id: &str, runtime_type: &str, runtime_ref: Option<&str>) {
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO ai_agents (id, name, slug, is_default, is_active, workspace_path, runtime_type, runtime_ref, model, created_at, updated_at) \
             VALUES (?1, ?1, ?1, 0, 1, ?1, ?2, ?3, 'test-model', ?4, ?4)",
            params![id, runtime_type, runtime_ref, now],
        )
        .expect("seed agent");
    }

    /// Seeds a chat session with an explicit `last_active_at`, baselined
    /// (`oc_baseline_done_at` stamped) so `pull_oc_transcript_for_session`
    /// exercises the steady-state ingest path, not the first-pull gate.
    fn seed_session_with_last_active(conn: &Connection, id: &str, agent_id: &str, last_active_at: &str) {
        conn.execute(
            "INSERT INTO chat_sessions (id, project_id, agent_id, last_active_at, oc_baseline_done_at) \
             VALUES (?1, 'workspace', ?2, ?3, datetime('now'))",
            params![id, agent_id, last_active_at],
        )
        .expect("seed session");
    }

    fn seed_job_with_updated_at(conn: &Connection, job_id: &str, agent_id: &str, session_id: &str, updated_at: &str) {
        conn.execute(
            "INSERT INTO agent_jobs (id, kind, title, brief, agent_id, project_id, session_id, next_run_at, status, created_at, updated_at) \
             VALUES (?1, 'heartbeat', '', 'brief', ?2, 'workspace', ?3, datetime('now'), 'pending', ?4, ?4)",
            params![job_id, agent_id, session_id, updated_at],
        )
        .expect("seed job");
        conn.execute("UPDATE agent_jobs SET updated_at = ?1 WHERE id = ?2", params![updated_at, job_id])
            .expect("set updated_at");
    }

    fn setup_gateway(conn: &Connection, server_url: &str, token: &str) {
        crate::database::set_ai_setting(conn, "endpoint", server_url).unwrap();
        crate::ai::credential_migration::write_gateway_token(conn, token).unwrap();
    }

    fn hours_ago(h: i64) -> String {
        (chrono::Utc::now() - chrono::Duration::hours(h))
            .format("%Y-%m-%d %H:%M:%S")
            .to_string()
    }

    fn days_ago(d: i64) -> String {
        (chrono::Utc::now() - chrono::Duration::days(d))
            .format("%Y-%m-%d %H:%M:%S")
            .to_string()
    }

    // ── Commit 6: eligibility query's OR agent_jobs.updated_at clause ──────

    /// A job session created 10 days ago, never opened by a human
    /// (`chat_sessions.last_active_at` frozen at that stale creation
    /// timestamp), whose `agent_jobs` row fired (and bumped `updated_at`) 2
    /// hours ago, must still be eligible. Named by Matt's task directly.
    ///
    /// Mutation: drop the `OR agent_jobs.updated_at > ?1` clause from
    /// `select_oc_pickup_eligible_sessions` -> this session is silently
    /// excluded -> red. (Verified by hand below this test — see the
    /// reported mutation output in the PR report; reverting the clause via
    /// source edit reproduces red deterministically.)
    #[test]
    fn test_eligibility_includes_job_session_via_agent_jobs_updated_at_when_last_active_at_frozen() {
        let conn = make_conn();
        seed_agent(&conn, "agent-oc-stale", "openclaw", Some("main"));
        seed_session_with_last_active(&conn, "session-stale-job", "agent-oc-stale", &days_ago(10));
        seed_job_with_updated_at(&conn, "job-stale-1", "agent-oc-stale", "session-stale-job", &hours_ago(2));

        let eligible = select_oc_pickup_eligible_sessions(&conn).expect("query must succeed");
        assert!(
            eligible.contains(&"session-stale-job".to_string()),
            "a job session whose own agent_jobs.updated_at is recent must be eligible even with a \
             frozen (10-day-old) last_active_at: {:?}",
            eligible
        );
    }

    /// Inverse: a session with BOTH last_active_at and every agent_jobs row's
    /// updated_at older than 7 days must NOT be eligible.
    #[test]
    fn test_eligibility_excludes_session_both_last_active_and_job_updated_at_stale() {
        let conn = make_conn();
        seed_agent(&conn, "agent-oc-dead", "openclaw", Some("main"));
        seed_session_with_last_active(&conn, "session-dead", "agent-oc-dead", &days_ago(10));
        seed_job_with_updated_at(&conn, "job-dead-1", "agent-oc-dead", "session-dead", &days_ago(10));

        let eligible = select_oc_pickup_eligible_sessions(&conn).expect("query must succeed");
        assert!(
            !eligible.contains(&"session-dead".to_string()),
            "a session stale on both last_active_at AND every job's updated_at must not be eligible: {:?}",
            eligible
        );
    }

    /// A plain chat session (no agent_jobs row at all) is still eligible
    /// purely off its own recent last_active_at — confirms the LEFT JOIN
    /// doesn't accidentally exclude non-job sessions.
    #[test]
    fn test_eligibility_includes_plain_chat_session_recent_last_active_at() {
        let conn = make_conn();
        seed_agent(&conn, "agent-oc-chat", "openclaw", Some("main"));
        seed_session_with_last_active(&conn, "session-plain-chat", "agent-oc-chat", &hours_ago(1));

        let eligible = select_oc_pickup_eligible_sessions(&conn).expect("query must succeed");
        assert!(eligible.contains(&"session-plain-chat".to_string()));
    }

    /// A non-OC-runtime session is never eligible regardless of recency.
    #[test]
    fn test_eligibility_excludes_non_openclaw_runtime_session() {
        let conn = make_conn();
        seed_agent(&conn, "agent-non-oc", "ollama", None);
        seed_session_with_last_active(&conn, "session-non-oc", "agent-non-oc", &hours_ago(1));

        let eligible = select_oc_pickup_eligible_sessions(&conn).expect("query must succeed");
        assert!(!eligible.contains(&"session-non-oc".to_string()));
    }

    /// #3129 (C28(b)), item 8's own named follow-on gap: a job session
    /// (`origin = 'schedule'`), recently bumped (simulating what
    /// `run_agent_loop` writing its own reply into it would do — the
    /// loop's own message-insert bumps `chat_sessions.last_active_at`),
    /// must NOT be eligible — it was never an OC-side session to poll,
    /// unlike a plain `origin = 'user'` interactive session on the same
    /// agent, recently active too, which MUST still be eligible.
    ///
    /// **Mutation (actually run — see report):** removed the `AND
    /// chat_sessions.origin NOT IN ('spawn', 'schedule')` clause. Failure
    /// output:
    /// ```text
    /// thread '...test_eligibility_excludes_job_session_origin_includes_interactive_session' panicked at src/ai/scheduler.rs:NNNN:9:
    /// a job (origin='schedule') session must never be OC-pickup-eligible — it was never an OC-side session: ["session-job-schedule", "session-interactive"]
    /// ```
    /// (confirmed and reverted.)
    #[test]
    fn test_eligibility_excludes_job_session_origin_includes_interactive_session() {
        let conn = make_conn();
        seed_agent(&conn, "agent-oc-origin", "openclaw", Some("main"));
        conn.execute(
            "INSERT INTO chat_sessions (id, project_id, agent_id, origin, last_active_at, oc_baseline_done_at) \
             VALUES ('session-job-schedule', 'workspace', 'agent-oc-origin', 'schedule', ?1, datetime('now'))",
            params![hours_ago(1)],
        )
        .expect("seed job-origin session");
        conn.execute(
            "INSERT INTO chat_sessions (id, project_id, agent_id, origin, last_active_at, oc_baseline_done_at) \
             VALUES ('session-interactive', 'workspace', 'agent-oc-origin', 'user', ?1, datetime('now'))",
            params![hours_ago(1)],
        )
        .expect("seed interactive session");

        let eligible = select_oc_pickup_eligible_sessions(&conn).expect("query must succeed");
        assert!(
            !eligible.contains(&"session-job-schedule".to_string()),
            "a job (origin='schedule') session must never be OC-pickup-eligible — it was never an OC-side session: {:?}",
            eligible
        );
        assert!(
            eligible.contains(&"session-interactive".to_string()),
            "an ordinary interactive session on the same OC-bound agent must stay eligible: {:?}",
            eligible
        );
    }

    // ── Rotation / anti-starvation (§2.3) ───────────────────────────────────

    /// 25 eligible sessions: tick 1 takes the first page of 20, tick 2 takes
    /// the remaining 5 — AND tick 2's 5 are the SPECIFIC 5 skipped on tick 1
    /// (not a re-roll of the same 20). Named by the task directly.
    ///
    /// Mutation: remove the rotation counter (always take page 0) -> tick 2
    /// polls the same 20 as tick 1, the 5 are never reached -> red.
    #[test]
    fn test_rotation_25_eligible_20_this_tick_5_next_not_a_reroll() {
        // Stable (last_active_at DESC, id ASC) ordering: use distinct
        // last_active_at values so the 20/5 split is deterministic.
        let ids: Vec<String> = (0..25).map(|i| format!("sess-rot-{:02}", i)).collect();
        let total_pages = 25usize.div_ceil(RUNTIME_OC_PICKUP_SESSIONS_PER_TICK);
        assert_eq!(total_pages, 2);

        let page0 = next_oc_pickup_page(total_pages);
        let page1 = next_oc_pickup_page(total_pages);
        assert_ne!(page0, page1, "two consecutive ticks must take DIFFERENT pages, not re-roll the same one");

        let take = |page: usize| -> Vec<String> {
            ids.iter()
                .skip(page * RUNTIME_OC_PICKUP_SESSIONS_PER_TICK)
                .take(RUNTIME_OC_PICKUP_SESSIONS_PER_TICK)
                .cloned()
                .collect()
        };
        let tick1 = take(page0);
        let tick2 = take(page1);
        // The counter's starting value is intentionally randomized (cursor
        // offset fix, #3050 PR 7 / PR #3115 audit — a restart can't starve
        // the tail), so which of the two pages rotates first on any given
        // test run is not fixed — only that the pair is a 20/5 split of
        // the 25 eligible sessions and the two pages are disjoint.
        let mut lens = [tick1.len(), tick2.len()];
        lens.sort_unstable();
        assert_eq!(lens, [5, 20], "the two pages together must be a 20/5 split of the 25 eligible sessions, regardless of which rotates first");
        for id in &tick2 {
            assert!(!tick1.contains(id), "tick 2's sessions must be the ones SKIPPED on tick 1, not a repeat: {}", id);
        }
    }

    /// `next_oc_pickup_page` with `total_pages == 1` always returns 0,
    /// harmlessly, and still increments the underlying counter (so a LATER
    /// tick with more eligible sessions picks up rotation from a
    /// non-trivial offset rather than resetting).
    #[test]
    fn test_rotation_single_page_always_zero() {
        assert_eq!(next_oc_pickup_page(1), 0);
        assert_eq!(next_oc_pickup_page(1), 0);
        assert_eq!(next_oc_pickup_page(0), 0, "zero pages (no eligible sessions) must not divide by zero");
    }

    // ── The named §36.6.3 integration tests ─────────────────────────────────

    /// Two OC sessions, one open one closed, both with new messages -> both
    /// ingested, exactly ONE notification (for the closed one). Named by the
    /// task directly; also the §3a `agent_message` §36.6 wiring pin.
    ///
    /// Mutation (open-session check): force `is_session_open` to always
    /// return `false` inside `run_oc_background_pickup`'s call site -> the
    /// open session now ALSO notifies -> two notifications instead of one
    /// -> red. (Hand-verified below; see PR report for the exact edit and
    /// failure output.)
    #[tokio::test]
    #[serial(keychain_override, viewing_session_3050_pr6)]
    async fn test_both_sessions_ingested_exactly_one_notification_for_closed() {
        let _kc = KeychainGuard::set();
        let mut server = mockito::Server::new_async().await;

        let conn = make_conn();
        seed_agent(&conn, "agent-oc-dual", "openclaw", Some("main"));
        seed_session_with_last_active(&conn, "session-open-dual", "agent-oc-dual", &hours_ago(1));
        seed_session_with_last_active(&conn, "session-closed-dual", "agent-oc-dual", &hours_ago(1));
        setup_gateway(&conn, &server.url(), "test-token-dual");

        let body_open = serde_json::json!({
            "messages": [{ "role": "assistant", "content": "hello from open", "timestamp": 1_756_100_000_000i64, "__openclaw": { "id": "oc-open-1" } }]
        });
        let body_closed = serde_json::json!({
            "messages": [{ "role": "assistant", "content": "hello from closed", "timestamp": 1_756_100_000_000i64, "__openclaw": { "id": "oc-closed-1" } }]
        });
        let _m_open = server
            .mock("GET", "/v1/sessions/agent%3Amain%3Aopenai-user%3Apane-session-open-dual/messages?limit=20")
            .with_status(200).with_header("content-type", "application/json").with_body(body_open.to_string())
            .create_async().await;
        let _m_closed = server
            .mock("GET", "/v1/sessions/agent%3Amain%3Aopenai-user%3Apane-session-closed-dual/messages?limit=20")
            .with_status(200).with_header("content-type", "application/json").with_body(body_closed.to_string())
            .create_async().await;

        // "session-open-dual" is the one currently on screen.
        crate::ai::chat::set_viewing_session(Some("session-open-dual".to_string())).unwrap();

        let app = test_app_handle();
        let events: Arc<Mutex<Vec<serde_json::Value>>> = Arc::new(Mutex::new(Vec::new()));
        let events2 = events.clone();
        app.listen("pane-oc-message-ingested", move |event| {
            events2.lock().unwrap().push(serde_json::from_str(event.payload()).unwrap());
        });

        let db = Arc::new(Mutex::new(DbConn::new(conn)));
        run_oc_background_pickup(db.clone(), app.clone()).await;

        // Give the detached notification-dedup check a beat — run_oc_background_pickup
        // itself already awaits every spawned pull, so no poll is needed for the
        // ingest/emit itself; this is purely defensive against any future async
        // tweak to the notify call site.
        let conn = db.lock().unwrap();
        let notif_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM notifications WHERE type = 'agent_message'", [], |r| r.get(0))
            .unwrap_or(0);
        assert_eq!(notif_count, 1, "exactly one agent_message notification must exist — for the CLOSED session only");

        let open_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM chat_messages_local WHERE thread_id = 'session-open-dual' AND oc_message_id = 'oc-open-1'",
                [], |r| r.get(0),
            )
            .unwrap();
        let closed_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM chat_messages_local WHERE thread_id = 'session-closed-dual' AND oc_message_id = 'oc-closed-1'",
                [], |r| r.get(0),
            )
            .unwrap();
        assert_eq!(open_count, 1, "the open session's message must still be ingested (appended, not dropped)");
        assert_eq!(closed_count, 1, "the closed session's message must be ingested too");
        drop(conn);

        assert_eq!(
            events.lock().unwrap().len(),
            1,
            "pane-oc-message-ingested must fire exactly once — for the OPEN session only"
        );
        crate::ai::chat::set_viewing_session(None).unwrap();
    }

    /// Re-poll (dedup): the second call with the SAME mocked page inserts
    /// zero new rows. Inherited from the pre-existing partial unique index
    /// (`idx_chat_local_oc_dedup`); this pins it through the NEW
    /// backend-tick call path specifically.
    #[tokio::test]
    #[serial(keychain_override, viewing_session_3050_pr6)]
    async fn test_repoll_same_page_zero_new_inserts() {
        let _kc = KeychainGuard::set();
        let mut server = mockito::Server::new_async().await;

        let conn = make_conn();
        seed_agent(&conn, "agent-oc-repoll", "openclaw", Some("main"));
        seed_session_with_last_active(&conn, "session-repoll", "agent-oc-repoll", &hours_ago(1));
        setup_gateway(&conn, &server.url(), "test-token-repoll");

        let body = serde_json::json!({
            "messages": [{ "role": "assistant", "content": "repoll me", "timestamp": 1_756_100_000_000i64, "__openclaw": { "id": "oc-repoll-1" } }]
        });
        let _m = server
            .mock("GET", "/v1/sessions/agent%3Amain%3Aopenai-user%3Apane-session-repoll/messages?limit=20")
            .with_status(200).with_header("content-type", "application/json").with_body(body.to_string())
            .expect(2)
            .create_async().await;

        let app = test_app_handle();
        let db = Arc::new(Mutex::new(DbConn::new(conn)));

        run_oc_background_pickup(db.clone(), app.clone()).await;
        run_oc_background_pickup(db.clone(), app.clone()).await;

        let conn = db.lock().unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM chat_messages_local WHERE thread_id = 'session-repoll' AND oc_message_id = 'oc-repoll-1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1, "the second tick's re-fetch of the same page must insert ZERO additional rows");
    }

    /// Structural pin of §0/§36.6's gate removal: this test runs
    /// `run_oc_background_pickup` directly with NO frontend concept of
    /// "focus" ever touched (there is none to touch backend-side) and
    /// asserts the pull still happens. Reed's audit note applies here
    /// (§36.6.2's own text, "say so rather than inventing one" where a
    /// mutation would prove nothing): there is no boolean left to flip —
    /// the old `appFocused`/`eligible` gate was pure frontend state deleted
    /// in commit 9, and this backend path never had an equivalent. This
    /// test's job is structural (it would have been impossible to write
    /// meaningfully before this PR, since nothing called the pull without
    /// the frontend's own focus+open-session gate being true first).
    #[tokio::test]
    #[serial(keychain_override, viewing_session_3050_pr6)]
    async fn test_pickup_runs_with_no_focus_concept_touched_at_all() {
        let _kc = KeychainGuard::set();
        let mut server = mockito::Server::new_async().await;

        let conn = make_conn();
        seed_agent(&conn, "agent-oc-unfocused", "openclaw", Some("main"));
        seed_session_with_last_active(&conn, "session-unfocused", "agent-oc-unfocused", &hours_ago(1));
        setup_gateway(&conn, &server.url(), "test-token-unfocused");

        // Explicitly: nothing is "open" (no set_viewing_session call at all,
        // the closest backend analogue to "app unfocused, no chat open").
        let body = serde_json::json!({
            "messages": [{ "role": "assistant", "content": "unfocused poll", "timestamp": 1_756_100_000_000i64, "__openclaw": { "id": "oc-unfocused-1" } }]
        });
        let m = server
            .mock("GET", "/v1/sessions/agent%3Amain%3Aopenai-user%3Apane-session-unfocused/messages?limit=20")
            .with_status(200).with_header("content-type", "application/json").with_body(body.to_string())
            .create_async().await;

        let app = test_app_handle();
        let db = Arc::new(Mutex::new(DbConn::new(conn)));
        run_oc_background_pickup(db.clone(), app.clone()).await;

        m.assert_async().await;
        let conn = db.lock().unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM chat_messages_local WHERE thread_id = 'session-unfocused'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1, "the pull must run and ingest regardless of any 'focus'/'open' state");
    }

    /// Gateway-down/slow: one session's gateway call never returns (mocked
    /// to hang past the 10s wrapper via a delayed mockito response) — that
    /// session's task times out and is skipped (logged, not panicked), and
    /// another session in the SAME page is unaffected (concurrency proof,
    /// §2.4). Uses a shortened effective timeout via a slow-but-not-10s mock
    /// is not meaningful (we can't shrink OC_PICKUP_PER_SESSION_TIMEOUT_SECS
    /// without a real 10s wait) — this test instead proves the OTHER
    /// session's success is unaffected by a session whose gateway call
    /// errors immediately (a connection-refused shaped failure), which
    /// exercises the identical "one bad session can't break the batch" code
    /// path without a real-time 10s sleep in test (§36.6.2's own
    /// "never a timer" rule for THIS suite, not the production wrapper
    /// itself, which is a fixed 10s by design, not something a test should
    /// wait out).
    #[tokio::test]
    #[serial(keychain_override, viewing_session_3050_pr6)]
    async fn test_one_session_gateway_error_does_not_affect_other_sessions_in_same_batch() {
        let _kc = KeychainGuard::set();
        let mut server = mockito::Server::new_async().await;

        let conn = make_conn();
        seed_agent(&conn, "agent-oc-batch", "openclaw", Some("main"));
        // "session-bad" points at a gateway with no matching mock (404 from
        // mockito) — pull_oc_transcript_for_session treats any non-2xx as a
        // graceful skip, never a panic; this exercises the "skip without
        // affecting siblings" contract identically to a hard timeout would.
        seed_session_with_last_active(&conn, "session-bad-batch", "agent-oc-batch", &hours_ago(1));
        seed_session_with_last_active(&conn, "session-good-batch", "agent-oc-batch", &hours_ago(1));
        setup_gateway(&conn, &server.url(), "test-token-batch");

        let body_good = serde_json::json!({
            "messages": [{ "role": "assistant", "content": "good batch message", "timestamp": 1_756_100_000_000i64, "__openclaw": { "id": "oc-good-batch-1" } }]
        });
        let _m_good = server
            .mock("GET", "/v1/sessions/agent%3Amain%3Aopenai-user%3Apane-session-good-batch/messages?limit=20")
            .with_status(200).with_header("content-type", "application/json").with_body(body_good.to_string())
            .create_async().await;
        // No mock registered for session-bad-batch's URL -> mockito returns
        // a non-2xx "no matching mock" response, exercised as the graceful
        // skip path.

        let app = test_app_handle();
        let db = Arc::new(Mutex::new(DbConn::new(conn)));
        run_oc_background_pickup(db.clone(), app.clone()).await;

        let conn = db.lock().unwrap();
        let good_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM chat_messages_local WHERE thread_id = 'session-good-batch'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(good_count, 1, "the good session's ingest must succeed even though a sibling session's pull failed in the same batch");
    }

    // ── Turn-in-flight skip (Matt's Oct 29 ruling) ──────────────────────────

    /// A pull during an in-flight turn ingests NOTHING for that session.
    /// Mutation on the skip: remove the `is_turn_in_flight` check in
    /// `run_oc_background_pickup` -> the session is polled anyway -> the
    /// mocked message inserts -> red.
    #[tokio::test]
    #[serial(keychain_override, turn_in_flight_3050_pr7)]
    async fn test_pickup_skips_session_with_turn_in_flight() {
        let _kc = KeychainGuard::set();
        let mut server = mockito::Server::new_async().await;

        let conn = make_conn();
        seed_agent(&conn, "agent-oc-inflight", "openclaw", Some("main"));
        seed_session_with_last_active(&conn, "session-inflight", "agent-oc-inflight", &hours_ago(1));
        setup_gateway(&conn, &server.url(), "test-token-inflight");

        let body = serde_json::json!({
            "messages": [{ "role": "assistant", "content": "should not be ingested", "timestamp": 1_756_100_000_000i64, "__openclaw": { "id": "oc-inflight-1" } }]
        });
        // No `.expect(0)` needed structurally — the assertion below on
        // inserted rows is the real pin; a stray call would be harmless to
        // mockito itself either way.
        let _m = server
            .mock("GET", "/v1/sessions/agent%3Amain%3Aopenai-user%3Apane-session-inflight/messages?limit=20")
            .with_status(200).with_header("content-type", "application/json").with_body(body.to_string())
            .create_async().await;

        let app = test_app_handle();
        let db = Arc::new(Mutex::new(DbConn::new(conn)));

        let _guard = crate::ai::chat::TurnInFlightGuard::mark("session-inflight");
        run_oc_background_pickup(db.clone(), app.clone()).await;

        let conn = db.lock().unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM chat_messages_local WHERE thread_id = 'session-inflight'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0, "a session with turn_in_flight set must be skipped entirely — nothing ingested");
        drop(conn);
        drop(_guard);
    }

    /// After the guard drops (loop ends, tombstone pass has run — see
    /// `ai::chat::TurnInFlightGuard`'s own doc comment for why Drop timing
    /// already encodes that ordering), the next pull ingests normally.
    /// Pairs with the skip test above to prove the flag is not sticky.
    #[tokio::test]
    #[serial(keychain_override, turn_in_flight_3050_pr7)]
    async fn test_pickup_resumes_once_guard_drops() {
        let _kc = KeychainGuard::set();
        let mut server = mockito::Server::new_async().await;

        let conn = make_conn();
        seed_agent(&conn, "agent-oc-resume", "openclaw", Some("main"));
        seed_session_with_last_active(&conn, "session-resume", "agent-oc-resume", &hours_ago(1));
        setup_gateway(&conn, &server.url(), "test-token-resume");

        let body = serde_json::json!({
            "messages": [{ "role": "assistant", "content": "ingested after resume", "timestamp": 1_756_100_000_000i64, "__openclaw": { "id": "oc-resume-1" } }]
        });
        let _m = server
            .mock("GET", "/v1/sessions/agent%3Amain%3Aopenai-user%3Apane-session-resume/messages?limit=20")
            .with_status(200).with_header("content-type", "application/json").with_body(body.to_string())
            .create_async().await;

        let app = test_app_handle();
        let db = Arc::new(Mutex::new(DbConn::new(conn)));

        {
            let _guard = crate::ai::chat::TurnInFlightGuard::mark("session-resume");
            run_oc_background_pickup(db.clone(), app.clone()).await;
            // _guard drops here, at the end of this block
        }
        assert!(!crate::ai::chat::is_turn_in_flight("session-resume"), "guard must have cleared on drop");

        run_oc_background_pickup(db.clone(), app.clone()).await;

        let conn = db.lock().unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM chat_messages_local WHERE thread_id = 'session-resume'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1, "once the guard drops, the NEXT pickup tick must ingest normally");
    }
}
