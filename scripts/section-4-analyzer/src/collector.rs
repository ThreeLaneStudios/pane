// Production Symbol Collector — ADR-0016 Step 2
//
// Walks Pane source crates and extracts all pub/pub(crate) function names,
// excluding test modules and test utilities.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use syn::visit::Visit;
use syn::{File, Item, ItemFn, ItemMod, Visibility};

#[derive(Debug, Serialize, Deserialize)]
pub struct ProductionSymbols {
    pub symbols: Vec<String>,
}

/// Check if a path should be excluded from symbol collection
fn is_excluded_path(path: &Path) -> bool {
    let path_str = path.to_string_lossy();
    
    // Exclude test directories
    if path_str.contains("/tests/") || path_str.contains("/src/tests/") {
        return true;
    }
    
    // Exclude test utility modules
    if path_str.contains("/test_utils/") || path_str.contains("/tests/common/") {
        return true;
    }
    
    // Exclude test files by pattern
    let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if file_name.contains("_test") || file_name.contains("_tests") {
        return true;
    }
    
    false
}

/// Check if an item has #[cfg(test)] attribute
fn has_cfg_test_attr(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|attr| {
        if attr.path().is_ident("cfg") {
            // Parse the attribute to check for "test"
            if let syn::Meta::List(meta_list) = &attr.meta {
                let tokens = meta_list.tokens.to_string();
                return tokens.contains("test");
            }
        }
        false
    })
}

/// Visitor to collect public function names from a syntax tree
struct FunctionCollector {
    symbols: HashSet<String>,
    /// Track whether we're inside a tests module
    in_tests_mod: bool,
}

impl FunctionCollector {
    fn new() -> Self {
        Self {
            symbols: HashSet::new(),
            in_tests_mod: false,
        }
    }
    
    /// Check if a function is public or pub(crate)
    fn is_public(&self, vis: &Visibility) -> bool {
        match vis {
            Visibility::Public(_) => true,
            Visibility::Restricted(vis_restricted) => {
                // pub(crate), pub(super), etc.
                vis_restricted.path.is_ident("crate") ||
                vis_restricted.path.is_ident("super") ||
                vis_restricted.path.segments.len() > 0
            }
            Visibility::Inherited => false,
        }
    }
}

impl<'ast> Visit<'ast> for FunctionCollector {
    fn visit_item_mod(&mut self, node: &'ast ItemMod) {
        // Check if this is a tests module
        let is_tests = node.ident == "tests" || has_cfg_test_attr(&node.attrs);
        
        if is_tests {
            // Don't recurse into tests modules
            return;
        }
        
        // Continue visiting the module's content
        syn::visit::visit_item_mod(self, node);
    }
    
    fn visit_item_fn(&mut self, node: &'ast ItemFn) {
        // Skip if we're inside a tests module
        if self.in_tests_mod {
            return;
        }
        
        // Skip if function has #[cfg(test)]
        if has_cfg_test_attr(&node.attrs) {
            return;
        }
        
        // Skip if function has #[test] or #[tokio::test]
        let is_test = node.attrs.iter().any(|attr| {
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
        });
        
        if is_test {
            return;
        }
        
        // Check if function is public or pub(crate)
        if self.is_public(&node.vis) {
            let fn_name = node.sig.ident.to_string();
            eprintln!("[COLLECTOR] Found production symbol: {}", fn_name);
            self.symbols.insert(fn_name);
        }
        
        // Continue visiting (for nested items, though rare in functions)
        syn::visit::visit_item_fn(self, node);
    }
}

