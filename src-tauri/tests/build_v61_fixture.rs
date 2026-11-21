// Test utility to build v61 fixture with CRR enabled
// Run with: cargo test --test build_v61_fixture -- --ignored --nocapture

use rusqlite::{Connection, Result};
use std::path::PathBuf;

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

#[test]
#[ignore] // Run manually with --ignored flag
fn build_v61_fixture_with_crr() -> Result<()> {
    let fixture_path = PathBuf::from("../tests/fixtures/upgrade-from-v61.db");
    let schema_sql = include_str!("../../tests/fixtures/build-v61-fixture.sql");
    let crsqlite_path = PathBuf::from("target/debug/resources/crsqlite.dylib");
    
    // Remove existing fixture
    if fixture_path.exists() {
        std::fs::remove_file(&fixture_path).unwrap();
    }
    
    let conn = Connection::open(&fixture_path)?;
    
    // Load cr-sqlite extension
    unsafe {
        conn.load_extension(&crsqlite_path, None)?;
    }
    eprintln!("✓ Loaded cr-sqlite extension");
    
    // Create v61 schema
    conn.execute_batch(schema_sql)?;
    eprintln!("✓ Created v61 schema (no seed data)");
    
    // Enable CRR on the 20 tables
    for table in &CRR_TABLES {
        conn.query_row(&format!("SELECT crsql_as_crr('{}');", table), [], |_| Ok(()))?;
        eprintln!("✓ CRR enabled: {}", table);
    }
    
    // Verify
    let version: String = conn.query_row(
        "SELECT value FROM ai_settings WHERE key = 'schema_version'",
        [],
        |row| row.get(0),
    )?;
    let project_count: i64 = conn.query_row("SELECT COUNT(*) FROM projects", [], |row| row.get(0))?;
    let note_count: i64 = conn.query_row("SELECT COUNT(*) FROM notes", [], |row| row.get(0))?;
    let crr_table_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name LIKE '%__crsql_clock'",
        [],
        |row| row.get(0),
    )?;
    
    eprintln!("\n✓ v61 fixture created successfully!");
    eprintln!("  Path: {}", fixture_path.display());
    eprintln!("  Schema version: {}", version);
    eprintln!("  Seed data: {} projects, {} notes", project_count, note_count);
    eprintln!("  CRR clock tables: {}", crr_table_count);
    
    assert_eq!(version, "61");
    assert_eq!(project_count, 0, "Fixture should have no seed projects (tests schema migration only)");
    assert_eq!(note_count, 0, "Fixture should have no seed notes (tests schema migration only)");
    assert_eq!(crr_table_count, 20, "Should have 20 CRR clock tables");
    
    Ok(())
}
