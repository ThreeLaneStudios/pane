use super::provider::*;
use super::tools::{ToolCall, ToolDef, CutoffSignal};
use reqwest;
use serde_json::{json, Value};
use futures_util::StreamExt;
use tauri::Emitter;

#[derive(Debug, serde::Serialize)]
pub struct OllamaProbeResult {
    pub reachable: bool,
    pub models: Vec<String>,
    pub error_class: Option<String>, // "unreachable" | "no_models" | "tags_failed" | None (success)
}

pub struct OllamaProvider {
    endpoint: String,  // default: http://localhost:11434
}

impl OllamaProvider {
    pub fn new(endpoint: Option<String>) -> Self {
        Self {
            endpoint: endpoint.unwrap_or_else(|| "http://localhost:11434".to_string()),
        }
    }
    
    /// Two-call presence probe: /api/version (reachability) then /api/tags (models)
    pub async fn probe_two_step(&self) -> OllamaProbeResult {
        eprintln!("[OLLAMA-PROBE] probe_two_step called with endpoint: {}", self.endpoint);

        // Step 1: GET /api/version with 5s timeout
        let client = match reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()
        {
            Ok(c) => c,
            Err(e) => {
                eprintln!("[OLLAMA-PROBE] Failed to build HTTP client: {}", e);
                return OllamaProbeResult {
                    reachable: false,
                    models: vec![],
                    error_class: Some("unreachable".to_string()),
                };
            }
        };

        let version_res = client
            .get(format!("{}/api/version", self.endpoint))
            .send()
            .await;

        match version_res {
            Err(e) => {
                eprintln!("[OLLAMA-PROBE] /api/version failed: {}", e);
                return OllamaProbeResult {
                    reachable: false,
                    models: vec![],
                    error_class: Some("unreachable".to_string()),
                };
            }
            Ok(r) if r.status().is_server_error() => {
                eprintln!("[OLLAMA-PROBE] /api/version returned 5xx: {}", r.status());
                return OllamaProbeResult {
                    reachable: false,
                    models: vec![],
                    error_class: Some("unreachable".to_string()),
                };
            }
            Ok(_) => {
                eprintln!("[OLLAMA-PROBE] /api/version OK — proceeding to /api/tags");
            }
        }

        // Step 2: GET /api/tags with 10s timeout
        let tags_client = match reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
        {
            Ok(c) => c,
            Err(e) => {
                eprintln!("[OLLAMA-PROBE] Failed to build tags HTTP client: {}", e);
                return OllamaProbeResult {
                    reachable: true,
                    models: vec![],
                    error_class: Some("tags_failed".to_string()),
                };
            }
        };

        let tags_res = tags_client
            .get(format!("{}/api/tags", self.endpoint))
            .send()
            .await;

        let tags_response = match tags_res {
            Err(e) => {
                eprintln!("[OLLAMA-PROBE] /api/tags request failed: {}", e);
                return OllamaProbeResult {
                    reachable: true,
                    models: vec![],
                    error_class: Some("tags_failed".to_string()),
                };
            }
            Ok(r) => r,
        };

        if !tags_response.status().is_success() {
            eprintln!("[OLLAMA-PROBE] /api/tags returned non-200: {}", tags_response.status());
            return OllamaProbeResult {
                reachable: true,
                models: vec![],
                error_class: Some("tags_failed".to_string()),
            };
        }

        let body: serde_json::Value = match tags_response.json().await {
            Ok(v) => v,
            Err(e) => {
                eprintln!("[OLLAMA-PROBE] Failed to parse /api/tags response: {}", e);
                return OllamaProbeResult {
                    reachable: true,
                    models: vec![],
                    error_class: Some("tags_failed".to_string()),
                };
            }
        };

        let models: Vec<String> = body["models"]
            .as_array()
            .unwrap_or(&vec![])
            .iter()
            .filter_map(|m| m["name"].as_str().map(|s| s.to_string()))
            .collect();

        if models.is_empty() {
            eprintln!("[OLLAMA-PROBE] /api/tags returned empty models list");
            return OllamaProbeResult {
                reachable: true,
                models: vec![],
                error_class: Some("no_models".to_string()),
            };
        }

        eprintln!("[OLLAMA-PROBE] probe_two_step succeeded: {} model(s)", models.len());
        OllamaProbeResult {
            reachable: true,
            models,
            error_class: None,
        }
    }

    /// Check if Ollama is running
    pub async fn is_available(&self) -> bool {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_millis(500))
            .build()
            .unwrap_or_default();
        
