use super::tools::{ToolDef, ToolCall, ToolResult};
use serde_json::Value;

/// Provider adapter trait for normalizing tool calling across different LLM APIs
pub trait ProviderAdapter {
    /// Convert Pane tool definitions to provider-specific format
    fn format_tools(&self, tools: &[ToolDef]) -> Value;
    
    /// Extract tool calls from provider response
    fn extract_tool_calls(&self, response: &Value) -> Vec<ToolCall>;
    
    /// Format tool results for the next API call
    #[allow(dead_code)] // Legacy provider-adapter seam.
    fn format_tool_results(&self, results: &[ToolResult]) -> Vec<Value>;
}

/// OpenAI/OpenRouter adapter (they use the same format)
pub struct OpenAIAdapter;

impl ProviderAdapter for OpenAIAdapter {
    fn format_tools(&self, tools: &[ToolDef]) -> Value {
        let formatted: Vec<Value> = tools.iter().map(|tool| {
            serde_json::json!({
                "type": "function",
                "function": {
                    "name": tool.name,
                    "description": tool.description,
                    "parameters": tool.parameters
                }
            })
        }).collect();
        Value::Array(formatted)
    }
    
    fn extract_tool_calls(&self, response: &Value) -> Vec<ToolCall> {
        let mut calls = Vec::new();
        
        if let Some(tool_calls) = response["choices"][0]["message"]["tool_calls"].as_array() {
            for call in tool_calls {
                // §4.5.9 (#2874 item 60): extract `id`/`name` independently of
                // `arguments` so both survive even when arguments are missing
                // or don't parse. BEFORE this PR, the all-or-nothing
                // `if let (Some, Some, Some)` tuple pattern below dropped the
                // ENTIRE call — id and name included — the moment arguments
                // was absent or unparseable; the agentic loop could never
                // refuse the call on its own `call_id` because the call_id
                // was gone too.
                let id = call["id"].as_str();
                let name = call["function"]["name"].as_str();
                let (id, name) = match (id, name) {
                    (Some(id), Some(name)) => (id.to_string(), name.to_string()),
                    // Neither id nor name is recoverable — nothing to attach a
                    // result to on the next turn. Nothing legitimate to log either.
                    _ => continue,
                };
                let args_str = call["function"]["arguments"].as_str();
                let json_str = match args_str {
                    Some(s) if !s.trim().is_empty() => s.to_string(),
                    // Missing or empty arguments == a real zero-arg call's wire
                    // shape — not a parse failure.
                    _ => "{}".to_string(),
                };
                match serde_json::from_str::<Value>(&json_str) {
                    Ok(params) => {
                        calls.push(ToolCall {
                            call_id: id,
                            name,
                            params,
                            args_parse_failed: false,
                        });
                    }
                    Err(e) => {
                        eprintln!(
                            "[OPENAI-ADAPTER] Tool args failed to parse for call_id={} name={}: {} (raw: {})",
                            id, name, e, json_str
                        );
                        calls.push(ToolCall {
                            call_id: id,
                            name,
                            params: serde_json::json!({}),
                            args_parse_failed: true,
                        });
                    }
                }
            }
        }
        
        calls
    }
    
    fn format_tool_results(&self, results: &[ToolResult]) -> Vec<Value> {
        results.iter().map(|result| {
            serde_json::json!({
                "role": "tool",
                "tool_call_id": result.call_id,
                "content": result.result
            })
        }).collect()
    }
}

/// Anthropic adapter
pub struct AnthropicAdapter;

impl ProviderAdapter for AnthropicAdapter {
    fn format_tools(&self, tools: &[ToolDef]) -> Value {
        // Anthropic format: [{name, description, input_schema}]
        serde_json::json!(
            tools.iter().map(|tool| {
                serde_json::json!({
                    "name": tool.name,
                    "description": tool.description,
                    "input_schema": tool.parameters
                })
            }).collect::<Vec<_>>()
        )
    }
    
