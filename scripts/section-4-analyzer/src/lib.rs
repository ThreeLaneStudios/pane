pub mod collector;
pub mod turn_origin;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use syn::visit::Visit;
use syn::{File, ItemFn};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ViolationKind {
    NoProductionCall,
    InvalidHelperAnnotation,
    UntestableMissingIssue,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Violation {
    pub file: PathBuf,
    pub line: usize,
    pub test_name: String,
    pub kind: ViolationKind,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UntestableAnnotation {
    pub file: PathBuf,
    pub test_name: String,
    pub issue_number: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisResult {
    pub violations: Vec<Violation>,
    pub untestable_annotations: Vec<UntestableAnnotation>,
}

/// Check if a function has #[test] or #[tokio::test] attribute
fn is_test_function(func: &ItemFn) -> bool {
    func.attrs.iter().any(|attr| {
        if attr.path().is_ident("test") {
            return true;
        }
        
        // Check for #[tokio::test]
        let path = attr.path();
        if path.segments.len() == 2 {
            if let (Some(first), Some(second)) = (path.segments.first(), path.segments.last()) {
                if first.ident == "tokio" && second.ident == "test" {
                    return true;
                }
            }
        }
        
        false
    })
}

/// Check for allowlist annotations in the function's documentation/comments
#[derive(Debug)]
enum Allowlist {
    None,
    HelperTest(String), // justification
    Untestable(String), // issue number
}

fn check_allowlist_annotations(func: &ItemFn) -> Allowlist {
    // Check attributes for doc comments
    // Doc comments are #[doc = "..."] attributes
    for attr in &func.attrs {
        if attr.path().is_ident("doc") {
            // Parse the meta to get the doc string
            if let syn::Meta::NameValue(meta) = &attr.meta {
                if let syn::Expr::Lit(expr_lit) = &meta.value {
                    if let syn::Lit::Str(lit_str) = &expr_lit.lit {
                        let comment = lit_str.value();
                        
                        // Check for §4-helper-test
                        if let Some(pos) = comment.find("§4-helper-test:") {
                            let justification = comment[pos + "§4-helper-test:".len()..].trim();
                            return Allowlist::HelperTest(justification.to_string());
                        }
                        
                        // Check for §4-untestable
                        if let Some(pos) = comment.find("§4-untestable:") {
                            let issue_ref = comment[pos + "§4-untestable:".len()..].trim();
                            return Allowlist::Untestable(issue_ref.to_string());
                        }
                    }
                }
            }
        }
    }
    
    Allowlist::None
}

/// Visitor to collect function calls from a test function body
struct CallCollector {
    calls: HashSet<String>,
}

impl CallCollector {
    fn new() -> Self {
        Self {
            calls: HashSet::new(),
        }
    }
}

impl<'ast> Visit<'ast> for CallCollector {
    fn visit_expr_call(&mut self, node: &'ast syn::ExprCall) {
        // Extract function name from call expression
        if let syn::Expr::Path(path_expr) = &*node.func {
            if let Some(segment) = path_expr.path.segments.last() {
                self.calls.insert(segment.ident.to_string());
            }
        }
        
        // Continue visiting nested expressions
        syn::visit::visit_expr_call(self, node);
    }
    
    fn visit_expr_method_call(&mut self, node: &'ast syn::ExprMethodCall) {
        // Method calls also count
        self.calls.insert(node.method.to_string());
        
        // Continue visiting
        syn::visit::visit_expr_method_call(self, node);
    }
    
    fn visit_macro(&mut self, node: &'ast syn::Macro) {
        // Handle known macros like tauri::command!
        if let Some(segment) = node.path.segments.last() {
            // For now, be conservative and count macro invocations
            self.calls.insert(segment.ident.to_string());
        }
        
        syn::visit::visit_macro(self, node);
    }
}

/// Analyze a test file for §4 compliance
pub fn analyze(test_file: &Path, production_symbols: &HashSet<String>) -> Result<AnalysisResult> {
    eprintln!("[ANALYZER] Analyzing file: {}", test_file.display());
    eprintln!("[ANALYZER] Production symbols count: {}", production_symbols.len());
    
    let content = std::fs::read_to_string(test_file)
        .with_context(|| format!("Failed to read test file: {}", test_file.display()))?;
    
    let syntax_tree: File = syn::parse_file(&content)
        .with_context(|| format!("Failed to parse test file: {}", test_file.display()))?;
    
    let mut violations = Vec::new();
    let mut untestable_annotations = Vec::new();
    
    // Walk all items looking for test functions
    for item in &syntax_tree.items {
        if let syn::Item::Fn(func) = item {
            if is_test_function(func) {
                eprintln!("[ANALYZER] Found test function: {}", func.sig.ident);
                
                let test_name = func.sig.ident.to_string();
                // Note: syn 2.x doesn't provide easy access to line numbers from Span
                // For now, we'll use a placeholder. In a real CI environment, we'd track this differently.
                let line = 0; // Placeholder - would need proc-macro2 unstable features for real line numbers
                
                // Check for allowlist annotations
                let allowlist = check_allowlist_annotations(func);
                eprintln!("[ANALYZER]   Allowlist status: {:?}", allowlist);
                
                match allowlist {
                    Allowlist::HelperTest(justification) => {
                        if justification.is_empty() || justification.trim().is_empty() {
                            violations.push(Violation {
                                file: test_file.to_path_buf(),
                                line,
                                test_name: test_name.clone(),
                                kind: ViolationKind::InvalidHelperAnnotation,
                                detail: "§4-helper-test annotation requires non-empty justification".to_string(),
                            });
                            // Invalid annotation - don't continue checking for production calls
                            continue;
                        } else {
                            // Valid helper test annotation - skip production call check
                            eprintln!("[ANALYZER]   Valid helper-test annotation, skipping");
                            continue;
                        }
                    }
                    Allowlist::Untestable(issue_ref) => {
                        // Validate issue reference format (#NNN)
                        if !issue_ref.starts_with('#') || issue_ref.len() < 2 {
                            violations.push(Violation {
                                file: test_file.to_path_buf(),
                                line,
                                test_name: test_name.clone(),
                                kind: ViolationKind::UntestableMissingIssue,
                                detail: format!("§4-untestable annotation has invalid issue reference: {}", issue_ref),
                            });
                            // Invalid annotation - don't continue checking for production calls
                            continue;
                        }
                        
                        // Extract numeric issue number (remove leading #)
                        let issue_number = issue_ref[1..].trim().to_string();
                        
                        // Validate it's actually numeric
                        if issue_number.parse::<u32>().is_err() {
                            violations.push(Violation {
                                file: test_file.to_path_buf(),
                                line,
                                test_name: test_name.clone(),
                                kind: ViolationKind::UntestableMissingIssue,
                                detail: format!("§4-untestable annotation has non-numeric issue reference: {}", issue_ref),
                            });
                            continue;
                        }
                        
                        // Valid untestable annotation - record it for issue-open check
                        eprintln!("[ANALYZER]   Valid untestable annotation: #{}, recording for issue check", issue_number);
                        untestable_annotations.push(UntestableAnnotation {
                            file: test_file.to_path_buf(),
                            test_name: test_name.clone(),
                            issue_number,
                        });
                        continue;
                    }
                    Allowlist::None => {
                        // No allowlist - need to check for production calls
                    }
                }
                
                // Collect all function/method calls in the test body
                let mut visitor = CallCollector::new();
                visitor.visit_item_fn(func);
                
                eprintln!("[ANALYZER]   Calls found: {:?}", visitor.calls);
                
                // Check if any call matches a production symbol
                let has_production_call = visitor.calls.iter().any(|call| {
                    production_symbols.contains(call)
                });
                
                if !has_production_call {
                    violations.push(Violation {
                        file: test_file.to_path_buf(),
                        line,
                        test_name,
                        kind: ViolationKind::NoProductionCall,
                        detail: format!(
                            "Test function contains no calls to production symbols. Calls found: {:?}",
                            visitor.calls
                        ),
                    });
                    eprintln!("[ANALYZER]   ❌ No production call found");
                } else {
                    eprintln!("[ANALYZER]   ✅ Production call found");
                }
            }
        }
    }
    
    eprintln!("[ANALYZER] Analysis complete. Violations: {}, Untestable annotations: {}", violations.len(), untestable_annotations.len());
    Ok(AnalysisResult {
        violations,
        untestable_annotations,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_violation_kind_serialization() {
        let kind = ViolationKind::NoProductionCall;
        let json = serde_json::to_string(&kind).unwrap();
        assert_eq!(json, "\"no_production_call\"");
    }
    
    #[test]
    fn test_violation_serialization() {
        let violation = Violation {
            file: PathBuf::from("test.rs"),
            line: 42,
            test_name: "test_foo".to_string(),
            kind: ViolationKind::NoProductionCall,
            detail: "No production calls found".to_string(),
        };
        
        let json = serde_json::to_string(&violation).unwrap();
        assert!(json.contains("test_foo"));
        assert!(json.contains("no_production_call"));
    }
}