        client.get(format!("{}/api/tags", self.endpoint))
            .send()
            .await
            .is_ok()
    }
    
    /// List available models
    pub async fn list_models(&self) -> Result<Vec<String>, String> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| format!("Failed to build HTTP client: {}", e))?;
        let res = client.get(format!("{}/api/tags", self.endpoint))
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    "Ollama did not respond within 30 seconds. Is it running?".to_string()
                } else {
                    format!("Ollama request failed: {}", e)
                }
            })?;
        
        let body: Value = res.json().await
            .map_err(|e| format!("Failed to parse response: {}", e))?;
        
        let models: Vec<String> = body["models"]
            .as_array()
            .unwrap_or(&vec![])
            .iter()
            .filter_map(|m| m["name"].as_str().map(|s| s.to_string()))
            .collect();
        
        Ok(models)
    }
    
    pub async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, String> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| format!("Failed to build HTTP client: {}", e))?;
        
        let messages: Vec<Value> = request.messages.iter()
            .map(|m| json!({
                "role": m.role,
                "content": m.content,
            }))
            .collect();
        
        let body = json!({
            "model": request.model,
            "messages": messages,
            "stream": false,
            "options": {
                "temperature": request.temperature,
                "num_predict": request.max_tokens as i64,
            }
        });
        
        let res = client
            .post(format!("{}/api/chat", self.endpoint))
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    "Ollama did not respond within 30 seconds. Is it running?".to_string()
                } else {
                    format!("Ollama request failed: {}", e)
                }
            })?;
        
        let status = res.status();
        let response_body: Value = res.json().await
            .map_err(|e| format!("Failed to parse response: {}", e))?;
        
        if !status.is_success() {
            return Err(format!("Ollama error ({}): {}", status, response_body));
        }
        
        let text = response_body["message"]["content"]
            .as_str()
            .unwrap_or("")
            .to_string();
        
        let input_tokens = response_body["prompt_eval_count"].as_u64().unwrap_or(0) as u32;
        let output_tokens = response_body["eval_count"].as_u64().unwrap_or(0) as u32;
        
        Ok(ChatResponse {
            text,
            model: request.model,
            input_tokens,
            output_tokens,
        })
    }
}

/// Format `ToolDef` slice into the OpenAI-compatible tools array that Ollama expects.
/// Shape: `[{"type":"function","function":{"name","description","parameters"}}]`
fn format_tools_for_ollama(tools: &[ToolDef]) -> serde_json::Value {
    let formatted: Vec<serde_json::Value> = tools
        .iter()
        .map(|t| {
            json!({
                "type": "function",
                "function": {
                    "name": t.name,
                    "description": t.description,
                    "parameters": t.parameters,
                }
            })
        })
        .collect();
    serde_json::Value::Array(formatted)
}

/// Parse `message.tool_calls` from an Ollama NDJSON chunk.
///
/// Ollama's format is OpenAI-shaped but with two differences from the OpenAI wire format:
/// 1. The `id` field on each tool call is optional (generate a synthetic id when absent).
/// 2. `function.arguments` is a JSON **object**, not a JSON-encoded string.
///
/// Returns a `Vec<ToolCall>` (may be empty if no tool calls are present).
pub fn parse_ollama_tool_calls(data: &serde_json::Value) -> Vec<ToolCall> {
    let Some(arr) = data["message"]["tool_calls"].as_array() else {
        return vec![];
    };
    let mut result = Vec::new();
    for (idx, call) in arr.iter().enumerate() {
        let Some(name) = call["function"]["name"].as_str() else {
            eprintln!("[OLLAMA-TOOL] Skipping tool call #{} — missing function.name", idx);
            continue;
        };
        // Ollama may omit the id field; synthesise one to satisfy downstream consumers.
        let call_id = call["id"]
            .as_str()
            .map(|s| s.to_string())
            .unwrap_or_else(|| format!("ollama_tool_{}", idx + 1));
        // arguments may be an object (Ollama native) or a JSON string (OpenAI compat).
        // §4.5.9 (#2874 item 60) PR 4 — mirrors `OpenAIAdapter::extract_tool_calls`
        // (adapter.rs): a string-arguments payload that fails to parse is NEVER
        // blanked to `{}` and dispatched. BEFORE this PR,
        // `.unwrap_or_else(|_| json!({}))` silently substituted `{}` and the call
        // ran with `args_parse_failed` hardcoded `false` — identical in shape to
        // Anthropic's own pre-#2992 blank-and-dispatch bug. `call_id`/`name` are
        // already in hand (extracted above), so they survive into the flagged
        // call for `check_args_parsed` (commands.rs) to refuse on the next turn.
        let (params, args_parse_failed) = if call["function"]["arguments"].is_object() {
            (call["function"]["arguments"].clone(), false)
        } else if let Some(s) = call["function"]["arguments"].as_str() {
            match serde_json::from_str::<serde_json::Value>(s) {
                Ok(v) => (v, false),
                Err(e) => {
                    eprintln!(
                        "[OLLAMA-TOOL] Tool args failed to parse for call_id={} name={}: {} (raw: {})",
                        call_id, name, e, s
                    );
                    (json!({}), true)
                }
            }
        } else {
            (json!({}), false)
        };
        result.push(ToolCall {
            name: name.to_string(),
            params,
            call_id,
            args_parse_failed,
        });
    }
    result
}

