use super::TestGateway;
use serde_json::json;

#[tokio::test]
async fn test_pair_returns_token() {
    eprintln!("[AUTH_TEST] test_pair_returns_token");
    let gateway = TestGateway::new().await.expect("Failed to create test gateway");

    // Generate a fresh pairing code
    let pairing_code = gateway.shared_state().auth_state.generate_pairing_code().await;
    
    let response = gateway
        .client()
        .post(&format!("{}/pair", gateway.base_url()))
        .json(&json!({ "code": pairing_code }))
        .send()
        .await
        .expect("Failed to send request");

    eprintln!("[AUTH_TEST] Response status: {}", response.status());
    assert_eq!(response.status(), 200);

    let body: serde_json::Value = response.json().await.expect("Failed to parse JSON");
    eprintln!("[AUTH_TEST] Response body: {}", body);
    
    assert!(body.get("token").is_some());
    assert!(body["token"].as_str().unwrap().len() > 0);
}

#[tokio::test]
async fn test_protected_endpoint_without_token() {
    eprintln!("[AUTH_TEST] test_protected_endpoint_without_token");
    let gateway = TestGateway::new().await.expect("Failed to create test gateway");

    let response = gateway
        .client()
        .get(&format!("{}/v1/agents", gateway.base_url()))
        .send()
        .await
        .expect("Failed to send request");

    eprintln!("[AUTH_TEST] Response status: {}", response.status());
    assert_eq!(response.status(), 401);
}

#[tokio::test]
async fn test_protected_endpoint_with_token() {
    eprintln!("[AUTH_TEST] test_protected_endpoint_with_token");
    let gateway = TestGateway::new().await.expect("Failed to create test gateway");

    // Configure mock to return empty agent list
    gateway
        .mock()
        .configure_response("agents.list", json!({ "agents": [] }))
        .await;

    let response = gateway
        .client()
        .get(&format!("{}/v1/agents", gateway.base_url()))
        .header("Authorization", format!("Bearer {}", gateway.auth_token()))
        .send()
        .await
        .expect("Failed to send request");

    eprintln!("[AUTH_TEST] Response status: {}", response.status());
    assert_eq!(response.status(), 200);
}
