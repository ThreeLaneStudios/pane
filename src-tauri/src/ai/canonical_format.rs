//! Provider-neutral canonical message format adapter (#395, PR5)
//!
//! ## Purpose
//!
//! Stores tool-call and tool-result history in a provider-neutral "canonical"
//! JSON shape so that sessions can be replayed to any provider (Anthropic,
//! OpenRouter, OC, Ollama) without wire-format errors.
//!
//! ## Canonical shape
//!
//! **`tool_calls` column** (JSON string) — array of:
//! ```json
//! [{"id": "toolu_abc", "name": "create_note", "params": {"title": "Test"}}]
//! ```
//!
//! **`tool_results` column** (JSON string) — array of:
//! ```json
//! [{"tool_use_id": "toolu_abc", "content": "Created note 'Test'", "is_error": false}]
//! ```
//!
//! ## Wire format adapters
//!
//! | Provider         | tool_calls wire                     | tool result wire                        |
//! |------------------|-------------------------------------|-----------------------------------------|
//! | Anthropic        | `content` array of `tool_use` blocks | user msg w/ `tool_result` block         |
//! | OpenAI / OR / OC | `tool_calls` array w/ `function` obj | role `"tool"` msg w/ `tool_call_id`     |
//!
//! ## Backward compatibility
//!
//! Pre-PR5 rows were stored in whichever provider format was active at write time.
//! `detect_and_normalize_stored_tool_calls` sniffs the encoding and normalises to
//! canonical before use. Anthropic format → `"type": "tool_use"` keys; OpenAI format
//! → `"function"` key. Canonical format → `"params"` key (no `"type"` / `"function"`).
//!
//! ## CI vs live-test gate
//!
//! Translation logic and roundtrips are fully `cargo test`-able. The acceptance
//! criterion (mid-session provider switch completing without API error) requires the
//! frontend session-switcher UI — **unverified until frontend switcher ships**.

use serde_json::{json, Value};

use super::tools::ToolCall;

// ─── Canonical shape builders ─────────────────────────────────────────────────

/// Convert a slice of in-flight [`ToolCall`]s into the canonical storage JSON array.
///
/// Output: `[{"id": "...", "name": "...", "params": {...}}, ...]`
pub fn tool_calls_to_canonical(tool_calls: &[ToolCall]) -> Value {
    let arr: Vec<Value> = tool_calls
        .iter()
        .map(|tc| {
            json!({
                "id": tc.call_id,
                "name": tc.name,
                "params": tc.params,
            })
        })
        .collect();
    Value::Array(arr)
}

/// Convert parallel slices of in-flight [`ToolCall`]s and result strings into
/// the canonical `tool_results` storage JSON array.
///
/// Output: `[{"tool_use_id": "...", "content": "...", "is_error": false}, ...]`
///
/// # Production caller status
///
/// **DEFERRED (§395 W-1)** — This function has no production callers yet.
/// The done event handler in `commands.rs` emits `all_tool_results: Vec<String>` (display
/// strings with ✅/❌ prefix). Wiring this function requires the frontend onDone handler to
/// distinguish display strings from canonical objects, touching `ChatSidebar.svelte`
/// and `ChatPanel.svelte`. Tracked as a follow-up to PR #1985.
///
/// Until then, `parse_stored_tool_results` handles the legacy plain-string-array
/// format by returning synthetic entries with `tool_use_id: ""` (which Anthropic
/// rejects on cross-session replay). Cross-session tool-result replay is **not supported**.
#[allow(dead_code)] // Deferred legacy tool-result wire migration.
pub fn tool_results_to_canonical(
    tool_calls: &[ToolCall],
    result_strings: &[String],
    is_errors: Option<&[bool]>,
) -> Value {
    let arr: Vec<Value> = tool_calls
        .iter()
        .zip(result_strings.iter())
        .enumerate()
        .map(|(i, (tc, result))| {
            let is_error = is_errors
                .and_then(|e| e.get(i))
                .copied()
                .unwrap_or(false);
            json!({
                "tool_use_id": tc.call_id,
                "content": result,
                "is_error": is_error,
            })
        })
        .collect();
    Value::Array(arr)
}

// ─── Canonical → Anthropic wire ──────────────────────────────────────────────

/// Translate canonical `tool_calls` array to Anthropic `content` blocks.
///
/// Anthropic expects `assistant` messages with tool calls to have `content` set
/// to an array of `{"type": "tool_use", "id": ..., "name": ..., "input": {...}}`.
///
/// Input: canonical `[{"id": ..., "name": ..., "params": {...}}]`
/// Output: Anthropic wire `[{"type": "tool_use", "id": ..., "name": ..., "input": {...}}]`
pub fn canonical_to_anthropic_wire(canonical: &Value) -> Value {
    let empty: Vec<Value> = vec![];
    let items = canonical.as_array().unwrap_or(&empty);
    let blocks: Vec<Value> = items
        .iter()
        .map(|item| {
            json!({
                "type": "tool_use",
                "id": item["id"].as_str().unwrap_or(""),
                "name": item["name"].as_str().unwrap_or(""),
                "input": item.get("params").cloned().unwrap_or(json!({})),
            })
        })
        .collect();
    Value::Array(blocks)
}