/// Build the JSON request body for Ollama's `/api/chat` streaming endpoint.
///
/// Extracted from `stream_ollama` for direct unit testing of the pure
/// body-building logic, the same way `format_tools_for_ollama` already is
/// above. #3059: `stream_ollama` itself is now generic over `R: tauri::Runtime`
/// (it no longer needs this extraction to be reachable from
/// `tauri::test::mock_app()`'s `AppHandle<MockRuntime>` — see
/// `test_run_agent_loop_survives_progress_past_old_idle_limit` in
/// `ai/commands.rs`, which now drives `stream_ollama` directly through the
/// real `run_agent_loop`); this helper stays extracted anyway, since testing
/// pure body-construction logic without a network round trip is still the
/// better-targeted test regardless of what `stream_ollama`'s own signature
/// requires.
///
/// §4.5.9 (#2874 item 60) PR 4 — `options.num_predict` is now set to the
/// caller-supplied `max_tokens` (the shared `CHAT_STREAM_MAX_TOKENS`
/// constant, threaded in by every `stream_ollama` call site the same way
/// every other provider threads its own max-output-tokens parameter through
/// its request body). BEFORE this PR, Ollama's streaming request carried no
/// output-length limit at all — `options` didn't exist in the body — so
/// Ollama was the one provider with no shared budget ceiling, governed
/// instead by whatever default `num_predict` Ollama/the model itself applies.
fn build_ollama_chat_body(model: &str, msg_values: Vec<Value>, tools: &[ToolDef], max_tokens: u32) -> Value {
    let mut body = json!({
        "model": model,
        "messages": msg_values,
        "stream": true,
        "options": {
            "num_predict": max_tokens,
        },
    });
    // Include tools in the request body so Ollama will populate message.tool_calls.
    // Without this, the model never emits native tool calls regardless of parsing.
    if !tools.is_empty() {
        body["tools"] = format_tools_for_ollama(tools);
    }
    body
}

