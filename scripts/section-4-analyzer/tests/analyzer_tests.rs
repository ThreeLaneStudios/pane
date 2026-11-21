use section_4_analyzer::{analyze, ViolationKind, AnalysisResult};
use std::collections::HashSet;
use std::path::Path;

fn load_production_symbols(name: &str) -> HashSet<String> {
    let path = format!("tests/fixtures/{}.json", name);
    let content = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("Failed to read {}", path));
    serde_json::from_str(&content)
        .unwrap_or_else(|_| panic!("Failed to parse {}", path))
}

#[test]
fn test_stub_test_fails() {
    eprintln!("\n=== TEST: Stub test should fail ===");
    let symbols = load_production_symbols("empty_symbols");
    let result = analyze(Path::new("tests/fixtures/stub_test.rs"), &symbols).unwrap();
    
    eprintln!("Violations found: {}", result.violations.len());
    for v in &result.violations {
        eprintln!("  - {} at line {}: {:?}", v.test_name, v.line, v.kind);
    }
    
    // Should have 2 violations (one for each test function)
    assert_eq!(result.violations.len(), 2);
    assert!(result.violations.iter().all(|v| v.kind == ViolationKind::NoProductionCall));
}

#[test]
fn test_valid_test_passes() {
    eprintln!("\n=== TEST: Valid test should pass ===");
    let symbols = load_production_symbols("production_symbols");
    let result = analyze(Path::new("tests/fixtures/valid_test.rs"), &symbols).unwrap();
    
    eprintln!("Violations found: {}", result.violations.len());
    for v in &result.violations {
        eprintln!("  - {} at line {}: {:?}", v.test_name, v.line, v.kind);
    }
    
    // Should have no violations
    assert_eq!(result.violations.len(), 0);
}

#[test]
fn test_helper_test_valid_passes() {
    eprintln!("\n=== TEST: Valid helper-test annotation should pass ===");
    let symbols = load_production_symbols("empty_symbols");
    let result = analyze(Path::new("tests/fixtures/helper_test_valid.rs"), &symbols).unwrap();
    
    eprintln!("Violations found: {}", result.violations.len());
    for v in &result.violations {
        eprintln!("  - {} at line {}: {:?}", v.test_name, v.line, v.kind);
    }
    
    // Should have no violations (annotation suppresses the check)
    assert_eq!(result.violations.len(), 0);
}

#[test]
fn test_untestable_valid_passes() {
    eprintln!("\n=== TEST: Valid untestable annotation should pass ===");
    let symbols = load_production_symbols("empty_symbols");
    let result = analyze(Path::new("tests/fixtures/untestable_valid.rs"), &symbols).unwrap();
    
    eprintln!("Violations found: {}", result.violations.len());
    eprintln!("Untestable annotations found: {}", result.untestable_annotations.len());
    for v in &result.violations {
        eprintln!("  - {} at line {}: {:?}", v.test_name, v.line, v.kind);
    }
    for u in &result.untestable_annotations {
        eprintln!("  - {} references issue #{}", u.test_name, u.issue_number);
    }
    
    // Should have no violations (annotation suppresses the check)
    assert_eq!(result.violations.len(), 0);
    // Should have 1 untestable annotation
    assert_eq!(result.untestable_annotations.len(), 1);
    assert_eq!(result.untestable_annotations[0].issue_number, "774");
}

#[test]
fn test_helper_test_empty_fails() {
    eprintln!("\n=== TEST: Empty helper-test justification should fail ===");
    let symbols = load_production_symbols("empty_symbols");
    let result = analyze(Path::new("tests/fixtures/helper_test_empty.rs"), &symbols).unwrap();
    
    eprintln!("Violations found: {}", result.violations.len());
    for v in &result.violations {
        eprintln!("  - {} at line {}: {:?}", v.test_name, v.line, v.kind);
    }
    
    // Should have 2 violations (one for each test with empty justification)
    assert_eq!(result.violations.len(), 2);
    assert!(result.violations.iter().all(|v| v.kind == ViolationKind::InvalidHelperAnnotation));
}

#[test]
fn test_stdlib_only_fails() {
    eprintln!("\n=== TEST: Stdlib-only test should fail ===");
    let symbols = load_production_symbols("production_symbols");
    let result = analyze(Path::new("tests/fixtures/stdlib_only_test.rs"), &symbols).unwrap();
    
    eprintln!("Violations found: {}", result.violations.len());
    for v in &result.violations {
        eprintln!("  - {} at line {}: {:?}", v.test_name, v.line, v.kind);
    }
    
    // Should have 1 violation
    assert_eq!(result.violations.len(), 1);
    assert_eq!(result.violations[0].kind, ViolationKind::NoProductionCall);
}

#[test]
fn test_method_call_passes() {
    eprintln!("\n=== TEST: Method call test should pass ===");
    let symbols = load_production_symbols("production_symbols");
    let result = analyze(Path::new("tests/fixtures/method_call_test.rs"), &symbols).unwrap();
    
    eprintln!("Violations found: {}", result.violations.len());
    for v in &result.violations {
        eprintln!("  - {} at line {}: {:?}", v.test_name, v.line, v.kind);
    }
    
    // Should have no violations (process_request is a production symbol)
    assert_eq!(result.violations.len(), 0);
}

#[test]
fn test_closure_call_passes() {
    eprintln!("\n=== TEST: Closure call test should pass ===");
    let symbols = load_production_symbols("production_symbols");
    let result = analyze(Path::new("tests/fixtures/closure_test.rs"), &symbols).unwrap();
    
    eprintln!("Violations found: {}", result.violations.len());
    for v in &result.violations {
        eprintln!("  - {} at line {}: {:?}", v.test_name, v.line, v.kind);
    }
    
    // Should have no violations (handle_event is a production symbol)
    assert_eq!(result.violations.len(), 0);
}