// ─── Canonical → OpenAI wire ─────────────────────────────────────────────────
//
// NOTE: canonical_results_to_anthropic_wire was deleted (#395 W-3).
// The function was unused in production and silently dropped `is_error`.
// When cross-session tool-result replay ships (§395 W-1 option a), a corrected
// version that preserves `is_error` should be added here and wired into the
// Anthropic dispatch boundary in anthropic.rs.
//

/// Translate canonical `tool_calls` array to OpenAI `tool_calls` wire format.
///
/// OpenAI / OpenRouter / OC / Ollama expect `assistant` messages with tool calls to have
/// a `tool_calls` field set to an array of
/// `{"id": ..., "type": "function", "function": {"name": ..., "arguments": "..."}}`.
///
/// `params` is serialized to a JSON string for `function.arguments`.
///
/// Input: canonical `[{"id": ..., "name": ..., "params": {...}}]`
/// Output: OpenAI wire `[{"id": ..., "type": "function", "function": {"name": ..., "arguments": "..."}}]`
pub fn canonical_to_openai_wire(canonical: &Value) -> Value {
    let empty: Vec<Value> = vec![];
    let items = canonical.as_array().unwrap_or(&empty);
    let tool_calls: Vec<Value> = items
        .iter()
        .map(|item| {
            let name = item["name"].as_str().unwrap_or("").to_string();
            let arguments = serde_json::to_string(
                item.get("params").unwrap_or(&json!({})),
            )
            .unwrap_or_else(|_| "{}".to_string());
            json!({
                "id": item["id"].as_str().unwrap_or(""),
                "type": "function",
                "function": {
                    "name": name,
                    "arguments": arguments,
                },
            })
        })
        .collect();
    Value::Array(tool_calls)
}

// ─── Ingest adapters (provider response → canonical) ─────────────────────────

/// Translate Anthropic response `tool_use` blocks to canonical format.
///
/// Anthropic returns tool calls as content blocks of `{"type": "tool_use", "id": ..., "name": ..., "input": {...}}`.
///
/// Input: Anthropic content array (may contain mixed text + tool_use blocks)
/// Output: canonical `[{"id": ..., "name": ..., "params": {...}}]`
pub fn anthropic_response_to_canonical(anthropic_content: &Value) -> Value {
    let empty: Vec<Value> = vec![];
    let items = anthropic_content.as_array().unwrap_or(&empty);
    let canonical: Vec<Value> = items
        .iter()
        .filter(|item| item.get("type").and_then(|t| t.as_str()) == Some("tool_use"))
        .map(|item| {
            json!({
                "id": item["id"].as_str().unwrap_or(""),
                "name": item["name"].as_str().unwrap_or(""),
                "params": item.get("input").cloned().unwrap_or(json!({})),
            })
        })
        .collect();
    Value::Array(canonical)
}

/// Translate OpenAI response `tool_calls` array to canonical format.
///
/// OpenAI returns tool calls as `[{"id": ..., "type": "function", "function": {"name": ..., "arguments": "..."}}]`.
/// `function.arguments` is a JSON-encoded string that must be parsed back to an object.
///
/// Input: OpenAI `tool_calls` array
/// Output: canonical `[{"id": ..., "name": ..., "params": {...}}]`
pub fn openai_response_to_canonical(openai_tool_calls: &Value) -> Value {
    let empty: Vec<Value> = vec![];
    let items = openai_tool_calls.as_array().unwrap_or(&empty);
    let canonical: Vec<Value> = items
        .iter()
        .map(|item| {
            let id = item["id"].as_str().unwrap_or("").to_string();
            let function = item.get("function").cloned().unwrap_or(json!({}));
            let name = function["name"].as_str().unwrap_or("").to_string();
            // arguments is a JSON-encoded string — parse it back to a Value
            let params: Value = function
                .get("arguments")
                .and_then(|a| a.as_str())
                .and_then(|s| serde_json::from_str(s).ok())
                .unwrap_or(json!({}));
            json!({
                "id": id,
                "name": name,
                "params": params,
            })
        })
        .collect();
    Value::Array(canonical)
}

// ─── Backward compatibility ───────────────────────────────────────────────────