/// Stream Ollama responses with NDJSON
///
/// #3059: `app` is generic over `R: tauri::Runtime` (rather than the implicit
/// `Wry` default) — same rationale and same mechanical change as
/// `stream_openrouter`'s `R: tauri::Runtime` generification (ADR-0067 D2/D5,
/// extended here per #3059 so `run_agent_loop<R>` compiles for every provider
/// arm, not just OpenRouter/OpenClaw/Venice). Every production call site still
/// passes a concrete `&tauri::AppHandle` (= `AppHandle<Wry>`); Rust infers
/// `R = Wry` there with zero call-site changes.
pub async fn stream_ollama<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    stream_id: &str,
    endpoint: &str,
    model: &str,
    messages: Vec<ChatMessage>,
    tools: &[ToolDef],
    max_tokens: u32,
) -> Result<(String, Vec<ToolCall>, CutoffSignal), String> {
    eprintln!("[OLLAMA-STREAM] Starting stream for model: {}, endpoint: {}", model, endpoint);
    
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(30))
        .timeout(std::time::Duration::from_secs(300))
        .build()
        .map_err(|e| format!("Failed to build HTTP client: {}", e))?;
    
    // Serialize messages with tool_calls and tool_call_id support.
    // tool_calls are stored in canonical format — translate to OpenAI wire here (#395 PR5).
    // Ollama speaks the OpenAI-compatible tool format (same wire shape as openrouter.rs).
    let msg_values: Vec<Value> = messages.iter()
        .map(|m| {
            let mut msg = json!({
                "role": m.role,
                "content": m.content,
            });
            if let Some(ref tc) = m.tool_calls {
                // Translate canonical → OpenAI wire format (#395 PR5)
                let canonical = super::canonical_format::detect_and_normalize_stored_tool_calls(tc);
                msg["tool_calls"] = super::canonical_format::canonical_to_openai_wire(&canonical);
            }
            if let Some(ref tcid) = m.tool_call_id {
                msg["tool_call_id"] = json!(tcid);
            }
            msg
        })
        .collect();
    
    let body = build_ollama_chat_body(model, msg_values, tools, max_tokens);
    if !tools.is_empty() {
        eprintln!("[OLLAMA-STREAM] Sending {} tool(s) in request", tools.len());
    }
    
    eprintln!("[OLLAMA-STREAM] Request body: {}", serde_json::to_string_pretty(&body).unwrap_or_default());
    
    let res = client
        .post(format!("{}/api/chat", endpoint))
        .json(&body)
        .send()
        .await
        .map_err(|e| {
            if e.is_timeout() {
                "Ollama did not respond within 30 seconds. Is it running?".to_string()
            } else {
                format!("Ollama request failed: {}", e)
            }
        })?;
    
    // Check HTTP status before streaming
    let status = res.status();
    if !status.is_success() {
        let error_body = res.text().await.unwrap_or_default();
        eprintln!("[OLLAMA-STREAM] HTTP {} error: {}", status, error_body);
        return Err(format!("Ollama API error ({}): {}", status, error_body));
    }
    
    eprintln!("[OLLAMA-STREAM] Connected, status: {}", status);
    
    let mut full_text = String::new();
    let mut native_tool_calls: Vec<ToolCall> = Vec::new();
    // §4.5.9 (#2874 item 60) — Ollama's cut-off signal is `done_reason` on
    // the terminal NDJSON line (the same line that carries `done: true`).
    let mut done_reason: Option<String> = None;
    let mut stream = res.bytes_stream();
    let mut buffer = String::new();
    
    let mut debug_line_count = 0;
    while let Some(chunk_result) = stream.next().await {
        let chunk = chunk_result.map_err(|e| format!("Stream error: {}", e))?;
        buffer.push_str(&String::from_utf8_lossy(&chunk));
        
        // Process complete lines (NDJSON format)
        while let Some(line_end) = buffer.find('\n') {
            let line = buffer[..line_end].trim().to_string();
            buffer = buffer[line_end + 1..].to_string();
            
            if line.is_empty() {
                continue;
            }
            
            debug_line_count += 1;
            if debug_line_count <= 3 {
                let truncated = &line[..line.floor_char_boundary(200)];
                eprintln!("[OLLAMA-STREAM] NDJSON line {}: {}", debug_line_count, truncated);
            }
            
            // Parse NDJSON (no "data: " prefix)
            if let Ok(data) = serde_json::from_str::<Value>(&line) {
                // Extract content from message.content
                if let Some(content) = data["message"]["content"].as_str() {
                    if !content.is_empty() {
                        full_text.push_str(content);
                        let _ = app.emit("ai-stream-chunk", json!({
                            "id": stream_id,
                            "text": content,
                            "done": false,
                        }));
                    }
                }
                
                // Check for done: true (final chunk)
                if data["done"].as_bool() == Some(true) {
                    eprintln!("[OLLAMA-STREAM] Stream complete (done: true)");
                    
                    // Extract usage stats from final chunk
                    let input_tokens = data["prompt_eval_count"].as_u64().unwrap_or(0) as u32;
                    let output_tokens = data["eval_count"].as_u64().unwrap_or(0) as u32;

                    // §4.5.9 (#2874 item 60) — capture done_reason from this
                    // same terminal line.
                    done_reason = data["done_reason"].as_str().map(|s| s.to_string());
                    
                    // Log usage stats
                    eprintln!(
                        "[OLLAMA-STREAM] Usage: input_tokens={}, output_tokens={}",
                        input_tokens,
                        output_tokens
                    );

                    // §4.5.8 — Parse native tool calls from the done chunk.
                    // Ollama delivers tool_calls as a complete block (not streaming deltas).
                    native_tool_calls = parse_ollama_tool_calls(&data);
                    if !native_tool_calls.is_empty() {
                        eprintln!(
                            "[OLLAMA-STREAM] Parsed {} native tool call(s)",
                            native_tool_calls.len()
                        );
                    }
                    
                    break;
                }
            } else {
                eprintln!("[OLLAMA-STREAM] Failed to parse NDJSON line: {}", line);
            }
        }
    }
    
    eprintln!("[OLLAMA-STREAM] Stream complete. full_text len: {}", full_text.len());

    let cutoff_signal = ollama_cutoff_signal(done_reason.as_deref());

    Ok((full_text, native_tool_calls, cutoff_signal))
}

/// §4.5.9 (#2874 item 60) — map Ollama's `done_reason` (terminal NDJSON
/// line) to the shared `CutoffSignal`. Ollama's documented values include
/// `"stop"` and `"length"`.
pub(crate) fn ollama_cutoff_signal(done_reason: Option<&str>) -> CutoffSignal {
    match done_reason {
        Some("length") => CutoffSignal::ConfirmedCutoff,
        Some(_) => CutoffSignal::ConfirmedNormalStop,
        None => CutoffSignal::Unavailable,
    }
}

#[cfg(test)]
mod ollama_cutoff_signal_tests {
    use super::*;

    /// §4.5.9 PR 2 test: `done_reason: "length"` confirms a cutoff.
    #[test]
    fn test_length_confirms_cutoff() {
        assert_eq!(ollama_cutoff_signal(Some("length")), CutoffSignal::ConfirmedCutoff);
    }

    #[test]
    fn test_stop_confirms_normal_stop() {
        assert_eq!(ollama_cutoff_signal(Some("stop")), CutoffSignal::ConfirmedNormalStop);
    }

