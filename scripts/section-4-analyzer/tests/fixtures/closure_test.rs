// Fixture: test calling production via closure (should PASS)

#[test]
fn test_closure_call() {
    let process = || {
        handle_event()
    };
    
    let result = process();
    assert!(result.is_ok());
}