/// Detect the format of a stored `tool_calls` JSON value and normalise to canonical.
///
/// Legacy rows were stored in whichever provider format was active when they were
/// written. Detection heuristic (checked on the first item in the array):
///
/// - `"type": "tool_use"` key present → Anthropic legacy → normalise via [`anthropic_response_to_canonical`]
/// - `"function"` key present → OpenAI legacy → normalise via [`openai_response_to_canonical`]
/// - `"params"` key present (and none of the above) → already canonical → return as-is
/// - Empty array or null → return empty array
///
/// Malformed JSON (passed as a `Value::Null` after failed parse) degrades to empty array.
pub fn detect_and_normalize_stored_tool_calls(stored: &Value) -> Value {
    let empty = Value::Array(vec![]);

    let arr = match stored.as_array() {
        Some(a) if !a.is_empty() => a,
        _ => return empty,
    };

    // Sniff format from first item
    let first = &arr[0];
    if first.get("type").and_then(|t| t.as_str()) == Some("tool_use") {
        // Anthropic legacy — `"input"` key holds params
        anthropic_response_to_canonical(stored)
    } else if first.get("function").is_some() {
        // OpenAI legacy — `"function"` key
        openai_response_to_canonical(stored)
    } else {
        // Already canonical (has "params" key) or unknown — pass through
        stored.clone()
    }
}

/// Parse a stored `tool_calls` JSON string, detect its format, and return canonical.
///
/// Returns an empty array if `stored_json` is `None`, empty, or unparseable.
#[allow(dead_code)] // Legacy stored wire-shape parser.
pub fn parse_stored_tool_calls(stored_json: Option<&str>) -> Value {
    let s = match stored_json {
        Some(s) if !s.is_empty() => s,
        _ => return Value::Array(vec![]),
    };
    match serde_json::from_str::<Value>(s) {
        Ok(v) => detect_and_normalize_stored_tool_calls(&v),
        Err(e) => {
            eprintln!(
                "[CANONICAL-FORMAT] Failed to parse stored tool_calls JSON: {} — treating as empty",
                e
            );
            Value::Array(vec![])
        }
    }
}

/// Parse a stored `tool_results` JSON string and return a canonical results array.
///
/// Unlike `tool_calls`, results were stored as string arrays (pre-PR5) rather than
/// structured JSON. This function handles both the legacy plain-string-array encoding
/// and the new canonical encoding.
///
/// Returns an empty array if `stored_json` is `None`, empty, or unparseable.
#[allow(dead_code)] // Legacy stored wire-shape parser.
pub fn parse_stored_tool_results(stored_json: Option<&str>) -> Value {
    let s = match stored_json {
        Some(s) if !s.is_empty() => s,
        _ => return Value::Array(vec![]),
    };
    match serde_json::from_str::<Value>(s) {
        Ok(Value::Array(arr)) => {
            // Check if first element is a plain string (legacy format: ["result1", "result2"])
            if arr.first().map(|v| v.is_string()).unwrap_or(false) {
                // Legacy: array of strings with no pairing info — cannot reconstruct tool_use_id.
                // Return as-is wrapped so callers can treat as opaque result log.
                Value::Array(
                    arr.into_iter()
                        .map(|s_val| {
                            json!({
                                "tool_use_id": "",
                                "content": s_val.as_str().unwrap_or(""),
                                "is_error": false,
                            })
                        })
                        .collect(),
                )
            } else {
                // Already canonical object format
                Value::Array(arr)
            }
        }
        Ok(other) => other,
        Err(e) => {
            eprintln!(
                "[CANONICAL-FORMAT] Failed to parse stored tool_results JSON: {} — treating as empty",
                e
            );
            Value::Array(vec![])
        }
    }
}

// ─── Message serialization helpers ───────────────────────────────────────────

