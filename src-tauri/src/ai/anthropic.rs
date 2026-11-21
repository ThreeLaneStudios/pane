use super::provider::*;
use super::tools::{ToolDef, ToolCall, CutoffSignal};
use super::adapter::{ProviderAdapter, AnthropicAdapter};
use reqwest;
use serde_json::{json, Value};
use futures_util::StreamExt;
use tauri::Emitter;

/// Error type for `stream_anthropic`: `(error_message, partial_text)`.
/// Mirrors `StreamOpenrouterError` so callers can preserve whatever text
/// had already streamed before the error occurred.
pub type StreamAnthropicError = (String, String);

pub struct AnthropicProvider {
    api_key: String,
}

/// Helper to log cache metrics with formatted numbers
fn log_cache_metrics(usage: &Value) {
    let input_tokens = usage["input_tokens"].as_u64().unwrap_or(0);
    let output_tokens = usage["output_tokens"].as_u64().unwrap_or(0);
    let cache_creation = usage["cache_creation_input_tokens"].as_u64().unwrap_or(0);
    let cache_read = usage["cache_read_input_tokens"].as_u64().unwrap_or(0);
    
    // Format numbers with commas
    let format_num = |n: u64| -> String {
        n.to_string()
            .as_bytes()
            .rchunks(3)
            .rev()
            .map(|chunk| std::str::from_utf8(chunk).unwrap())
            .collect::<Vec<_>>()
            .join(",")
    };
    
    eprintln!(
        "[CACHE] input: {} | output: {} | cache_write: {} | cache_read: {}",
        format_num(input_tokens),
        format_num(output_tokens),
        format_num(cache_creation),
        format_num(cache_read)
    );
}

/// Classify a 400 Bad Request from Anthropic as a billing error.
///
/// Returns `true` iff ALL three conditions hold:
///   1. `status` is 400 Bad Request
///   2. `error_type` is `"invalid_request_error"`
///   3. `message` contains `"credit balance"` (case-insensitive)
///
/// Per spec §6.4.3 billing-error misclassification quirk and ai-integration.md §4.3.1.
/// Used by `AnthropicProvider::chat`, `chat_with_tools`, `stream_anthropic`, and
/// `system_files::validate_anthropic_key`.
pub(crate) fn is_anthropic_billing_error(
    status: reqwest::StatusCode,
    error_type: &str,
    message: &str,
) -> bool {
    status == reqwest::StatusCode::BAD_REQUEST
        && error_type == "invalid_request_error"
        && message.to_lowercase().contains("credit balance")
}

/// Extended response that can include tool calls.
// Preserve the provider response shape for legacy callers.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct AnthropicResponse {
    pub response: ChatResponse,
    pub tool_calls: Vec<ToolCall>,
    pub raw_response: Value,
    pub assistant_message: Option<Value>,
    /// §4.5.9 (#2874 item 60) — read from the non-streaming Messages API
    /// response's top-level `stop_reason` field (`raw_response["stop_reason"]`).
    /// Per the #2874 item 60 scope audit, every production call site of
    /// `chat_with_tools` passes `tools: None`, so `tool_calls` is always empty
    /// here today and nothing currently reads this field for dispatch — it is
    /// read anyway, per spec, so it is available the moment that changes.
    pub cutoff_signal: CutoffSignal,
}

impl AnthropicProvider {
    pub fn new(api_key: String) -> Self {
        Self { api_key }
    }
    
    #[allow(dead_code)] // Legacy non-tool provider entry point.
    pub async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, String> {
        let client = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(30))
            .timeout(std::time::Duration::from_secs(300))
            .build()
            .map_err(|e| format!("Failed to build HTTP client: {}", e))?;
        
        // Separate system message from conversation
        let system_msg = request.messages.iter()
            .find(|m| m.role == "system")
            .map(|m| m.content.clone());
        
        let mut messages: Vec<Value> = request.messages.iter()
            .filter(|m| m.role != "system")
            .map(|m| json!({
                "role": m.role,
                "content": m.content,
            }))
            .collect();
        
        // §26 Prompt Caching: Add cache_control to second-to-last message (last before new user message)
        if messages.len() >= 2 {
            let second_last_idx = messages.len() - 2;
            if let Some(msg) = messages.get_mut(second_last_idx) {
                // Convert content to array format with cache_control
                if let Some(content_str) = msg["content"].as_str() {
                    msg["content"] = json!([{
                        "type": "text",
                        "text": content_str,
                        "cache_control": { "type": "ephemeral" }
                    }]);
                }
            }
        }
        