    #[test]
    fn test_absent_done_reason_is_unavailable() {
        assert_eq!(ollama_cutoff_signal(None), CutoffSignal::Unavailable);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Test: probe_two_step returns unreachable for invalid endpoint
    #[tokio::test]
    async fn test_probe_two_step_unreachable() {
        // Port 19999 is highly unlikely to be in use
        let provider = OllamaProvider::new(Some("http://127.0.0.1:19999".to_string()));
        let result = provider.probe_two_step().await;
        assert!(!result.reachable, "Should be unreachable");
        assert!(result.models.is_empty(), "Models should be empty");
        assert_eq!(result.error_class, Some("unreachable".to_string()));
    }

    /// Test: OllamaProbeResult serializes to expected JSON shape
    #[test]
    fn test_probe_result_serialization() {
        let success = OllamaProbeResult {
            reachable: true,
            models: vec!["llama3.2".to_string(), "mistral".to_string()],
            error_class: None,
        };
        let json = serde_json::to_value(&success).unwrap();
        assert_eq!(json["reachable"], true);
        assert_eq!(json["models"][0], "llama3.2");
        assert_eq!(json["error_class"], serde_json::Value::Null);

        let unreachable = OllamaProbeResult {
            reachable: false,
            models: vec![],
            error_class: Some("unreachable".to_string()),
        };
        let json2 = serde_json::to_value(&unreachable).unwrap();
        assert_eq!(json2["reachable"], false);
        assert_eq!(json2["error_class"], "unreachable");
        assert_eq!(json2["models"].as_array().unwrap().len(), 0);
    }

    /// Test: Parse valid NDJSON stream
    #[test]
    fn test_parse_ndjson_stream() {
        // Simulate NDJSON chunks
        let chunks = vec![
            r#"{"model":"llama3.2","message":{"role":"assistant","content":"Hello"},"done":false}"#,
            r#"{"model":"llama3.2","message":{"role":"assistant","content":" world"},"done":false}"#,
            r#"{"model":"llama3.2","message":{"role":"assistant","content":"!"},"done":false}"#,
            r#"{"model":"llama3.2","message":{"role":"assistant","content":""},"done":true,"prompt_eval_count":26,"eval_count":290}"#,
        ];

        let mut full_text = String::new();
        let mut input_tokens = 0u32;
        let mut output_tokens = 0u32;

        for chunk in chunks {
            let data: Value = serde_json::from_str(chunk).unwrap();
            
            if let Some(content) = data["message"]["content"].as_str() {
                full_text.push_str(content);
            }
            
            if data["done"].as_bool() == Some(true) {
                input_tokens = data["prompt_eval_count"].as_u64().unwrap_or(0) as u32;
                output_tokens = data["eval_count"].as_u64().unwrap_or(0) as u32;
            }
        }

        assert_eq!(full_text, "Hello world!");
        assert_eq!(input_tokens, 26);
        assert_eq!(output_tokens, 290);
    }

    /// Test: Handle partial line buffering
    #[test]
    fn test_partial_line_buffering() {
        // Simulate NDJSON lines split across TCP chunks
        let chunk1 = r#"{"model":"llama3.2","message":{"role":"assistant","content":"Hello"},"done":false}"#;
        let chunk2 = "\n";
        let chunk3 = r#"{"model":"llama3.2","message":{"role":"assistant","content":" world"},"done":false}"#;
        let chunk4 = "\n";
        
        let mut buffer = String::new();
        let mut results = Vec::new();
        
        // Process chunks
        for chunk in [chunk1, chunk2, chunk3, chunk4] {
            buffer.push_str(chunk);
            
            // Extract complete lines
            while let Some(line_end) = buffer.find('\n') {
                let line = buffer[..line_end].trim().to_string();
                buffer = buffer[line_end + 1..].to_string();
                
                if !line.is_empty() {
                    let data: Value = serde_json::from_str(&line).unwrap();
                    if let Some(content) = data["message"]["content"].as_str() {
                        results.push(content.to_string());
                    }
                }
            }
        }
        
        assert_eq!(results, vec!["Hello", " world"]);
    }

    /// Test: Handle empty content fields
    #[test]
    fn test_empty_content_fields() {
        let chunk = r#"{"model":"llama3.2","message":{"role":"assistant","content":""},"done":true,"prompt_eval_count":10,"eval_count":0}"#;
        
        let data: Value = serde_json::from_str(chunk).unwrap();
        
        // Empty content should not panic
        let content = data["message"]["content"].as_str().unwrap_or("");
        assert_eq!(content, "");
        
        // Usage stats should still be extracted
        assert_eq!(data["prompt_eval_count"].as_u64(), Some(10));
        assert_eq!(data["eval_count"].as_u64(), Some(0));
    }

    /// Test: Extract usage stats from final chunk
    #[test]
    fn test_extract_usage_stats() {
        let final_chunk = r#"{"model":"llama3.2","message":{"role":"assistant","content":""},"done":true,"total_duration":5043500667,"load_duration":3013701500,"prompt_eval_count":26,"prompt_eval_duration":325953000,"eval_count":290,"eval_duration":1700225000}"#;
        
        let data: Value = serde_json::from_str(final_chunk).unwrap();
        
        assert_eq!(data["done"].as_bool(), Some(true));
        assert_eq!(data["prompt_eval_count"].as_u64().unwrap_or(0) as u32, 26);
        assert_eq!(data["eval_count"].as_u64().unwrap_or(0) as u32, 290);
        assert!(data["total_duration"].as_u64().is_some());
        assert!(data["eval_duration"].as_u64().is_some());
    }
    
    /// Test: NDJSON format (no SSE "data: " prefix) - §4 verification test
    #[test]
    fn test_ndjson_no_sse_prefix() {
        // Ollama uses raw NDJSON, NOT SSE format
        let ndjson_line = r#"{"model":"llama3.2","message":{"role":"assistant","content":"test"},"done":false}"#;
        let sse_line = r#"data: {"model":"llama3.2","message":{"role":"assistant","content":"test"},"done":false}"#;
        
        // NDJSON line should parse directly
        let ndjson_result = serde_json::from_str::<Value>(ndjson_line);
        assert!(ndjson_result.is_ok(), "NDJSON line should parse directly");
        
        // SSE line should NOT parse (has "data: " prefix)
        let sse_result = serde_json::from_str::<Value>(sse_line);
        assert!(sse_result.is_err(), "SSE line should fail to parse (has data: prefix)");
        
        // Verify content extraction works on NDJSON
        let data = ndjson_result.unwrap();
        assert_eq!(data["message"]["content"].as_str(), Some("test"));
    }
    
    /// Test: Robustness - malformed JSON line is skipped
    #[test]
    fn test_malformed_json_skipped() {
        let chunks = vec![
            r#"{"model":"llama3.2","message":{"role":"assistant","content":"Good"},"done":false}"#,
            r#"{this is not valid json}"#,  // Malformed line
            r#"{"model":"llama3.2","message":{"role":"assistant","content":" data"},"done":false}"#,
        ];
        
        let mut full_text = String::new();
        
        for chunk in chunks {
            // Parser should skip malformed JSON
            if let Ok(data) = serde_json::from_str::<Value>(chunk) {
                if let Some(content) = data["message"]["content"].as_str() {
                    full_text.push_str(content);
                }
            }
        }
        
        // Should have processed valid chunks only
        assert_eq!(full_text, "Good data");
    }

    // -------------------------------------------------------------------------
    // §4.5.8 — Native tool-call parsing tests (#1586)
    // -------------------------------------------------------------------------

    /// Test: parse_ollama_tool_calls — standard object-arguments case (Ollama native format)
    #[test]
    fn test_parse_ollama_tool_calls_object_args() {
        let done_chunk = json!({
            "model": "llama3.2",
            "message": {
                "role": "assistant",
                "content": "",
                "tool_calls": [
                    {
                        "function": {
                            "name": "create_note",
                            "arguments": {
                                "title": "My Note",
                                "project_id": "proj-123"
                            }
                        }
                    }
                ]
            },
            "done": true
        });

        let calls = parse_ollama_tool_calls(&done_chunk);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "create_note");
        assert_eq!(calls[0].params["title"], "My Note");
        assert_eq!(calls[0].params["project_id"], "proj-123");
        // Synthetic call_id when no id field present
        assert_eq!(calls[0].call_id, "ollama_tool_1");
    }

