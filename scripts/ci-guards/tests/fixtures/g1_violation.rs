// G1 fixture: VIOLATION — hardcoded temperature literal in non-test Rust code
// This file exists only to test the G1 guard. It must NOT appear in production source.

fn stream_example(body: &mut serde_json::Value) {
    // Violation: hardcoded temperature literal (not a variable)
    *body = serde_json::json!({
        "model": model_id,
        "temperature": 0.7,
        "messages": messages,
    });
}

fn another_path(request: &ChatRequest) {
    let params = serde_json::json!({
        "temperature": 0.3,
    });
}