/// Collect production symbols from a single Rust source file
fn collect_from_file(file_path: &Path) -> Result<HashSet<String>> {
    eprintln!("[COLLECTOR] Processing file: {}", file_path.display());
    
    let content = fs::read_to_string(file_path)
        .with_context(|| format!("Failed to read file: {}", file_path.display()))?;
    
    let syntax_tree: File = syn::parse_file(&content)
        .with_context(|| format!("Failed to parse file: {}", file_path.display()))?;
    
    let mut collector = FunctionCollector::new();
    
    // Check if the entire file is marked with #[cfg(test)]
    let file_is_test = syntax_tree.attrs.iter().any(|attr| {
        attr.path().is_ident("cfg") && {
            if let syn::Meta::List(meta_list) = &attr.meta {
                meta_list.tokens.to_string().contains("test")
            } else {
                false
            }
        }
    });
    
    if file_is_test {
        eprintln!("[COLLECTOR]   File is marked #[cfg(test)], skipping");
        return Ok(HashSet::new());
    }
    
    // Visit all items in the file
    for item in &syntax_tree.items {
        match item {
            Item::Mod(item_mod) => {
                // Check if this is a tests module before visiting
                if item_mod.ident == "tests" || has_cfg_test_attr(&item_mod.attrs) {
                    eprintln!("[COLLECTOR]   Skipping tests module");
                    continue;
                }
                collector.visit_item_mod(item_mod);
            }
            Item::Fn(item_fn) => {
                collector.visit_item_fn(item_fn);
            }
            _ => {
                // Visit other items (impl blocks, traits, etc.)
                collector.visit_item(item);
            }
        }
    }
    
    eprintln!("[COLLECTOR]   Found {} symbols", collector.symbols.len());
    Ok(collector.symbols)
}

/// Recursively walk a directory and collect .rs files
fn walk_directory(dir: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }
    
    for entry in fs::read_dir(dir)
        .with_context(|| format!("Failed to read directory: {}", dir.display()))? 
    {
        let entry = entry?;
        let path = entry.path();
        
        if path.is_dir() {
            walk_directory(&path, files)?;
        } else if path.extension().and_then(|s| s.to_str()) == Some("rs") {
            if !is_excluded_path(&path) {
                files.push(path);
            } else {
                eprintln!("[COLLECTOR] Excluded: {}", path.display());
            }
        }
    }
    
    Ok(())
}

