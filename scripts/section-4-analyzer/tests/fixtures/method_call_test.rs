// Fixture: test calling production via method (should PASS)

#[test]
fn test_method_call() {
    let gateway = TestGateway::new();
    let result = gateway.process_request();
    assert!(result.is_ok());
}
