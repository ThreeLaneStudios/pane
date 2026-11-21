// Fixture: helper test with empty justification (should FAIL)

/// §4-helper-test:
#[test]
fn test_helper_no_justification() {
    let result = internal_helper();
    assert!(result);
}

/// §4-helper-test:   
#[test]
fn test_helper_whitespace_only() {
    assert_eq!(2 + 2, 4);
}