    fn extract_tool_calls(&self, response: &Value) -> Vec<ToolCall> {
        let mut calls = Vec::new();
        
        // Anthropic puts tool_use blocks in content array
        if let Some(content_array) = response["content"].as_array() {
            for block in content_array {
                if block["type"].as_str() == Some("tool_use") {
                    // §4.5.9 (#2874 item 60): extract `id`/`name` independently
                    // of `input` so both survive even if `input` is absent.
                    // Anthropic's non-streaming Messages API delivers `input`
                    // already parsed (never a truncatable JSON string) — per
                    // the #2874 item 60 scope audit this path cannot hit a
                    // genuine "invalid JSON" failure the way the streaming
                    // paths can, but a missing `input` key entirely is still
                    // defended against here rather than assumed away, same as
                    // the all-or-nothing tuple pattern this replaces used to
                    // drop id+name on.
                    let id = block["id"].as_str();
                    let name = block["name"].as_str();
                    let (id, name) = match (id, name) {
                        (Some(id), Some(name)) => (id.to_string(), name.to_string()),
                        _ => continue,
                    };
                    match block.get("input") {
                        Some(input) => {
                            calls.push(ToolCall {
                                call_id: id,
                                name,
                                params: input.clone(),
                                args_parse_failed: false,
                            });
                        }
                        None => {
                            eprintln!(
                                "[ANTHROPIC-ADAPTER] tool_use block missing 'input' for call_id={} name={}",
                                id, name
                            );
                            calls.push(ToolCall {
                                call_id: id,
                                name,
                                params: serde_json::json!({}),
                                args_parse_failed: true,
                            });
                        }
                    }
                }
            }
        }
        
        calls
    }
    
    fn format_tool_results(&self, results: &[ToolResult]) -> Vec<Value> {
        // Anthropic expects tool results as a user message with tool_result blocks
        if results.is_empty() {
            return vec![];
        }
        
        let content_blocks: Vec<Value> = results.iter().map(|result| {
            serde_json::json!({
                "type": "tool_result",
                "tool_use_id": result.call_id,
                "content": result.result
            })
        }).collect();
        
        vec![serde_json::json!({
            "role": "user",
            "content": content_blocks
        })]
    }
}

/// Ollama adapter (uses OpenAI format)
#[allow(dead_code)] // Retained for the legacy adapter seam.
pub struct OllamaAdapter;

impl ProviderAdapter for OllamaAdapter {
    fn format_tools(&self, tools: &[ToolDef]) -> Value {
        OpenAIAdapter.format_tools(tools)
    }
    
    fn extract_tool_calls(&self, response: &Value) -> Vec<ToolCall> {
        OpenAIAdapter.extract_tool_calls(response)
    }
    
    fn format_tool_results(&self, results: &[ToolResult]) -> Vec<Value> {
        OpenAIAdapter.format_tool_results(results)
    }
}

#[cfg(test)]
mod extract_tool_calls_tests {
    //! §4.5.9 (#2874 item 60) — non-streaming adapters. Both adapters here
    //! previously used an all-or-nothing `if let (Some, Some, Some)` tuple
    //! pattern: if `arguments`/`input` was missing or unparseable, the ENTIRE
    //! call — `id` and `name` included — was silently dropped. These tests
    //! prove the restructured extraction keeps `call_id`/`name` recoverable
    //! and flags `args_parse_failed` instead of dropping the call.
    use super::*;
    use serde_json::json;

    // ── OpenAIAdapter (OpenRouter/Ollama/OpenClaw one-shot) ─────────────────

    /// Test 1: a truncated/invalid `arguments` string no longer produces a
    /// dispatchable call. BEFORE this PR: dropped entirely (0 calls returned).
    /// AFTER: 1 call returned, flagged `args_parse_failed: true`.
    #[test]
    fn test_openai_adapter_truncated_arguments_not_dispatched() {
        let response = json!({
            "choices": [{
                "message": {
                    "tool_calls": [{
                        "id": "call_cut1",
                        "function": {
                            "name": "add_sheet_rows",
                            "arguments": "{\"note_id\":\"n1\",\"rows\":[[\"cut off"
                        }
                    }]
                }
            }]
        });
        let calls = OpenAIAdapter.extract_tool_calls(&response);
        assert_eq!(
            calls.len(),
            1,
            "BEFORE this PR this would be 0 (silently dropped); AFTER it must be 1 (preserved, marked failed)"
        );
        assert!(calls[0].args_parse_failed);
    }

    /// Test 2: `call_id` and `name` survive even when `arguments` is entirely
    /// absent from the response (not just unparseable).
    #[test]
    fn test_openai_adapter_missing_arguments_preserves_id_and_name() {
        let response = json!({
            "choices": [{
                "message": {
                    "tool_calls": [{
                        "id": "call_noargs",
                        "function": { "name": "list_notes" }
                    }]
                }
            }]
        });
        let calls = OpenAIAdapter.extract_tool_calls(&response);
        assert_eq!(calls.len(), 1, "id/name present — call must survive");
        assert_eq!(calls[0].call_id, "call_noargs");
        assert_eq!(calls[0].name, "list_notes");
        assert!(
            !calls[0].args_parse_failed,
            "a genuinely zero-arg call (no arguments field at all) is a normal call, not a parse failure"
        );
    }

