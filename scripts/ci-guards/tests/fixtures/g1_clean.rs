// G1 fixture: CLEAN — temperature sourced from variable, not a literal
// This file exists only to test the G1 guard. It must NOT appear in production source.

fn stream_example(request: &ChatRequest) {
    // Clean: temperature is read from the request, not hardcoded
    let body = serde_json::json!({
        "model": model_id,
        "temperature": request.temperature,
        "messages": messages,
    });
}

fn internal_call() {
    // Clean: using a named constant (acceptable pattern once constants are extracted)
    let body = serde_json::json!({
        "messages": messages,
    });
}
