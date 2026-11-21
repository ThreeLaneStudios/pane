// Fixture: test calling only stdlib functions (should FAIL)

#[test]
fn test_only_stdlib() {
    let vec = Vec::new();
    let string = String::from("test");
    assert!(vec.is_empty());
    assert_eq!(string.len(), 4);
}
