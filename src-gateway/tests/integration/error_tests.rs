use super::TestGateway;
use serde_json::json;

#[tokio::test]
async fn test_unknown_route_returns_404() {
    eprintln!("[ERROR_TEST] test_unknown_route_returns_404");
    let gateway = TestGateway::new().await.expect("Failed to create test gateway");

    let response = gateway
        .client()
        .get(&format!("{}/nonexistent", gateway.base_url()))
        .send()
        .await
        .expect("Failed to send request");

    eprintln!("[ERROR_TEST] Response status: {}", response.status());
    assert_eq!(response.status(), 404);

    // Try to get response body - might be JSON or text
    let body_text = response.text().await.expect("Failed to get response text");
    eprintln!("[ERROR_TEST] Response body text: {}", body_text);
    
    // Check if it's JSON
    if !body_text.is_empty() {
        if let Ok(body) = serde_json::from_str::<serde_json::Value>(&body_text) {
            assert_eq!(body["error"].as_str().unwrap(), "not_found");
        }
    }
}

/// §4-helper-test: HTTP integration test - exercises production handlers (sessions::create, sessions::send_message) via full HTTP stack
#[tokio::test]
async fn test_oversized_session_message() {
    eprintln!("[ERROR_TEST] test_oversized_session_message");
    let gateway = TestGateway::new().await.expect("Failed to create test gateway");

    // Create a session first (OC returns camelCase)
    gateway
        .mock()
        .configure_response("sessions.create", json!({ "sessionId": "test-session", "key": "test-session" }))
        .await;

    let create_response = gateway
        .client()
        .post(&format!("{}/v1/sessions", gateway.base_url()))
        .header("Authorization", format!("Bearer {}", gateway.auth_token()))
        .json(&json!({
            "oc_agent_id": "test-agent"
        }))
        .send()
        .await
        .expect("Failed to send request");

    let create_body: serde_json::Value = create_response.json().await.expect("Failed to parse JSON");
    let session_id = create_body["session_id"].as_str().expect("Missing session ID");

    // Try to send oversized message (>1 MiB)
    let large_content = "x".repeat(2 * 1024 * 1024);
    let result = gateway
        .client()
        .post(&format!("{}/v1/sessions/{}/messages", gateway.base_url(), session_id))
        .header("Authorization", format!("Bearer {}", gateway.auth_token()))
        .json(&json!({
            "role": "user",
            "content": large_content,
            "idempotency_key": "test-key"
        }))
        .send()
        .await;

    // The server should reject the oversized payload
    // This can manifest as either:
    // 1. A 413 response, OR
    // 2. A connection error (server closes connection during body send)
    match result {
        Ok(response) => {
            eprintln!("[ERROR_TEST] Response status: {}", response.status());
            assert_eq!(response.status(), 413);
        }
        Err(e) => {
            eprintln!("[ERROR_TEST] Request failed (expected - server rejected oversized payload): {}", e);
            // This is acceptable - the server closed the connection or rejected the request
            // The fact that it errored is sufficient validation
        }
    }
}
