use super::TestGateway;
use serde_json::json;

/// §4-helper-test: HTTP integration test - exercises production handler sessions::create via full HTTP stack
#[tokio::test]
async fn test_session_create_stores_mapping() {
    eprintln!("[SESSION_TEST] test_session_create_stores_mapping");
    let gateway = TestGateway::new().await.expect("Failed to create test gateway");

    // Configure mock to return a session ID (OC returns camelCase)
    let oc_session_id = "oc-session-123";
    gateway
        .mock()
        .configure_response("sessions.create", json!({ "sessionId": oc_session_id, "key": oc_session_id }))
        .await;

    let response = gateway
        .client()
        .post(&format!("{}/v1/sessions", gateway.base_url()))
        .header("Authorization", format!("Bearer {}", gateway.auth_token()))
        .json(&json!({
            "oc_agent_id": "test-agent"
        }))
        .send()
        .await
        .expect("Failed to send request");

    eprintln!("[SESSION_TEST] Response status: {}", response.status());
    assert_eq!(response.status(), 200);

    let body: serde_json::Value = response.json().await.expect("Failed to parse JSON");
    eprintln!("[SESSION_TEST] Response body: {}", body);
    
    let gateway_session_id = body["session_id"].as_str().expect("Missing session ID");

    // Verify mapping exists in shared state
    let mappings = gateway.shared_state().session_mappings.read().await;
    assert!(mappings.contains_key(gateway_session_id));
    assert_eq!(mappings.get(gateway_session_id).unwrap(), oc_session_id);
}

/// §4-helper-test: HTTP integration test - exercises production handlers (sessions::create, sessions::send_message) via full HTTP stack
#[tokio::test]
async fn test_session_create_then_send() {
    eprintln!("[SESSION_TEST] test_session_create_then_send — CRITICAL TEST (would catch #749)");
    let gateway = TestGateway::new().await.expect("Failed to create test gateway");

    // Step 1: Create session (OC returns camelCase)
    let oc_session_id = "oc-session-456";
    gateway
        .mock()
        .configure_response("sessions.create", json!({ "sessionId": oc_session_id, "key": oc_session_id }))
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

    assert_eq!(create_response.status(), 200);
    
    let create_body: serde_json::Value = create_response.json().await.expect("Failed to parse JSON");
    let gateway_session_id = create_body["session_id"].as_str().expect("Missing session ID");
    eprintln!("[SESSION_TEST] Created session: gateway_id={}, oc_id={}", gateway_session_id, oc_session_id);

    // Step 2: Configure mock for message send (OC returns messageSeq)
    gateway
        .mock()
        .configure_response("sessions.send", json!({ "messageSeq": 123 }))
        .await;

    // Clear previous calls to isolate the send call
    gateway.mock().clear_calls().await;

    // Step 3: Send message to the session
    let send_response = gateway
        .client()
        .post(&format!("{}/v1/sessions/{}/messages", gateway.base_url(), gateway_session_id))
        .header("Authorization", format!("Bearer {}", gateway.auth_token()))
        .json(&json!({
            "role": "user",
            "content": "test message",
            "idempotency_key": "test-key-123"
        }))
        .send()
        .await
        .expect("Failed to send request");

    eprintln!("[SESSION_TEST] Send response status: {}", send_response.status());
    assert_eq!(send_response.status(), 200);

    // Step 4: CRITICAL ASSERTION — Verify mock OC received the OC session ID, not the gateway UUID
    let calls = gateway.mock().get_calls().await;
    eprintln!("[SESSION_TEST] Mock OC received {} calls", calls.len());
    
    // Find the sessions.send call
    let send_call = calls.iter()
        .find(|(method, _)| method == "sessions.send")
        .expect("sessions.send not called on mock OC");

    eprintln!("[SESSION_TEST] sessions.send params: {}", send_call.1);
    
    // Extract key from params (OC expects key, not session_id)
    let sent_session_id = send_call.1["key"]
        .as_str()
        .expect("Missing key in sessions.send params");

    // This is the test that would have caught #749
    assert_eq!(
        sent_session_id, oc_session_id,
        "Gateway sent wrong session ID to OC! Expected OC ID '{}', got '{}'",
        oc_session_id, sent_session_id
    );
}

/// §4-helper-test: HTTP integration test - exercises production handlers (sessions::create, sessions::delete_session) via full HTTP stack
#[tokio::test]
async fn test_session_delete_cleans_mapping() {
    eprintln!("[SESSION_TEST] test_session_delete_cleans_mapping");
    let gateway = TestGateway::new().await.expect("Failed to create test gateway");

    // Step 1: Create session (OC returns camelCase)
    let oc_session_id = "oc-session-789";
    gateway
        .mock()
        .configure_response("sessions.create", json!({ "sessionId": oc_session_id, "key": oc_session_id }))
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
    let gateway_session_id = create_body["session_id"].as_str().expect("Missing session ID");

    // Verify mapping exists
    {
        let mappings = gateway.shared_state().session_mappings.read().await;
        assert!(mappings.contains_key(gateway_session_id));
    }

    // Step 2: Delete session
    gateway
        .mock()
        .configure_response("sessions.delete", json!({ "success": true }))
        .await;

    let delete_response = gateway
        .client()
        .delete(&format!("{}/v1/sessions/{}", gateway.base_url(), gateway_session_id))
        .header("Authorization", format!("Bearer {}", gateway.auth_token()))
        .send()
        .await
        .expect("Failed to send request");

    assert_eq!(delete_response.status(), 200);

    // Step 3: Verify mapping is removed
    let mappings = gateway.shared_state().session_mappings.read().await;
    assert!(!mappings.contains_key(gateway_session_id), "Mapping should be removed after delete");
}
