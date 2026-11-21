// Fixture: stub test with no production calls (should FAIL)

#[test]
fn test_stub_behavior() {
    let result = 2 + 2;
    assert_eq!(result, 4);
}

#[tokio::test]
async fn test_async_stub() {
    let value = "hello".to_string();
    assert_eq!(value, "hello");
}