/// Serialize a canonical `tool_calls` value to a JSON string for DB storage.
///
/// Returns `None` if the canonical array is empty.
#[allow(dead_code)] // Legacy canonical storage seam.
pub fn canonical_to_storage_string(canonical: &Value) -> Option<String> {
    match canonical.as_array() {
        Some(arr) if arr.is_empty() => None,
        _ => serde_json::to_string(canonical).ok(),
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::tools::ToolCall;

    fn make_tool_call(id: &str, name: &str, params: Value) -> ToolCall {
        ToolCall {
            call_id: id.to_string(),
            name: name.to_string(),
            params,
            args_parse_failed: false,
        }
    }

    // ─── canonical shape ─────────────────────────────────────────────────────

    #[test]
    fn test_tool_calls_to_canonical_shape() {
        let tc = make_tool_call("call_abc", "create_note", json!({"title": "Test"}));
        let canonical = tool_calls_to_canonical(&[tc]);
        let arr = canonical.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["id"], "call_abc");
        assert_eq!(arr[0]["name"], "create_note");
        assert_eq!(arr[0]["params"]["title"], "Test");
        // Must NOT contain provider-specific keys
        assert!(arr[0].get("type").is_none());
        assert!(arr[0].get("function").is_none());
        assert!(arr[0].get("input").is_none());
    }

    #[test]
    fn test_tool_results_to_canonical_shape() {
        let tc = make_tool_call("call_abc", "create_note", json!({"title": "Test"}));
        let results = vec!["Created note 'Test'".to_string()];
        let canonical = tool_results_to_canonical(&[tc], &results, None);
        let arr = canonical.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["tool_use_id"], "call_abc");
        assert_eq!(arr[0]["content"], "Created note 'Test'");
        assert_eq!(arr[0]["is_error"], false);
    }

    // ─── test_canonical_to_anthropic_wire ────────────────────────────────────

    #[test]
    fn test_canonical_to_anthropic_wire() {
        let canonical = json!([
            {"id": "toolu_01", "name": "create_note", "params": {"title": "Foo"}},
            {"id": "toolu_02", "name": "search_notes", "params": {"query": "bar"}}
        ]);
        let wire = canonical_to_anthropic_wire(&canonical);
        let arr = wire.as_array().unwrap();
        assert_eq!(arr.len(), 2);

        // First block
        assert_eq!(arr[0]["type"], "tool_use");
        assert_eq!(arr[0]["id"], "toolu_01");
        assert_eq!(arr[0]["name"], "create_note");
        assert_eq!(arr[0]["input"]["title"], "Foo");

        // Second block
        assert_eq!(arr[1]["type"], "tool_use");
        assert_eq!(arr[1]["id"], "toolu_02");
        assert_eq!(arr[1]["name"], "search_notes");
        assert_eq!(arr[1]["input"]["query"], "bar");
    }

    // ─── test_canonical_to_openai_wire ───────────────────────────────────────

    #[test]
    fn test_canonical_to_openai_wire() {
        let canonical = json!([
            {"id": "call_01", "name": "create_note", "params": {"title": "Foo"}},
            {"id": "call_02", "name": "list_tasks", "params": {}}
        ]);
        let wire = canonical_to_openai_wire(&canonical);
        let arr = wire.as_array().unwrap();
        assert_eq!(arr.len(), 2);

        // First entry
        assert_eq!(arr[0]["id"], "call_01");
        assert_eq!(arr[0]["type"], "function");
        assert_eq!(arr[0]["function"]["name"], "create_note");
        // arguments must be a JSON-encoded string
        let args_str = arr[0]["function"]["arguments"].as_str().unwrap();
        let args: Value = serde_json::from_str(args_str).unwrap();
        assert_eq!(args["title"], "Foo");

        // Second entry — empty params should serialize to "{}"
        let args2_str = arr[1]["function"]["arguments"].as_str().unwrap();
        assert_eq!(args2_str, "{}");
    }

    // ─── test_anthropic_response_to_canonical ────────────────────────────────

    #[test]
    fn test_anthropic_response_to_canonical() {
        // Mixed content array (text block + tool_use block)
        let anthropic_content = json!([
            {"type": "text", "text": "Let me create that note for you."},
            {"type": "tool_use", "id": "toolu_01", "name": "create_note", "input": {"title": "My Note"}}
        ]);
        let canonical = anthropic_response_to_canonical(&anthropic_content);
        let arr = canonical.as_array().unwrap();
        // Only tool_use blocks should appear in canonical
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["id"], "toolu_01");
        assert_eq!(arr[0]["name"], "create_note");
        assert_eq!(arr[0]["params"]["title"], "My Note");
        // Must not carry Anthropic-specific fields
        assert!(arr[0].get("type").is_none());
        assert!(arr[0].get("input").is_none());
    }

    // ─── test_openai_response_to_canonical ───────────────────────────────────

    #[test]
    fn test_openai_response_to_canonical() {
        let openai_tool_calls = json!([
            {
                "id": "call_01",
                "type": "function",
                "function": {
                    "name": "create_note",
                    "arguments": "{\"title\":\"My Note\",\"content\":\"Hello\"}"
                }
            }
        ]);
        let canonical = openai_response_to_canonical(&openai_tool_calls);
        let arr = canonical.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["id"], "call_01");
        assert_eq!(arr[0]["name"], "create_note");
        assert_eq!(arr[0]["params"]["title"], "My Note");
        assert_eq!(arr[0]["params"]["content"], "Hello");
        // Must not carry OpenAI-specific fields
        assert!(arr[0].get("type").is_none());
        assert!(arr[0].get("function").is_none());
    }

    // ─── test_canonical_roundtrip_anthropic ──────────────────────────────────

    #[test]
    fn test_canonical_roundtrip_anthropic() {
        let original_canonical = json!([
            {"id": "toolu_abc", "name": "create_note", "params": {"title": "Roundtrip", "content": "body"}},
            {"id": "toolu_def", "name": "list_tasks", "params": {}}
        ]);

        // Step 1: canonical → Anthropic wire (for sending to provider)
        let wire = canonical_to_anthropic_wire(&original_canonical);

        // Step 2: Anthropic wire → canonical (as if reading the response back)
        let restored = anthropic_response_to_canonical(&wire);

        let orig_arr = original_canonical.as_array().unwrap();
        let rest_arr = restored.as_array().unwrap();
        assert_eq!(rest_arr.len(), orig_arr.len(), "roundtrip must preserve item count");
        for i in 0..orig_arr.len() {
            assert_eq!(rest_arr[i]["id"], orig_arr[i]["id"], "id must survive roundtrip");
            assert_eq!(rest_arr[i]["name"], orig_arr[i]["name"], "name must survive roundtrip");
            assert_eq!(rest_arr[i]["params"], orig_arr[i]["params"], "params must survive roundtrip");
        }
    }

    // ─── test_canonical_roundtrip_openai ─────────────────────────────────────

    #[test]
    fn test_canonical_roundtrip_openai() {
        let original_canonical = json!([
            {"id": "call_xyz", "name": "edit_note", "params": {"note_id": "abc123", "content": "Updated"}},
            {"id": "call_uvw", "name": "set_status", "params": {"note_id": "abc123", "status": "done"}}
        ]);

        // Step 1: canonical → OpenAI wire
        let wire = canonical_to_openai_wire(&original_canonical);

        // Step 2: OpenAI wire → canonical
        let restored = openai_response_to_canonical(&wire);

        let orig_arr = original_canonical.as_array().unwrap();
        let rest_arr = restored.as_array().unwrap();
        assert_eq!(rest_arr.len(), orig_arr.len(), "roundtrip must preserve item count");
        for i in 0..orig_arr.len() {
            assert_eq!(rest_arr[i]["id"], orig_arr[i]["id"], "id must survive roundtrip");
            assert_eq!(rest_arr[i]["name"], orig_arr[i]["name"], "name must survive roundtrip");
            assert_eq!(rest_arr[i]["params"], orig_arr[i]["params"], "params must survive roundtrip");
        }
    }

    // ─── test_legacy_format_handled ──────────────────────────────────────────

    #[test]
    fn test_legacy_format_handled() {
        // Pre-PR5 OpenAI-style stored history
        let legacy_openai = json!([
            {
                "id": "call_legacy",
                "type": "function",
                "name": "create_note",
                "function": {
                    "name": "create_note",
                    "arguments": "{\"title\":\"Legacy Note\"}"
                }
            }
        ]);
        let canonical = detect_and_normalize_stored_tool_calls(&legacy_openai);
        let arr = canonical.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["id"], "call_legacy");
        assert_eq!(arr[0]["name"], "create_note");
        assert_eq!(arr[0]["params"]["title"], "Legacy Note");

        // Pre-PR5 Anthropic-style stored history
        let legacy_anthropic = json!([
            {
                "type": "tool_use",
                "id": "toolu_legacy",
                "name": "list_tasks",
                "input": {"status": "todo"}
            }
        ]);
        let canonical2 = detect_and_normalize_stored_tool_calls(&legacy_anthropic);
        let arr2 = canonical2.as_array().unwrap();
        assert_eq!(arr2.len(), 1);
        assert_eq!(arr2[0]["id"], "toolu_legacy");
        assert_eq!(arr2[0]["name"], "list_tasks");
        assert_eq!(arr2[0]["params"]["status"], "todo");
    }

    // ─── test_empty_tool_history ─────────────────────────────────────────────

    #[test]
    fn test_empty_tool_history() {
        // null stored value
        let result = parse_stored_tool_calls(None);
        assert_eq!(result, Value::Array(vec![]));

        // empty string
        let result = parse_stored_tool_calls(Some(""));
        assert_eq!(result, Value::Array(vec![]));

        // empty JSON array
        let result = parse_stored_tool_calls(Some("[]"));
        assert_eq!(result, Value::Array(vec![]));

        // tool_results null
        let result = parse_stored_tool_results(None);
        assert_eq!(result, Value::Array(vec![]));

        // tool_results empty string
        let result = parse_stored_tool_results(Some(""));
        assert_eq!(result, Value::Array(vec![]));

        // canonical_to_anthropic_wire on empty returns empty array
        let wire = canonical_to_anthropic_wire(&Value::Array(vec![]));
        assert_eq!(wire, Value::Array(vec![]));

        // canonical_to_openai_wire on empty returns empty array
        let wire = canonical_to_openai_wire(&Value::Array(vec![]));
        assert_eq!(wire, Value::Array(vec![]));

        // canonical_to_storage_string on empty returns None
        let s = canonical_to_storage_string(&Value::Array(vec![]));
        assert!(s.is_none());
    }

    // ─── test_malformed_tool_history ─────────────────────────────────────────

    #[test]
    fn test_malformed_tool_history() {
        // Invalid JSON string — must not panic, must return empty array
        let result = parse_stored_tool_calls(Some("{not valid json}"));
        assert_eq!(result, Value::Array(vec![]));

        // JSON but wrong type (object instead of array)
        let result = parse_stored_tool_calls(Some("{\"id\": \"x\"}"));
        // detect_and_normalize_stored_tool_calls receives a non-array → returns empty
        assert_eq!(result, Value::Array(vec![]));

        // tool_results: invalid JSON
        let result = parse_stored_tool_results(Some("[broken"));
        assert_eq!(result, Value::Array(vec![]));

        // canonical_to_anthropic_wire with null input
        let wire = canonical_to_anthropic_wire(&Value::Null);
        assert_eq!(wire, Value::Array(vec![]));

        // canonical_to_openai_wire with null input
        let wire = canonical_to_openai_wire(&Value::Null);
        assert_eq!(wire, Value::Array(vec![]));

        // detect_and_normalize_stored_tool_calls with empty array
        let result = detect_and_normalize_stored_tool_calls(&Value::Array(vec![]));
        assert_eq!(result, Value::Array(vec![]));
    }

    // ─── Additional wire format correctness checks ───────────────────────────

    #[test]
    fn test_anthropic_wire_has_required_fields() {
        let canonical = json!([{"id": "t1", "name": "create_note", "params": {"title": "X"}}]);
        let wire = canonical_to_anthropic_wire(&canonical);
        let arr = wire.as_array().unwrap();
        let block = &arr[0];
        assert!(block.get("type").is_some(), "Anthropic wire must have 'type'");
        assert!(block.get("id").is_some(), "Anthropic wire must have 'id'");
        assert!(block.get("name").is_some(), "Anthropic wire must have 'name'");
        assert!(block.get("input").is_some(), "Anthropic wire must have 'input'");
        assert_eq!(block["type"], "tool_use");
    }

    #[test]
    fn test_openai_wire_has_required_fields() {
        let canonical = json!([{"id": "c1", "name": "list_tasks", "params": {}}]);
        let wire = canonical_to_openai_wire(&canonical);
        let arr = wire.as_array().unwrap();
        let entry = &arr[0];
        assert!(entry.get("id").is_some(), "OpenAI wire must have 'id'");
        assert!(entry.get("type").is_some(), "OpenAI wire must have 'type'");
        assert!(entry.get("function").is_some(), "OpenAI wire must have 'function'");
        assert_eq!(entry["type"], "function");
        let f = &entry["function"];
        assert!(f.get("name").is_some(), "function must have 'name'");
        assert!(f.get("arguments").is_some(), "function must have 'arguments'");
        // arguments must be a valid JSON string
        serde_json::from_str::<Value>(f["arguments"].as_str().unwrap()).expect("arguments must parse as JSON");
    }

    #[test]
    fn test_parse_stored_tool_calls_canonical_passthrough() {
        // A stored canonical value should pass through unchanged
        let stored = r#"[{"id":"c1","name":"create_note","params":{"title":"Test"}}]"#;
        let result = parse_stored_tool_calls(Some(stored));
        let arr = result.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["id"], "c1");
        assert_eq!(arr[0]["name"], "create_note");
        assert_eq!(arr[0]["params"]["title"], "Test");
    }

    #[test]
    fn test_parse_stored_tool_results_legacy_string_array() {
        // Pre-PR5 tool_results were stored as plain string arrays
        let stored = r#"["✅ Created note 'Test'", "✅ Navigated to Test"]"#;
        let result = parse_stored_tool_results(Some(stored));
        let arr = result.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["content"], "✅ Created note 'Test'");
        assert_eq!(arr[0]["is_error"], false);
    }
}

