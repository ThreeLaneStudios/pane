// G2 fixture: VIOLATION — hardcoded max_tokens literal in JSON body
// This file exists only to test the G2 guard. It must NOT appear in production source.

async fn stream_openrouter(request: ChatRequest) -> Result<impl Stream> {
    // Violation: hardcoded integer literal for max_tokens, ignores request.max_tokens
    let body = serde_json::json!({
        "model": request.model,
        "max_tokens": 4096,
        "stream": true,
        "messages": request.messages,
    });
    stream_request(body).await
}
