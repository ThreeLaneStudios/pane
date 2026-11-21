use super::TestGateway;

#[tokio::test]
async fn test_health_no_auth() {
    eprintln!("[HEALTH_TEST] test_health_no_auth");
    let gateway = TestGateway::new().await.expect("Failed to create test gateway");

    let response = gateway
        .client()
        .get(&format!("{}/v1/health", gateway.base_url()))
        .send()
        .await
        .expect("Failed to send request");

    eprintln!("[HEALTH_TEST] Response status: {}", response.status());
    assert_eq!(response.status(), 200);

    let body: serde_json::Value = response.json().await.expect("Failed to parse JSON");
    eprintln!("[HEALTH_TEST] Response body: {}", body);
    
    assert_eq!(body["status"].as_str().unwrap(), "healthy");
}

#[tokio::test]
async fn test_health_with_auth_shows_details() {
    eprintln!("[HEALTH_TEST] test_health_with_auth_shows_details");
    let gateway = TestGateway::new().await.expect("Failed to create test gateway");

    let response = gateway
        .client()
        .get(&format!("{}/v1/health", gateway.base_url()))
        .header("Authorization", format!("Bearer {}", gateway.auth_token()))
        .send()
        .await
        .expect("Failed to send request");

    eprintln!("[HEALTH_TEST] Response status: {}", response.status());
    assert_eq!(response.status(), 200);

    let body: serde_json::Value = response.json().await.expect("Failed to parse JSON");
    eprintln!("[HEALTH_TEST] Response body: {}", body);
    
    assert_eq!(body["status"].as_str().unwrap(), "healthy");
    // With auth, we should get more details
    assert!(body.get("uptime").is_some());
}
