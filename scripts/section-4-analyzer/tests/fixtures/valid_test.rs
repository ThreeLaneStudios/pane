// Fixture: valid test that calls production function (should PASS)

#[test]
fn test_calls_production() {
    let result = route_memory_facts();
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_async_production_call() {
    let gateway = create_gateway().await;
    let response = send_request(&gateway).await;
    assert_eq!(response.status(), 200);
}