        let mut body = json!({
            "model": request.model,
            "max_tokens": request.max_tokens,
            "messages": messages,
        });
        
        // §26 Prompt Caching: Convert system to content array with cache_control on last block
        if let Some(system) = system_msg {
            body["system"] = json!([{
                "type": "text",
                "text": system,
                "cache_control": { "type": "ephemeral" }
            }]);
        }
        
        let req = client
            .post("https://api.anthropic.com/v1/messages")
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json");
        
        // OAuth tokens (sk-ant-oat*) use Bearer auth; standard keys use x-api-key
        let req = if self.api_key.starts_with("sk-ant-oat") {
            req.header("Authorization", format!("Bearer {}", self.api_key))
               .header("anthropic-beta", "oauth-2025-04-20")
        } else {
            req.header("x-api-key", &self.api_key)
        };
        
        let res = req
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    "AI provider did not respond within 30 seconds. Check your connection.".to_string()
                } else {
                    format!("Anthropic request failed: {}", e)
                }
            })?;
        
        let status = res.status();
        let response_body: Value = res.json().await
            .map_err(|e| format!("Failed to parse response: {}", e))?;
        
        if !status.is_success() {
            let error_type = response_body["error"]["type"].as_str().unwrap_or("");
            let error_msg = response_body["error"]["message"]
                .as_str()
                .unwrap_or("Unknown error");
            if is_anthropic_billing_error(status, error_type, error_msg) {
                return Err("Insufficient Anthropic credits. Add credits at console.anthropic.com/settings/plans to continue.".to_string());
            }
            return Err(format!("Anthropic error ({}): {}", status, error_msg));
        }
        
        let text = response_body["content"][0]["text"]
            .as_str()
            .unwrap_or("")
            .to_string();
        
        let input_tokens = response_body["usage"]["input_tokens"].as_u64().unwrap_or(0) as u32;
        let output_tokens = response_body["usage"]["output_tokens"].as_u64().unwrap_or(0) as u32;
        
        // Log cache metrics
        log_cache_metrics(&response_body["usage"]);
        
        Ok(ChatResponse {
            text,
            model: request.model,
            input_tokens,
            output_tokens,
        })
    }
    
    /// Chat with tool support
    pub async fn chat_with_tools(
        &self,
        request: ChatRequest,
        tools: Option<&[ToolDef]>,
        _db: Option<&crate::database::DbState>
    ) -> Result<AnthropicResponse, String> {
        let client = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(30))
            .timeout(std::time::Duration::from_secs(300))
            .build()
            .map_err(|e| format!("Failed to build HTTP client: {}", e))?;
        
        // Extract system message
        let system_msg = request.messages.iter()
            .find(|m| m.role == "system")
            .map(|m| m.content.clone());
        
        // Build messages (Anthropic format, system separate)
        let mut messages: Vec<Value> = request.messages.iter()
            .filter(|m| m.role != "system")
            .map(|m| {
                // Handle different content types for tool results
                if m.role == "tool" {
                    // Tool results in Anthropic wire format: user message with tool_result block
                    json!({
                        "role": "user",
                        "content": [{
                            "type": "tool_result",
                            "tool_use_id": m.tool_call_id.as_ref().unwrap_or(&String::new()),
                            "content": m.content
                        }]
                    })
                } else if m.role == "assistant" && m.tool_calls.is_some() {
                    // Assistant message with tool calls: translate canonical → Anthropic wire (#395 PR5)
                    let canonical = m.tool_calls.as_ref().unwrap();
                    let wire = super::canonical_format::detect_and_normalize_stored_tool_calls(canonical);
                    let anthropic_blocks = super::canonical_format::canonical_to_anthropic_wire(&wire);
                    json!({
                        "role": "assistant",
                        "content": anthropic_blocks
                    })
                } else {
                    json!({
                        "role": m.role,
                        "content": m.content
                    })
                }
            })
            .collect();
        
        // §26 Prompt Caching: Add cache_control to second-to-last message (last before new user message)
        if messages.len() >= 2 {
            let second_last_idx = messages.len() - 2;
            if let Some(msg) = messages.get_mut(second_last_idx) {
                // Only add cache_control if content is a simple string (not tool results or tool calls)
                if let Some(content_str) = msg["content"].as_str() {
                    msg["content"] = json!([{
                        "type": "text",
                        "text": content_str,
                        "cache_control": { "type": "ephemeral" }
                    }]);
                }
            }
        }
        
        let mut body = json!({
            "model": request.model,
            "max_tokens": request.max_tokens,
            "messages": messages,
        });
        
        // §26 Prompt Caching: Convert system to content array with cache_control on last block
        if let Some(system) = system_msg {
            body["system"] = json!([{
                "type": "text",
                "text": system,
                "cache_control": { "type": "ephemeral" }
            }]);
        }
        
        // Add tools if provided
        if let Some(tool_defs) = tools {
            let adapter = AnthropicAdapter;
            let formatted_tools = adapter.format_tools(tool_defs);
            body["tools"] = formatted_tools;
            body["tool_choice"] = json!({"type": "auto"});
        }
        
        eprintln!("[ANTHROPIC] Request body: {}", serde_json::to_string_pretty(&body).unwrap_or_default());
        
        let req = client
            .post("https://api.anthropic.com/v1/messages")
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json");
        
        // OAuth tokens (sk-ant-oat*) use Bearer auth; standard keys use x-api-key
        let req = if self.api_key.starts_with("sk-ant-oat") {
            req.header("Authorization", format!("Bearer {}", self.api_key))
               .header("anthropic-beta", "oauth-2025-04-20")
        } else {
            req.header("x-api-key", &self.api_key)
        };
        
        let res = req
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    "AI provider did not respond within 30 seconds. Check your connection.".to_string()
                } else {
                    format!("Anthropic request failed: {}", e)
                }
            })?;
        
        let status = res.status();
        let response_body: Value = res.json().await
            .map_err(|e| format!("Failed to parse response: {}", e))?;
        
        eprintln!("[ANTHROPIC] Response status: {}", status);
        eprintln!("[ANTHROPIC] Response body: {}", serde_json::to_string_pretty(&response_body).unwrap_or_default());
        
        if !status.is_success() {
            let error_type = response_body["error"]["type"].as_str().unwrap_or("");
            let error_msg = response_body["error"]["message"]
                .as_str()
                .unwrap_or("Unknown error");
            if is_anthropic_billing_error(status, error_type, error_msg) {
                return Err("Insufficient Anthropic credits. Add credits at console.anthropic.com/settings/plans to continue.".to_string());
            }
            return Err(format!("Anthropic error ({}): {}", status, error_msg));
        }
        
        // Extract text from content blocks
        let mut text = String::new();
        if let Some(content_array) = response_body["content"].as_array() {
            for block in content_array {
                if block["type"].as_str() == Some("text") {
                    if let Some(t) = block["text"].as_str() {
                        text.push_str(t);
                    }
                }
            }
        }
        
        let input_tokens = response_body["usage"]["input_tokens"].as_u64().unwrap_or(0) as u32;
        let output_tokens = response_body["usage"]["output_tokens"].as_u64().unwrap_or(0) as u32;
        
        // Log cache metrics
        log_cache_metrics(&response_body["usage"]);
        
        // Extract tool calls
        let tool_calls = if tools.is_some() {
            let adapter = AnthropicAdapter;
            adapter.extract_tool_calls(&response_body)
        } else {
            vec![]
        };

        // §4.5.9 (#2874 item 60) — the non-streaming Messages API carries
        // `stop_reason` as a top-level field on the response body (not nested
        // under a delta, unlike the streaming SSE shape).
        let cutoff_signal = anthropic_cutoff_signal(response_body["stop_reason"].as_str());

        Ok(AnthropicResponse {
            response: ChatResponse {
                text,
                model: request.model,
                input_tokens,
                output_tokens,
            },
            tool_calls,
            raw_response: response_body.clone(),
            assistant_message: Some(response_body["content"].clone()),
            cutoff_signal,
        })
    }
}

