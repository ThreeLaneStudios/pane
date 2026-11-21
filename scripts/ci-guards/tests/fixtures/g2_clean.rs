// G2 fixture: CLEAN — max_tokens derived from ChatRequest, no literal
// This file exists only to test the G2 guard. It must NOT appear in production source.

async fn stream_openrouter(request: ChatRequest) -> Result<impl Stream> {
    // Clean: max_tokens comes from the incoming request, not a hardcoded value
    let body = serde_json::json!({
        "model": request.model,
        "max_tokens": request.max_tokens,
        "stream": true,
        "messages": request.messages,
    });
    stream_request(body).await
}