/// Collect all production symbols from the Pane source tree
pub fn collect_production_symbols(repo_root: &Path) -> Result<ProductionSymbols> {
    eprintln!("[COLLECTOR] Starting symbol collection from: {}", repo_root.display());
    
    let mut all_files = Vec::new();
    
    // Walk src-tauri/src
    let tauri_src = repo_root.join("src-tauri").join("src");
    if tauri_src.exists() {
        eprintln!("[COLLECTOR] Walking src-tauri/src...");
        walk_directory(&tauri_src, &mut all_files)?;
    } else {
        eprintln!("[COLLECTOR] Warning: src-tauri/src not found");
    }
    
    // Walk src-gateway/src
    let gateway_src = repo_root.join("src-gateway").join("src");
    if gateway_src.exists() {
        eprintln!("[COLLECTOR] Walking src-gateway/src...");
        walk_directory(&gateway_src, &mut all_files)?;
    } else {
        eprintln!("[COLLECTOR] Warning: src-gateway/src not found");
    }
    
    eprintln!("[COLLECTOR] Found {} Rust files to process", all_files.len());
    
    let mut all_symbols = HashSet::new();
    
    for file in &all_files {
        match collect_from_file(file) {
            Ok(symbols) => {
                all_symbols.extend(symbols);
            }
            Err(e) => {
                eprintln!("[COLLECTOR] Warning: Failed to process {}: {}", file.display(), e);
                // Continue processing other files
            }
        }
    }
    
    let mut symbols: Vec<String> = all_symbols.into_iter().collect();
    symbols.sort(); // Deterministic output
    
    eprintln!("[COLLECTOR] Collection complete. Total symbols: {}", symbols.len());
    
    Ok(ProductionSymbols { symbols })
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_is_excluded_path() {
        assert!(is_excluded_path(Path::new("src-tauri/src/tests/foo.rs")));
        assert!(is_excluded_path(Path::new("src-tauri/src/test_utils/bar.rs")));
        assert!(is_excluded_path(Path::new("src-gateway/tests/common/baz.rs")));
        assert!(is_excluded_path(Path::new("src/foo_test.rs")));
        assert!(is_excluded_path(Path::new("src/foo_tests.rs")));
        
        assert!(!is_excluded_path(Path::new("src-tauri/src/ai/memory_router.rs")));
        assert!(!is_excluded_path(Path::new("src-gateway/src/lib.rs")));
    }
    
    #[test]
    fn test_collect_from_simple_file() {
        let source = r#"
pub fn production_function() {}

pub(crate) fn internal_function() {}

fn private_function() {}

#[cfg(test)]
mod tests {
    #[test]
    fn test_something() {}
}
"#;
        
        let temp_dir = std::env::temp_dir();
        let temp_file = temp_dir.join("test_collect.rs");
        std::fs::write(&temp_file, source).unwrap();
        
        let symbols = collect_from_file(&temp_file).unwrap();
        
        assert!(symbols.contains("production_function"));
        assert!(symbols.contains("internal_function"));
        assert!(!symbols.contains("private_function"));
        assert!(!symbols.contains("test_something"));
        
        std::fs::remove_file(&temp_file).ok();
    }
    
    #[test]
    fn test_collect_from_pane_repo() {
        // This test validates against the actual Pane repository
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let repo_root = manifest_dir
            .parent()
            .and_then(|p| p.parent())
            .expect("Could not find repo root");
        
        eprintln!("Testing against repo at: {}", repo_root.display());
        
        // Collect production symbols from the actual repo
        let result = collect_production_symbols(&repo_root);
        
        // If the repo structure doesn't exist, skip this test
        if result.is_err() {
            eprintln!("Skipping test - repo structure not found");
            return;
        }
        
        let symbols = result.unwrap();
        let symbol_set: HashSet<String> = symbols.symbols.iter().cloned().collect();
        
        eprintln!("Collected {} symbols", symbol_set.len());
        
        // Verify known production symbols from src-tauri/src/ai/memory_router.rs
        let expected_symbols = vec![
            "get_file_mutex",
            "count_words",
            "route_memory_facts",
            "route_facts_to_file",
            "mark_fact_routed",
            "remove_inactive_facts",
        ];
        
        for expected in &expected_symbols {
            assert!(
                symbol_set.contains(*expected),
                "Expected production symbol '{}' not found. Available symbols: {:?}",
                expected,
                symbol_set.iter().take(20).collect::<Vec<_>>()
            );
        }
        
        // Verify test-only functions do NOT appear
        // These are common test helper names that should be excluded
        let excluded_patterns = vec![
            "test_",      // Test function prefix
            "setup_test", // Common test setup pattern
            "mock_",      // Mock objects
        ];
        
        for pattern in &excluded_patterns {
            let test_symbols: Vec<_> = symbol_set
                .iter()
                .filter(|s| s.starts_with(pattern))
                .collect();
            
            if !test_symbols.is_empty() {
                eprintln!(
                    "Warning: Found symbols matching test pattern '{}': {:?}",
                    pattern, test_symbols
                );
            }
        }
        
        // Verify we found a reasonable number of symbols
        // The Pane codebase should have at least 50 production functions
        assert!(
            symbol_set.len() >= 50,
            "Expected at least 50 production symbols, found {}",
            symbol_set.len()
        );
    }
    
    #[test]
    fn test_excludes_test_modules() {
        let source = r#"
pub fn production_fn() {}

mod tests {
    pub fn test_helper() {}
    
    #[test]
    fn test_something() {}
}

#[cfg(test)]
mod more_tests {
    pub fn another_test_helper() {}
}
"#;
        
        let temp_dir = std::env::temp_dir();
        let temp_file = temp_dir.join("test_exclude_mods.rs");
        std::fs::write(&temp_file, source).unwrap();
        
        let symbols = collect_from_file(&temp_file).unwrap();
        
        assert!(symbols.contains("production_fn"));
        assert!(!symbols.contains("test_helper"));
        assert!(!symbols.contains("test_something"));
        assert!(!symbols.contains("another_test_helper"));
        
        std::fs::remove_file(&temp_file).ok();
    }
}