    /// Test: parse_ollama_tool_calls — string-encoded arguments (OpenAI-compat variant)
    #[test]
    fn test_parse_ollama_tool_calls_string_args() {
        let done_chunk = json!({
            "message": {
                "role": "assistant",
                "content": "",
                "tool_calls": [
                    {
                        "id": "call_abc123",
                        "function": {
                            "name": "search_notes",
                            "arguments": "{\"query\":\"meeting notes\"}"
                        }
                    }
                ]
            },
            "done": true
        });

        let calls = parse_ollama_tool_calls(&done_chunk);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "search_notes");
        assert_eq!(calls[0].params["query"], "meeting notes");
        // Explicit id field preserved
        assert_eq!(calls[0].call_id, "call_abc123");
        assert!(!calls[0].args_parse_failed, "valid string arguments must not be flagged as a parse failure");
    }

    /// §4.5.9 (#2874 item 60) PR 4 — string-encoded arguments that fail to
    /// parse as JSON must be flagged `args_parse_failed: true`, NOT blanked
    /// to `params: {}` and dispatched. BEFORE this PR:
    /// `.unwrap_or_else(|_| json!({}))` silently substituted `{}` and
    /// `args_parse_failed` was hardcoded `false` — the call ran with empty
    /// params and no trace. AFTER: `args_parse_failed == true`, and `name`/
    /// `call_id` both survive so `check_args_parsed` (commands.rs) can
    /// attach a refusal to this exact call on the next turn instead of
    /// letting it run.
    #[test]
    fn test_parse_ollama_tool_calls_string_args_unparseable_flagged_not_blanked() {
        let done_chunk = json!({
            "message": {
                "role": "assistant",
                "content": "",
                "tool_calls": [
                    {
                        "id": "call_bad_1",
                        "function": {
                            "name": "add_sheet_rows",
                            // Deliberately truncated/invalid JSON — simulates a
                            // cut-off or malformed OpenAI-compat string payload.
                            "arguments": "{\"note_id\": \"n1\", \"rows\": [[1, 2"
                        }
                    }
                ]
            },
            "done": true
        });

        let calls = parse_ollama_tool_calls(&done_chunk);
        assert_eq!(calls.len(), 1, "the call must still be recognized, not dropped");
        assert_eq!(calls[0].name, "add_sheet_rows", "name must survive a parse failure");
        assert_eq!(calls[0].call_id, "call_bad_1", "call_id must survive a parse failure");
        assert!(
            calls[0].args_parse_failed,
            "BEFORE: bad string arguments were blanked to params: {{}} and dispatched \
             (args_parse_failed hardcoded false); AFTER: must be flagged, not blanked"
        );
        assert_eq!(calls[0].params, json!({}));
    }

    /// Test: parse_ollama_tool_calls — multiple tool calls in one done chunk
    #[test]
    fn test_parse_ollama_tool_calls_multiple() {
        let done_chunk = json!({
            "message": {
                "tool_calls": [
                    {
                        "function": {
                            "name": "create_note",
                            "arguments": {"title": "Note A", "project_id": "p1"}
                        }
                    },
                    {
                        "function": {
                            "name": "set_status",
                            "arguments": {"note_id": "n42", "status": "done"}
                        }
                    }
                ]
            },
            "done": true
        });

        let calls = parse_ollama_tool_calls(&done_chunk);
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].name, "create_note");
        assert_eq!(calls[0].call_id, "ollama_tool_1");
        assert_eq!(calls[1].name, "set_status");
        assert_eq!(calls[1].call_id, "ollama_tool_2");
        assert_eq!(calls[1].params["status"], "done");
    }

    /// Test: parse_ollama_tool_calls — no tool_calls field returns empty vec
    #[test]
    fn test_parse_ollama_tool_calls_absent() {
        let done_chunk = json!({
            "message": {"role": "assistant", "content": "Hello!"},
            "done": true,
            "eval_count": 12
        });
        let calls = parse_ollama_tool_calls(&done_chunk);
        assert!(calls.is_empty(), "Expected empty vec when tool_calls is absent");
    }

    /// Test: parse_ollama_tool_calls — entry missing function.name is skipped
    #[test]
    fn test_parse_ollama_tool_calls_missing_name_skipped() {
        let done_chunk = json!({
            "message": {
                "tool_calls": [
                    {
                        "function": {
                            // No "name" field
                            "arguments": {"x": 1}
                        }
                    },
                    {
                        "function": {
                            "name": "navigate",
                            "arguments": {"target": "home"}
                        }
                    }
                ]
            },
            "done": true
        });
        let calls = parse_ollama_tool_calls(&done_chunk);
        assert_eq!(calls.len(), 1, "Entry with missing name should be skipped");
        assert_eq!(calls[0].name, "navigate");
    }

    /// Test: format_tools_for_ollama produces OpenAI-shaped array
    #[test]
    fn test_format_tools_for_ollama() {
        let tools = vec![
            ToolDef {
                name: "create_note".to_string(),
                description: "Create a note".to_string(),
                parameters: json!({"type": "object", "properties": {"title": {"type": "string"}}}),
                access: crate::ai::tools::ToolAccess::Write,
            },
        ];
        let formatted = format_tools_for_ollama(&tools);
        let arr = formatted.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["type"], "function");
        assert_eq!(arr[0]["function"]["name"], "create_note");
        assert_eq!(arr[0]["function"]["description"], "Create a note");
        assert!(arr[0]["function"]["parameters"].is_object());
    }

    /// Test: format_tools_for_ollama — empty slice produces empty array
    #[test]
    fn test_format_tools_for_ollama_empty() {
        let formatted = format_tools_for_ollama(&[]);
        assert_eq!(formatted.as_array().unwrap().len(), 0);
    }

    // -------------------------------------------------------------------------
    // §4.5.9 (#2874 item 60) PR 4 — build_ollama_chat_body / num_predict
    // -------------------------------------------------------------------------

    /// `build_ollama_chat_body` must send `options.num_predict` equal to the
    /// shared `CHAT_STREAM_MAX_TOKENS` constant — asserted against the
    /// constant itself, not a literal, so a future change to the constant
    /// can't silently desync this test. BEFORE this PR: `stream_ollama`'s
    /// request body had no "options" key at all — Ollama was the only
    /// provider with no output-length limit sent on the wire.
    #[test]
    fn test_build_ollama_chat_body_sends_num_predict_equal_to_chat_stream_max_tokens() {
        let max_tokens = crate::ai::chat::CHAT_STREAM_MAX_TOKENS;
        let body = build_ollama_chat_body("llama3.2", vec![], &[], max_tokens);
        assert_eq!(
            body["options"]["num_predict"], max_tokens,
            "Ollama's streaming request must carry the shared CHAT_STREAM_MAX_TOKENS \
             output limit as options.num_predict, not go unbounded"
        );
    }

    /// Sanity: the rest of the body shape (model/messages/stream) is
    /// unaffected by adding `options`.
    #[test]
    fn test_build_ollama_chat_body_preserves_model_messages_stream() {
        let max_tokens = crate::ai::chat::CHAT_STREAM_MAX_TOKENS;
        let msg = json!({"role": "user", "content": "hi"});
        let body = build_ollama_chat_body("llama3.2", vec![msg.clone()], &[], max_tokens);
        assert_eq!(body["model"], "llama3.2");
        assert_eq!(body["messages"], json!([msg]));
        assert_eq!(body["stream"], true);
    }

    /// Tools, when present, are still embedded in the body alongside `options`.
    #[test]
    fn test_build_ollama_chat_body_includes_tools_when_present() {
        let tools = vec![ToolDef {
            name: "create_note".to_string(),
            description: "Create a note".to_string(),
            parameters: json!({"type": "object"}),
            access: crate::ai::tools::ToolAccess::Write,
        }];
        let max_tokens = crate::ai::chat::CHAT_STREAM_MAX_TOKENS;
        let body = build_ollama_chat_body("llama3.2", vec![], &tools, max_tokens);
        assert_eq!(body["tools"].as_array().unwrap().len(), 1);
        assert_eq!(body["options"]["num_predict"], max_tokens);
    }

    /// No tools → no `tools` key at all (matches the pre-existing
    /// `if !tools.is_empty()` gate's behavior, unchanged by this PR).
    #[test]
    fn test_build_ollama_chat_body_omits_tools_key_when_empty() {
        let max_tokens = crate::ai::chat::CHAT_STREAM_MAX_TOKENS;
        let body = build_ollama_chat_body("llama3.2", vec![], &[], max_tokens);
        assert!(body.get("tools").is_none());
    }

    /// Test: Ollama message serialization includes canonical tool_calls as OpenAI wire (#395 PR5, W-2).
    ///
    /// Verifies that a ChatMessage with canonical tool_calls produces an OpenAI-compatible
    /// `tool_calls` array in the serialized message body — not silently dropped.
    #[test]
    fn test_ollama_msg_serialization_includes_tool_calls() {
        use serde_json::json;

        // Canonical tool_calls stored in the message (as written by tool_calls_to_canonical)
        let canonical_tc = json!([
            {"id": "call_abc", "name": "create_note", "params": {"title": "Hello"}}
        ]);

        let msg = ChatMessage {
            role: "assistant".to_string(),
            content: "".to_string(),
            tool_calls: Some(canonical_tc.clone()),
            tool_call_id: None,
            content_blocks: None,
            attachment_ids: None,
            id: None,
        };

        // Replicate the stream_ollama serialization logic
        let mut serialized = json!({
            "role": msg.role,
            "content": msg.content,
        });
        if let Some(ref tc) = msg.tool_calls {
            let canonical = crate::ai::canonical_format::detect_and_normalize_stored_tool_calls(tc);
            serialized["tool_calls"] = crate::ai::canonical_format::canonical_to_openai_wire(&canonical);
        }
        if let Some(ref tcid) = msg.tool_call_id {
            serialized["tool_call_id"] = json!(tcid);
        }

        // Must include tool_calls in OpenAI wire format
        let tcs = serialized["tool_calls"].as_array().expect("tool_calls must be present");
        assert_eq!(tcs.len(), 1, "tool_calls must not be dropped");
        assert_eq!(tcs[0]["id"], "call_abc");
        assert_eq!(tcs[0]["type"], "function");
        assert_eq!(tcs[0]["function"]["name"], "create_note");
        // arguments must be a valid JSON string
        let args_str = tcs[0]["function"]["arguments"].as_str().expect("arguments must be a string");
        let args: serde_json::Value = serde_json::from_str(args_str).expect("arguments must parse as JSON");
        assert_eq!(args["title"], "Hello", "tool params must survive canonical -> OpenAI wire roundtrip");
    }

    /// Test: Ollama message with tool_call_id (tool result message) serializes correctly.
    #[test]
    fn test_ollama_msg_serialization_tool_call_id() {
        use serde_json::json;

        let msg = ChatMessage {
            role: "tool".to_string(),
            content: "Created note successfully".to_string(),
            tool_calls: None,
            tool_call_id: Some("call_abc".to_string()),
            content_blocks: None,
            attachment_ids: None,
            id: None,
        };

        let mut serialized = json!({
            "role": msg.role,
            "content": msg.content,
        });
        if let Some(ref tc) = msg.tool_calls {
            let canonical = crate::ai::canonical_format::detect_and_normalize_stored_tool_calls(tc);
            serialized["tool_calls"] = crate::ai::canonical_format::canonical_to_openai_wire(&canonical);
        }
        if let Some(ref tcid) = msg.tool_call_id {
            serialized["tool_call_id"] = json!(tcid);
        }

        assert_eq!(serialized["role"], "tool");
        assert_eq!(serialized["content"], "Created note successfully");
        assert_eq!(serialized["tool_call_id"], "call_abc");
        assert!(serialized.get("tool_calls").is_none() || serialized["tool_calls"].as_array().map(|a| a.is_empty()).unwrap_or(false),
            "tool result message must not have tool_calls");
    }
}