    /// Well-formed arguments still parse normally (no regression).
    #[test]
    fn test_openai_adapter_valid_arguments_parse_normally() {
        let response = json!({
            "choices": [{
                "message": {
                    "tool_calls": [{
                        "id": "call_ok1",
                        "function": {
                            "name": "create_note",
                            "arguments": "{\"title\":\"Groceries\"}"
                        }
                    }]
                }
            }]
        });
        let calls = OpenAIAdapter.extract_tool_calls(&response);
        assert_eq!(calls.len(), 1);
        assert!(!calls[0].args_parse_failed);
        assert_eq!(calls[0].params["title"], "Groceries");
    }

    /// Test 3: a mixed batch (one failed + one successful call) keeps BOTH,
    /// each with its own distinct `call_id` — required for 1:1 result pairing
    /// downstream in the agentic loop.
    #[test]
    fn test_openai_adapter_mixed_batch_keeps_both_calls() {
        let response = json!({
            "choices": [{
                "message": {
                    "tool_calls": [
                        {
                            "id": "call_ok2",
                            "function": { "name": "create_note", "arguments": "{\"title\":\"Fine\"}" }
                        },
                        {
                            "id": "call_bad2",
                            "function": { "name": "add_sheet_rows", "arguments": "{\"note_id\":\"n1\",\"rows\":[[\"cut" }
                        }
                    ]
                }
            }]
        });
        let calls = OpenAIAdapter.extract_tool_calls(&response);
        assert_eq!(calls.len(), 2, "both calls must survive extraction");
        assert_eq!(calls[0].call_id, "call_ok2");
        assert!(!calls[0].args_parse_failed);
        assert_eq!(calls[1].call_id, "call_bad2");
        assert!(calls[1].args_parse_failed);
    }

    // ── AnthropicAdapter (non-streaming one-shot) ───────────────────────────

    /// `call_id`/`name` survive even when `input` is entirely absent from a
    /// `tool_use` block (defensive — real Anthropic responses always include
    /// `input`, per the #2874 item 60 scope audit, but the extraction must
    /// not assume well-formed provider data).
    #[test]
    fn test_anthropic_adapter_missing_input_preserves_id_and_name() {
        let response = json!({
            "content": [{
                "type": "tool_use",
                "id": "toolu_noinput",
                "name": "list_notes"
            }]
        });
        let calls = AnthropicAdapter.extract_tool_calls(&response);
        assert_eq!(calls.len(), 1, "BEFORE this PR the all-or-nothing tuple pattern would drop this call entirely");
        assert_eq!(calls[0].call_id, "toolu_noinput");
        assert_eq!(calls[0].name, "list_notes");
        assert!(calls[0].args_parse_failed);
    }

    /// Well-formed `tool_use` blocks still parse normally (no regression).
    #[test]
    fn test_anthropic_adapter_valid_input_parses_normally() {
        let response = json!({
            "content": [{
                "type": "tool_use",
                "id": "toolu_ok1",
                "name": "create_note",
                "input": {"title": "Groceries"}
            }]
        });
        let calls = AnthropicAdapter.extract_tool_calls(&response);
        assert_eq!(calls.len(), 1);
        assert!(!calls[0].args_parse_failed);
        assert_eq!(calls[0].params["title"], "Groceries");
    }

    /// A mixed batch (one missing-input + one well-formed `tool_use` block)
    /// keeps both, each with its own distinct `call_id`.
    #[test]
    fn test_anthropic_adapter_mixed_batch_keeps_both_calls() {
        let response = json!({
            "content": [
                {"type": "tool_use", "id": "toolu_ok2", "name": "create_note", "input": {"title": "Fine"}},
                {"type": "tool_use", "id": "toolu_bad2", "name": "add_sheet_rows"}
            ]
        });
        let calls = AnthropicAdapter.extract_tool_calls(&response);
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].call_id, "toolu_ok2");
        assert!(!calls[0].args_parse_failed);
        assert_eq!(calls[1].call_id, "toolu_bad2");
        assert!(calls[1].args_parse_failed);
    }
}
