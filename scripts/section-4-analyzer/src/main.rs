use anyhow::{Context, Result};
use section_4_analyzer::{analyze, collector::collect_production_symbols, turn_origin::check_turn_origin_construction};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

fn print_usage() {
    eprintln!("Usage:");
    eprintln!("  section-4-analyzer analyze <test-file> <production-symbols-json>");
    eprintln!("  section-4-analyzer collect-symbols <source-dir-1> [source-dir-2] ...");
    eprintln!("  section-4-analyzer check-turn-origin <source-dir-1> [source-dir-2] ...");
    eprintln!("");
    eprintln!("Commands:");
    eprintln!("  analyze           - Analyze a test file for §4 violations");
    eprintln!("  collect-symbols   - Collect production symbols from source directories");
    eprintln!("  check-turn-origin - §36.11: fail if TurnOrigin::User/Job is constructed");
    eprintln!("                      outside the allowlisted sites (#3059)");
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    
    if args.len() < 2 {
        print_usage();
        std::process::exit(1);
    }
    
    let command = &args[1];
    
    match command.as_str() {
        "analyze" => {
            if args.len() != 4 {
                eprintln!("Error: analyze command requires exactly 2 arguments");
                print_usage();
                std::process::exit(1);
            }
            
            let test_file = Path::new(&args[2]);
            let symbols_file = Path::new(&args[3]);
            
            // Load production symbols from JSON
            let symbols_content = std::fs::read_to_string(symbols_file)
                .with_context(|| format!("Failed to read symbols file: {}", symbols_file.display()))?;
            
            let production_symbols: HashSet<String> = serde_json::from_str(&symbols_content)
                .with_context(|| "Failed to parse production symbols JSON")?;
            
            // Analyze the test file
            let result = analyze(test_file, &production_symbols)?;
            
            // Output result as JSON
            let output = serde_json::to_string_pretty(&result)?;
            println!("{}", output);
            
            // Exit with code 1 if violations found
            if !result.violations.is_empty() {
                std::process::exit(1);
            }
        }
        "collect-symbols" => {
            if args.len() < 3 {
                eprintln!("Error: collect-symbols command requires at least one source directory");
                print_usage();
                std::process::exit(1);
            }
            
            // Determine the repo root
            // For Pane, we expect source dirs like src-tauri/src and src-gateway/src
            // We'll use the first argument as a base to find the repo root
            let first_dir = PathBuf::from(&args[2]);
            
            // Walk up to find the repo root (where src-tauri and src-gateway live)
            let repo_root = if first_dir.ends_with("src-tauri/src") || first_dir.ends_with("src-tauri\\src") {
                first_dir.parent().and_then(|p| p.parent()).unwrap_or(&first_dir)
            } else if first_dir.ends_with("src-gateway/src") || first_dir.ends_with("src-gateway\\src") {
                first_dir.parent().and_then(|p| p.parent()).unwrap_or(&first_dir)
            } else {
                // Assume the current directory or the provided path is the repo root
                &first_dir
            };
            
            eprintln!("[COLLECT] Using repo root: {}", repo_root.display());
            
            // Collect production symbols
            let symbols = collect_production_symbols(repo_root)?;
            
            // Output as JSON
            let output = serde_json::to_string_pretty(&symbols.symbols)?;
            println!("{}", output);
        }
        "check-turn-origin" => {
            if args.len() < 3 {
                eprintln!("Error: check-turn-origin command requires at least one source directory");
                print_usage();
                std::process::exit(1);
            }

            let source_dirs: Vec<PathBuf> = args[2..].iter().map(PathBuf::from).collect();
            let report = check_turn_origin_construction(&source_dirs)?;

            let output = serde_json::to_string_pretty(&report)?;
            println!("{}", output);

            if !report.violations.is_empty() {
                std::process::exit(1);
            }
        }
        _ => {
            eprintln!("Error: unknown command '{}'", command);
            print_usage();
            std::process::exit(1);
        }
    }
    
    Ok(())
}


