// Build v61 fixture with CRR enabled
// Compile with: rustc --edition 2021 build_v61_fixture.rs -L ../../src-tauri/target/debug/deps -l sqlite3
// Or run with: cargo run --bin build-v61-fixture

use rusqlite::{params, Connection, Result};
use std::path::PathBuf;

const FIXTURE_PATH: &str = "tests/fixtures/upgrade-from-v61.db";
const CRSQLITE_PATH: &str = "src-tauri/target/debug/resources/crsqlite.dylib";

const CRR_TABLES: [&str; 20] = [
    "projects",
    "notes",
    "tags",
    "note_tags",
    "note_links",
    "ai_context_markers",
    "ai_pinned_messages",
    "ai_conversation_summaries",
    "ai_memory",
    "ai_actions",
    "user_profiles",
    "user_dashboard_config",
    "dashboard_modules",
    "module_items",
    "daily_entries",
    "habit_entries",
    "reflection_entries",
    "gratitude_entries",
    "sync_state",
    "note_versions",
];

fn main() -> Result<()> {
    let fixture_path = PathBuf::from(FIXTURE_PATH);
    
    // Remove existing fixture if present
    if fixture_path.exists() {
        std::fs::remove_file(&fixture_path)?;
    }
    
    let conn = Connection::open(&fixture_path)?;
    
    // Load cr-sqlite extension
    unsafe {
        conn.load_extension(CRSQLITE_PATH, None)?;
    }
    eprintln!("✓ Loaded cr-sqlite extension");
    
    // Run the v61 schema SQL
    let schema_sql = include_str!("build-v61-fixture.sql");
    conn.execute_batch(schema_sql)?;
    eprintln!("✓ Created v61 schema with seed data");
    
    // Enable CRR on the 20 tables
    for table in &CRR_TABLES {
        conn.query_row(&format!("SELECT crsql_as_crr('{}');", table), [], |_| Ok(()))?;
        eprintln!("✓ CRR enabled: {}", table);
    }
    
    eprintln!("\n✓ v61 fixture created successfully at: {}", fixture_path.display());
    eprintln!("✓ Schema version: 61");
    eprintln!("✓ CRR enabled on 20 tables");
    
    // Verify
    let version: String = conn.query_row(
        "SELECT value FROM ai_settings WHERE key = 'schema_version'",
        [],
        |row| row.get(0),
    )?;
    eprintln!("✓ Verified schema_version: {}", version);
    
    let project_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM projects",
        [],
        |row| row.get(0),
    )?;
    eprintln!("✓ Seed data: {} project(s)", project_count);
    
    Ok(())
}