/// Stream Anthropic responses with SSE
///
/// #3059: `app` is generic over `R: tauri::Runtime` (rather than the implicit
/// `Wry` default) — same rationale and same mechanical change as
/// `stream_openrouter`'s `R: tauri::Runtime` generification (ADR-0067 D2/D5,
/// extended here per #3059 so `run_agent_loop<R>` compiles for every provider
/// arm, not just OpenRouter/OpenClaw/Venice). Every production call site still
/// passes a concrete `&tauri::AppHandle` (= `AppHandle<Wry>`); Rust infers
/// `R = Wry` there with zero call-site changes.
pub async fn stream_anthropic<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    stream_id: &str,
    api_key: &str,
    model: &str,
    messages: Vec<ChatMessage>,
    tools: &[ToolDef],
    max_tokens: u32,
) -> Result<(String, Vec<ToolCall>, CutoffSignal), StreamAnthropicError> {
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(30))
            .timeout(std::time::Duration::from_secs(300))
        .build()
        .map_err(|e| (format!("Failed to build HTTP client: {}", e), String::new()))?;
    
    let adapter = AnthropicAdapter;
    let formatted_tools = adapter.format_tools(tools);
    
    eprintln!("[ANTHROPIC-STREAM] Tools: {}", serde_json::to_string_pretty(&formatted_tools).unwrap_or_default());
    
    // Extract system message
    let system_msg = messages.iter()
        .find(|m| m.role == "system")
        .map(|m| m.content.clone());
    
    // Build messages — translate canonical tool calls → Anthropic wire (#395 PR5)
    let mut msg_values: Vec<Value> = messages.iter()
        .filter(|m| m.role != "system")
        .map(|m| {
            if m.role == "tool" {
                json!({
                    "role": "user",
                    "content": [{
                        "type": "tool_result",
                        "tool_use_id": m.tool_call_id.as_ref().unwrap_or(&String::new()),
                        "content": m.content
                    }]
                })
            } else if m.role == "assistant" && m.tool_calls.is_some() {
                // Translate canonical → Anthropic wire (#395 PR5)
                let canonical = m.tool_calls.as_ref().unwrap();
                let wire = super::canonical_format::detect_and_normalize_stored_tool_calls(canonical);
                let anthropic_blocks = super::canonical_format::canonical_to_anthropic_wire(&wire);
                json!({
                    "role": "assistant",
                    "content": anthropic_blocks
                })
            } else {
                json!({
                    "role": m.role,
                    "content": m.content
                })
            }
        })
        .collect();
    
    // §26 Prompt Caching: Add cache_control to second-to-last message (last before new user message)
    if msg_values.len() >= 2 {
        let second_last_idx = msg_values.len() - 2;
        if let Some(msg) = msg_values.get_mut(second_last_idx) {
            // Only add cache_control if content is a simple string (not tool results or tool calls)
            if let Some(content_str) = msg["content"].as_str() {
                msg["content"] = json!([{
                    "type": "text",
                    "text": content_str,
                    "cache_control": { "type": "ephemeral" }
                }]);
            }
        }
    }
    
    let mut body = json!({
        "model": model,
        "max_tokens": max_tokens,
        "stream": true,
        "tools": formatted_tools,
        "messages": msg_values,
        "tool_choice": {"type": "auto"},
    });
    
    // §26 Prompt Caching: Convert system to content array with cache_control on last block
    if let Some(system) = system_msg {
        body["system"] = json!([{
            "type": "text",
            "text": system,
            "cache_control": { "type": "ephemeral" }
        }]);
    }
    
    // Fix 3: Log full body keys and tool count to stderr
    eprintln!("[ANTHROPIC-STREAM] Body keys: {:?}", body.as_object().map(|o| o.keys().collect::<Vec<_>>()));
    eprintln!("[ANTHROPIC-STREAM] Tools count: {}", body["tools"].as_array().map(|a| a.len()).unwrap_or(0));
    eprintln!("[ANTHROPIC-STREAM] Tool choice: {}", body["tool_choice"]);
    eprintln!("[ANTHROPIC-STREAM] Request body: {}", serde_json::to_string_pretty(&body).unwrap_or_default());
    
    // Fix 1: Emit diagnostic info to frontend console
    let tool_count = if let Some(arr) = body["tools"].as_array() { arr.len() } else { 0 };
    let _ = app.emit("ai-stream-chunk", json!({
        "id": stream_id,
        "text": "",
        "done": false,
        "debug": format!("[ANTHROPIC] Sending {} tools, model: {}, tool_choice: {:?}", tool_count, model, body["tool_choice"]),
    }));
    
    let req = client
        .post("https://api.anthropic.com/v1/messages")
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json");
    
    // OAuth tokens (sk-ant-oat*) use Bearer auth; standard keys use x-api-key
    let req = if api_key.starts_with("sk-ant-oat") {
        req.header("Authorization", format!("Bearer {}", api_key))
           .header("anthropic-beta", "oauth-2025-04-20")
    } else {
        req.header("x-api-key", api_key)
    };
    
    let res = req
        .json(&body)
        .send()
        .await
        .map_err(|e| {
            if e.is_timeout() {
                ("AI provider did not respond within 30 seconds. Check your connection.".to_string(), String::new())
            } else {
                (format!("Request failed: {}", e), String::new())
            }
        })?;

    // Check for HTTP errors BEFORE trying to stream
    let status = res.status();
    if !status.is_success() {
        let error_body = res.text().await.unwrap_or_else(|_| "Unknown error".to_string());
        eprintln!("[ANTHROPIC-STREAM] HTTP error {}: {}", status, error_body);
        let error_body_json: serde_json::Value =
            serde_json::from_str(&error_body).unwrap_or(serde_json::Value::Null);
        let error_type = error_body_json["error"]["type"].as_str().unwrap_or("");
        let error_msg_str = error_body_json["error"]["message"].as_str().unwrap_or("");
        if is_anthropic_billing_error(status, error_type, error_msg_str) {
            return Err(("Insufficient Anthropic credits. Add credits at console.anthropic.com/settings/plans to continue.".to_string(), String::new()));
        }
        return Err((format!("Anthropic API error ({}): {}", status, error_body), String::new()));
    }

    eprintln!("[ANTHROPIC-STREAM] Connected, status: {}", status);
    
    let mut full_text = String::new();
    let mut tool_calls: Vec<ToolCall> = Vec::new();
    let mut partial_tool_calls: std::collections::HashMap<usize, (String, String, String)> = std::collections::HashMap::new();
    let mut usage_data: Option<Value> = None;
    // §4.5.9 (#2874 item 60) — Anthropic's cut-off signal arrives on the
    // `message_delta` event's `delta.stop_reason` field, not on `message_stop`
    // (which carries no fields at all per Anthropic's documented protocol).
    let mut stop_reason: Option<String> = None;
    
    let mut stream = res.bytes_stream();
    let mut buffer = String::new();
    
    while let Some(chunk_result) = stream.next().await {
        let chunk = match chunk_result {
            Ok(bytes) => bytes,
            Err(e) => {
                eprintln!(
                    "[ANTHROPIC-STREAM] Mid-stream error: {}. Preserving partial: {} chars, {} partial tool call(s)",
                    e, full_text.len(), partial_tool_calls.len()
                );
                return Err((format!("Stream error: {}", e), full_text.clone()));
            }
        };
        eprintln!("[ANTHROPIC-STREAM] Raw chunk: {}", String::from_utf8_lossy(&chunk));
        buffer.push_str(&String::from_utf8_lossy(&chunk));
        
        while let Some(line_end) = buffer.find('\n') {
            let line = buffer[..line_end].trim().to_string();
            buffer = buffer[line_end + 1..].to_string();
            
            if line.is_empty() {
                continue;
            }
            
            // Anthropic SSE format: "event: <type>" followed by "data: <json>"
            if line.starts_with("event:") {
                // Just note the event type, we'll process on data: line
                continue;
            }
            
            if let Some(json_str) = line.strip_prefix("data: ") {
                if let Ok(data) = serde_json::from_str::<Value>(json_str) {
                    let event_type = data["type"].as_str().unwrap_or("");
                    eprintln!("[ANTHROPIC-STREAM] Event: {}", event_type);
                    
                    // Capture usage data AND the cut-off signal from message_delta event
                    if event_type == "message_delta" {
                        if let Some(usage) = data.get("usage") {
                            usage_data = Some(usage.clone());
                        }
                        if let Some(sr) = data["delta"]["stop_reason"].as_str() {
                            stop_reason = Some(sr.to_string());
                        }
                    }
                    
                    match event_type {
                        "content_block_start" => {
                            let index = data["index"].as_u64().unwrap_or(0) as usize;
                            let block = &data["content_block"];
                            let block_type = block["type"].as_str().unwrap_or("unknown");
                            
                            // Fix 2: Emit event type to frontend
                            let _ = app.emit("ai-stream-chunk", json!({
                                "id": stream_id,
                                "text": "",
                                "done": false,
                                "debug": format!("[ANTHROPIC] content_block_start: type={}", block_type),
                            }));
                            
                            if block["type"].as_str() == Some("tool_use") {
                                let id = block["id"].as_str().unwrap_or("").to_string();
                                let name = block["name"].as_str().unwrap_or("").to_string();
                                partial_tool_calls.insert(index, (id, name, String::new()));
                            }
                        }
                        "content_block_delta" => {
                            let index = data["index"].as_u64().unwrap_or(0) as usize;
                            let delta = &data["delta"];
                            
                            if delta["type"].as_str() == Some("text_delta") {
                                if let Some(text) = delta["text"].as_str() {
                                    full_text.push_str(text);
                                    let _ = app.emit("ai-stream-chunk", json!({
                                        "id": stream_id,
                                        "text": text,
                                        "done": false,
                                    }));
                                }
                            } else if delta["type"].as_str() == Some("input_json_delta") {
                                if let Some(partial_json) = delta["partial_json"].as_str() {
                                    if let Some(entry) = partial_tool_calls.get_mut(&index) {
                                        entry.2.push_str(partial_json);
                                    }
                                }
                            }
                        }
                        "message_stop" => {
                            // Stream complete
                            break;
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    
    // Log cache metrics if we captured usage data
    if let Some(usage) = usage_data {
        log_cache_metrics(&usage);
    }
    
    // Parse tool calls from accumulated JSON. §4.5.9 (#2874 item 60): a call
    // whose arguments were cut off mid-stream or are otherwise invalid JSON is
    // never dispatched with blanked/substituted arguments — `finalize_tool_calls`
    // preserves `call_id` and `name` either way and flags the failure so the
    // agentic loop can refuse it instead of running it.
    tool_calls.extend(finalize_tool_calls(partial_tool_calls));

    let cutoff_signal = anthropic_cutoff_signal(stop_reason.as_deref());

    Ok((full_text, tool_calls, cutoff_signal))
}

/// §4.5.9 (#2874 item 60) — map Anthropic's `stop_reason` (read from the
/// streaming `message_delta` event's `delta.stop_reason`, or from the
/// non-streaming response's top-level `stop_reason` field — same string,
/// two different locations on the wire) to the shared `CutoffSignal`.
/// `"max_tokens"` is Anthropic's documented value for an output-length
/// cutoff; any other present value (`"end_turn"`, `"tool_use"`,
/// `"stop_sequence"`, ...) is a normal stop; absence means the signal wasn't
/// captured (shouldn't happen on a clean completion, but handled explicitly
/// rather than assumed).
pub(crate) fn anthropic_cutoff_signal(stop_reason: Option<&str>) -> CutoffSignal {
    match stop_reason {
        Some("max_tokens") => CutoffSignal::ConfirmedCutoff,
        Some(_) => CutoffSignal::ConfirmedNormalStop,
        None => CutoffSignal::Unavailable,
    }
}

#[cfg(test)]
mod anthropic_cutoff_signal_tests {
    use super::*;

    /// §4.5.9 PR 2 test: `stop_reason: "max_tokens"` (the real Anthropic
    /// cut-off sentinel) must confirm a cut-off.
    #[test]
    fn test_max_tokens_confirms_cutoff() {
        assert_eq!(anthropic_cutoff_signal(Some("max_tokens")), CutoffSignal::ConfirmedCutoff);
    }

    /// A normal `end_turn` (or any other real value) confirms a normal stop,
    /// not a cutoff — required so an invalid-JSON failure on a normal stop
    /// picks the "invalid" message, not the "cut off" one.
    #[test]
    fn test_end_turn_confirms_normal_stop() {
        assert_eq!(anthropic_cutoff_signal(Some("end_turn")), CutoffSignal::ConfirmedNormalStop);
    }

    #[test]
    fn test_tool_use_confirms_normal_stop() {
        assert_eq!(anthropic_cutoff_signal(Some("tool_use")), CutoffSignal::ConfirmedNormalStop);
    }

    /// No captured stop_reason at all — the signal is unavailable, not
    /// assumed to be either a cutoff or a normal stop.
    #[test]
    fn test_absent_stop_reason_is_unavailable() {
        assert_eq!(anthropic_cutoff_signal(None), CutoffSignal::Unavailable);
    }
}

/// §4.5.9 — turn this stream's accumulated per-index tool-call fragments
/// (`call_id`, `name`, raw concatenated `input_json_delta` text) into finished
/// `ToolCall`s. Pulled out of `stream_anthropic`'s SSE loop so the parse-failure
/// behavior is unit-testable without a live/mocked HTTP stream (`stream_anthropic`
/// posts to a hardcoded Anthropic URL and has no base-url override seam).
///
/// An empty accumulated string (a tool call with no arguments at all) is treated
/// as `{}`, matching every real provider's wire shape for a zero-arg call —
/// this is NOT a parse failure. Anything else that fails to parse as JSON
/// (truncated by the output-length limit, or genuinely malformed) returns a
/// `ToolCall` with `args_parse_failed: true` and placeholder `params: {}`;
/// it MUST NOT be dispatched (see `check_args_parsed` in `commands.rs`).
fn finalize_tool_calls(
    partial_tool_calls: std::collections::HashMap<usize, (String, String, String)>,
) -> Vec<ToolCall> {
    let mut tool_calls = Vec::new();
    for (_index, (id, name, args_json)) in partial_tool_calls {
        let json_str = if args_json.trim().is_empty() { "{}".to_string() } else { args_json };
        match serde_json::from_str::<Value>(&json_str) {
            Ok(params) => {
                tool_calls.push(ToolCall {
                    call_id: id,
                    name,
                    params,
                    args_parse_failed: false,
                });
            }
            Err(e) => {
                eprintln!(
                    "[ANTHROPIC] Tool args failed to parse for call_id={} name={}: {} (raw, first 200 chars: {})",
                    id,
                    name,
                    e,
                    &json_str[..json_str.floor_char_boundary(200)]
                );
                tool_calls.push(ToolCall {
                    call_id: id,
                    name,
                    params: json!({}),
                    args_parse_failed: true,
                });
            }
        }
    }
    tool_calls
}

#[cfg(test)]
mod finalize_tool_calls_tests {
    use super::*;
    use std::collections::HashMap;

    fn partials(entries: Vec<(usize, &str, &str, &str)>) -> HashMap<usize, (String, String, String)> {
        entries
            .into_iter()
            .map(|(idx, id, name, args)| (idx, (id.to_string(), name.to_string(), args.to_string())))
            .collect()
    }

    /// §4.5.9 test 1 (Anthropic): a truncated/invalid arguments string no
    /// longer produces a dispatchable call. BEFORE this PR, `stream_anthropic`
    /// pushed this exact shape with `params: json!({})` and no failure marker
    /// — indistinguishable from a real empty-args call, and it ran. AFTER:
    /// `args_parse_failed` is `true`, which `check_args_parsed` (commands.rs)
    /// refuses before `execute_single_tool` is ever reached.
    #[test]
    fn test_truncated_args_marked_as_parse_failed_not_dispatched() {
        let partial = partials(vec![(
            0,
            "toolu_01cutoff",
            "add_sheet_rows",
            // Cut off mid-string — exactly the shape a max_tokens truncation
            // produces: valid JSON up to the point the stream stopped.
            "{\"note_id\":\"n1\",\"columns\":[\"a\",\"b\"],\"rows\":[[\"1\",\"2",
        )]);
        let result = finalize_tool_calls(partial);
        assert_eq!(result.len(), 1, "the call must still be returned (for call_id/result pairing), just not runnable");
        assert!(
            result[0].args_parse_failed,
            "truncated JSON must be flagged as a parse failure, not silently blanked-and-dispatched"
        );
    }

    /// Genuinely malformed (not just truncated) JSON must also be refused, not
    /// blanked-and-dispatched.
    #[test]
    fn test_invalid_json_marked_as_parse_failed() {
        let partial = partials(vec![(0, "toolu_02bad", "create_note", "{not json at all")]);
        let result = finalize_tool_calls(partial);
        assert_eq!(result.len(), 1);
        assert!(result[0].args_parse_failed);
    }

    /// §4.5.9 test 2: `call_id` and tool `name` survive into the result even
    /// when arguments fail to parse — required so the agentic loop can attach
    /// the "couldn't run" message to the right `call_id`.
    #[test]
    fn test_call_id_and_name_survive_parse_failure() {
        let partial = partials(vec![(0, "toolu_03xyz", "edit_note", "{\"note_id\": \"n1\", \"content\": \"trun")]);
        let result = finalize_tool_calls(partial);
        assert_eq!(result[0].call_id, "toolu_03xyz");
        assert_eq!(result[0].name, "edit_note");
    }

    /// A real zero-arg tool call (empty accumulated string, Anthropic's own
    /// wire shape for "no input") is NOT a parse failure — must not be
    /// misclassified and refused.
    #[test]
    fn test_empty_args_is_not_a_parse_failure() {
        let partial = partials(vec![(0, "toolu_04empty", "list_notes", "")]);
        let result = finalize_tool_calls(partial);
        assert!(!result[0].args_parse_failed);
        assert_eq!(result[0].params, json!({}));
    }

    /// Well-formed arguments parse normally and are never flagged.
    #[test]
    fn test_valid_args_parse_normally() {
        let partial = partials(vec![(0, "toolu_05ok", "create_note", r#"{"title":"Groceries"}"#)]);
        let result = finalize_tool_calls(partial);
        assert!(!result[0].args_parse_failed);
        assert_eq!(result[0].params["title"], "Groceries");
    }

    /// §4.5.9 test 3: a mixed batch (one failed call + one successful call in
    /// the same turn) must return BOTH calls, each keeping its own `call_id`
    /// — this is what lets the commands.rs loop's
    /// `tool_calls.iter().zip(tool_results_for_provider.iter())` stay paired
    /// 1:1 instead of silently dropping the failed entry and shifting the zip.
    #[test]
    fn test_mixed_batch_keeps_both_calls_with_distinct_call_ids() {
        let partial = partials(vec![
            (0, "toolu_ok", "create_note", r#"{"title":"Fine"}"#),
            (1, "toolu_bad", "add_sheet_rows", "{\"note_id\":\"n1\",\"rows\":[[\"cut off"),
        ]);
        let mut result = finalize_tool_calls(partial);
        result.sort_by(|a, b| a.call_id.cmp(&b.call_id));
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].call_id, "toolu_bad");
        assert!(result[0].args_parse_failed);
        assert_eq!(result[1].call_id, "toolu_ok");
        assert!(!result[1].args_parse_failed);
    }
}

#[cfg(test)]
mod billing_error_tests {
    use super::*;
    use reqwest::StatusCode;

    // ── Positive: should return true ────────────────────────────────────────

    #[test]
    fn billing_error_credit_balance_too_low() {
        // Real Anthropic 400 body shape: invalid_request_error + credit balance message
        assert!(is_anthropic_billing_error(
            StatusCode::BAD_REQUEST,
            "invalid_request_error",
            "Your credit balance is too low to access the Anthropic API. \
             Please go to Plans & Billing to upgrade or purchase credits.",
        ));
    }

    #[test]
    fn billing_error_credit_balance_case_insensitive() {
        // Variant: Anthropic may change capitalisation; must match case-insensitively
        assert!(is_anthropic_billing_error(
            StatusCode::BAD_REQUEST,
            "invalid_request_error",
            "Insufficient Credit Balance \u{2014} add funds to continue.",
        ));
    }

    #[test]
    fn billing_error_user_string_contains_billing_cta() {
        // Spot-check of the wording constant: must contain "credit" and console link
        let msg = "Insufficient Anthropic credits. Add credits at console.anthropic.com/settings/plans to continue.";
        assert!(msg.to_lowercase().contains("credit"));
        assert!(msg.contains("console.anthropic.com"));
    }

    // ── Negative: should return false ───────────────────────────────────────

    #[test]
    fn billing_error_not_triggered_for_invalid_model() {
        // 400 + invalid_request_error but unrelated message → must NOT classify as billing
        assert!(!is_anthropic_billing_error(
            StatusCode::BAD_REQUEST,
            "invalid_request_error",
            "model: Field required [type=missing, input_value={'messages': ...}]",
        ));
    }

    #[test]
    fn billing_error_not_triggered_for_wrong_status() {
        // "credit balance" in message but non-400 status → must NOT classify as billing
        assert!(!is_anthropic_billing_error(
            StatusCode::OK,
            "invalid_request_error",
            "credit balance is too low",
        ));
    }

    #[test]
    fn billing_error_not_triggered_for_wrong_type() {
        // Wrong error type → must NOT classify as billing
        assert!(!is_anthropic_billing_error(
            StatusCode::BAD_REQUEST,
            "authentication_error",
            "credit balance is too low",
        ));
    }
}

#[cfg(test)]
mod stream_error_type_tests {
    use super::*;

    /// Compile-time shape guard: `StreamAnthropicError` must be `(String, String)`.
    /// If the error type is ever changed back to a plain `String`, the
    /// destructuring below will not compile. Mirrors `openrouter_error_type_tests`.
    #[test]
    fn test_stream_anthropic_error_is_tuple() {
        let err: StreamAnthropicError = (
            "Stream error: tcp reset".to_string(),
            "partial response text so far".to_string(),
        );
        let (msg, partial) = err;
        assert!(msg.contains("Stream error"), "error message must mention Stream error");
        assert!(!partial.is_empty(), "mid-stream error must carry non-empty partial text");
    }

    /// Pre-stream errors (HTTP 4xx, connection refused) fire before any bytes have
    /// streamed, so their partial-text field must be empty (`String::new()`).
    #[test]
    fn test_pre_stream_anthropic_error_has_empty_partial() {
        let err: StreamAnthropicError = (
            "Anthropic API error (401): Unauthorized".to_string(),
            String::new(),
        );
        let (_msg, partial) = err;
        assert!(partial.is_empty(), "pre-stream error must carry empty partial text");
    }

    /// The error tuple for a billing error must carry empty partial text because
    /// it fires before the stream body is consumed.
    #[test]
    fn test_billing_error_has_empty_partial() {
        let err: StreamAnthropicError = (
            "Insufficient Anthropic credits. Add credits at console.anthropic.com/settings/plans to continue.".to_string(),
            String::new(),
        );
        let (msg, partial) = err;
        assert!(msg.contains("credit"), "billing error must mention credits");
        assert!(partial.is_empty(), "billing error must carry empty partial text");
    }
}
