// Fixture: helper test with valid annotation (should PASS)

/// §4-helper-test: This tests the internal threshold calculation logic before integration
#[test]
fn test_threshold_helper() {
    let threshold = calculate_threshold(100);
    assert_eq!(threshold, 50);
}