/// §4.5.9 (#2874 item 60) — correctness requirement: "the failed result must
/// replay correctly in the next request to each provider... Anthropic's
/// `tool_use`/`tool_result` matching is strict — a result with no matching
/// call, or a call with no result, is a hard API error on the following turn."
///
/// This module tests that replay shape against a hand-authored,
/// protocol-accurate SSE transcript shaped exactly like a real Anthropic
/// streaming response that gets cut off mid-`tool_use` by the output-length
/// limit (`message_start` -> `content_block_start` (with `input: {}`, matching
/// Anthropic's own documented example) -> several `input_json_delta` chunks
/// that never close their JSON -> `content_block_stop` -> `message_delta` with
/// `stop_reason: "max_tokens"` AND `stop_sequence: null` (Anthropic always
/// pairs the two) -> `message_stop`). This is NOT a live-captured transcript
/// — no Anthropic API credential was available to this PR/session to record
/// one (see the PR report's "live test" section) — but every event name,
/// field name, and the overall event sequence matches Anthropic's documented
/// Messages API streaming protocol.
///
/// **PR 2 replacement note (#2874 item 60, per PR 1's audit WARN):** PR 1's
/// version of this fixture omitted `content_block_stop`, omitted `input: {}`
/// from `content_block_start`, and omitted `stop_sequence` from
/// `message_delta` — harmless for PR 1 (nothing read those fields yet), but
/// load-bearing now that PR 2's `stream_anthropic` reads `stop_reason` from
/// `message_delta`. This fixture is a full replacement, not an extension, and
/// `accumulate_sse_transcript` below now mirrors ALL of `stream_anthropic`'s
/// SSE match arms (`message_start`/`content_block_start`/`content_block_delta`/
/// `content_block_stop`/`message_delta`/`message_stop`), not just the three
/// PR 1 needed.
#[cfg(test)]
mod cutoff_replay_shape_tests {
    use super::*;
    use std::collections::HashMap;

    /// A recorded-shape Anthropic SSE transcript for a tool call cut off
    /// mid-argument by `max_tokens`. The `input_json_delta` chunks concatenate
    /// to `{"note_id":"n1","columns":["a","b"],"rows":[["1","2` — valid JSON
    /// up to the exact point the model's output was cut off, same shape as a
    /// real truncation (not synthetically malformed JSON).
    const RECORDED_SSE_TRANSCRIPT: &str = concat!(
        "event: message_start\n",
        "data: {\"type\":\"message_start\",\"message\":",
        "{\"id\":\"msg_01Cutoff\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[],",
        "\"model\":\"test-model\",\"stop_reason\":null,\"stop_sequence\":null,",
        "\"usage\":{\"input_tokens\":512,\"output_tokens\":1}}}\n\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":",
        "{\"type\":\"tool_use\",\"id\":\"toolu_01Cutoff\",\"name\":\"add_sheet_rows\",\"input\":{}}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":",
        "{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"note_id\\\":\\\"n1\\\",\"}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":",
        "{\"type\":\"input_json_delta\",\"partial_json\":\"\\\"columns\\\":[\\\"a\\\",\\\"b\\\"],\"}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":",
        "{\"type\":\"input_json_delta\",\"partial_json\":\"\\\"rows\\\":[[\\\"1\\\",\\\"2\"}}\n\n",
        "event: content_block_stop\n",
        "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
        "event: message_delta\n",
        "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"max_tokens\",\"stop_sequence\":null},",
        "\"usage\":{\"output_tokens\":16384}}\n\n",
        "event: message_stop\n",
        "data: {\"type\":\"message_stop\"}\n\n"
    );

    /// Test-only SSE accumulation, written to mirror exactly what
    /// `stream_anthropic`'s own `match event_type { ... }` arms do
    /// (`anthropic.rs`) for `content_block_start` / `content_block_delta` /
    /// `message_delta` (`delta.stop_reason`) — so this test exercises the
    /// real event shape, not a strawman. `message_start` and
    /// `content_block_stop` carry no fields production reads, so they're
    /// parsed (proving they don't break accumulation) but otherwise no-ops,
    /// matching production exactly. Stops at `message_stop`, same as
    /// production. Returns the accumulated per-index tool-call fragments
    /// AND the captured `stop_reason`, so the test below can resolve the
    /// real `CutoffSignal` instead of assuming one.
    fn accumulate_sse_transcript(transcript: &str) -> (HashMap<usize, (String, String, String)>, Option<String>) {
        let mut partial_tool_calls: HashMap<usize, (String, String, String)> = HashMap::new();
        let mut stop_reason: Option<String> = None;
        for block in transcript.split("\n\n") {
            let Some(data_line) = block.lines().find(|l| l.starts_with("data: ")) else { continue };
            let data: Value = serde_json::from_str(&data_line[6..]).expect("fixture must be valid JSON per line");
            match data["type"].as_str().unwrap_or("") {
                "content_block_start" => {
                    let index = data["index"].as_u64().unwrap_or(0) as usize;
                    let block = &data["content_block"];
                    if block["type"].as_str() == Some("tool_use") {
                        let id = block["id"].as_str().unwrap_or("").to_string();
                        let name = block["name"].as_str().unwrap_or("").to_string();
                        partial_tool_calls.insert(index, (id, name, String::new()));
                    }
                }
                "content_block_delta" => {
                    let index = data["index"].as_u64().unwrap_or(0) as usize;
                    let delta = &data["delta"];
                    if delta["type"].as_str() == Some("input_json_delta") {
                        if let Some(partial_json) = delta["partial_json"].as_str() {
                            if let Some(entry) = partial_tool_calls.get_mut(&index) {
                                entry.2.push_str(partial_json);
                            }
                        }
                    }
                }
                "message_delta" => {
                    if let Some(sr) = data["delta"]["stop_reason"].as_str() {
                        stop_reason = Some(sr.to_string());
                    }
                }
                "message_stop" => break,
                _ => {}
            }
        }
        (partial_tool_calls, stop_reason)
    }

    /// Test 4 (#2874 item 60 correctness requirement): the Anthropic replay
    /// shape is valid for a cut-off tool call, proven against the recorded-
    /// shape transcript above, using the SAME production functions commands.rs
    /// wires together (`tool_calls_to_canonical` ->
    /// `detect_and_normalize_stored_tool_calls` -> `canonical_to_anthropic_wire`
    /// for the assistant's replayed `tool_use` block; the literal shape
    /// `anthropic.rs`'s message-builder uses for a `role:"tool"` message for
    /// the matching `tool_result` block).
    #[test]
    fn test_cutoff_tool_call_replays_as_valid_anthropic_tool_use_tool_result_pair() {
        let (partial_tool_calls, stop_reason) = accumulate_sse_transcript(RECORDED_SSE_TRANSCRIPT);
        assert_eq!(partial_tool_calls.len(), 1, "transcript carries exactly one tool_use block");
        assert_eq!(
            stop_reason.as_deref(), Some("max_tokens"),
            "fixture's message_delta.stop_reason must be captured — this is PR 2's signal-plumbing being exercised, not just PR 1's replay shape"
        );
        let cutoff_signal = super::super::anthropic::anthropic_cutoff_signal(stop_reason.as_deref());
        assert_eq!(cutoff_signal, crate::ai::tools::CutoffSignal::ConfirmedCutoff);

        // `finalize_tool_calls` lives in `anthropic.rs`; reproduce its match arm
        // here rather than reaching across the crate — the parse-failure shape
        // itself (params: {} placeholder, args_parse_failed: true) is already
        // covered by anthropic.rs's own `finalize_tool_calls_tests`. This test's
        // job is what happens to that shape AFTER, through the wire-format
        // functions this file owns.
        let (id, name, args_json) = partial_tool_calls.into_values().next().unwrap();
        assert!(
            serde_json::from_str::<Value>(&args_json).is_err(),
            "the accumulated JSON must genuinely fail to parse (cut off, unclosed) for this to be a real test of the failure path"
        );
        let failed_call = ToolCall {
            call_id: id.clone(),
            name: name.clone(),
            params: json!({}),
            args_parse_failed: true,
        };

        // 1) Assistant replay message: tool_calls_to_canonical -> wire.
        let canonical = tool_calls_to_canonical(&[failed_call.clone()]);
        let normalized = detect_and_normalize_stored_tool_calls(&canonical);
        let anthropic_blocks = canonical_to_anthropic_wire(&normalized);
        let blocks = anthropic_blocks.as_array().expect("must be an array");
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0]["type"], "tool_use");
        assert_eq!(
            blocks[0]["id"], id,
            "the replayed tool_use id must be the SAME call_id the original (truncated) call carried — \
             this is what lets Anthropic match it to the tool_result below"
        );
        assert_eq!(blocks[0]["name"], name);
        assert!(
            blocks[0]["input"].is_object(),
            "Anthropic's tool_use.input MUST be a JSON object on replay — not null, not a string. \
             Got: {:?}. A bare parse-failure sentinel (e.g. Value::Null) would fail this.",
            blocks[0]["input"]
        );

        // 2) Matching tool_result message — same literal shape
        // `anthropic.rs`'s message-builder uses for `m.role == "tool"`
        // (`{"role":"user","content":[{"type":"tool_result","tool_use_id":...,"content":...}]}`),
        // built from the SAME `call_id` the §4.5.9 gate (`check_args_parsed`,
        // commands.rs) attaches its refusal message to. PR 2: this transcript's
        // real `stop_reason: "max_tokens"` resolves to `ConfirmedCutoff`
        // (asserted above), so the message is the §4.5.9 "cut off" text, not
        // the "no signal available" fallback PR 1 always produced.
        let refusal_message = format!(
            "Your `{}` call was cut off before it finished (hit the output length limit) and could not run. Split it into smaller calls and try again.",
            failed_call.name
        );
        let tool_result_wire = json!({
            "role": "user",
            "content": [{
                "type": "tool_result",
                "tool_use_id": failed_call.call_id,
                "content": refusal_message
            }]
        });

        // 3) The strict Anthropic pairing requirement: tool_use.id ==
        // tool_result.tool_use_id, both present, neither orphaned.
        assert_eq!(
            blocks[0]["id"], tool_result_wire["content"][0]["tool_use_id"],
            "tool_use block's id must equal the paired tool_result's tool_use_id — \
             a mismatch here is exactly the 'hard API error on the following turn' \
             the #2874 item 60 correctness requirement warns about"
        );
        assert!(
            tool_result_wire["content"][0]["content"]
                .as_str()
                .unwrap()
                .contains("could not run"),
            "the tool_result's content must carry the §4.5.9 refusal, not the tool's real output — \
             the call never dispatched"
        );
    }
}
