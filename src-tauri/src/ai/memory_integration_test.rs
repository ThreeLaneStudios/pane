// Integration test for Memory.md extraction pipeline
// Tests: Threshold check → route_memory_facts → Memory.md on disk
// Issue: #719

#[cfg(test)]
mod tests {
    use rusqlite::Connection;
    use std::path::PathBuf;
    use tempfile::TempDir;

    /// Helper: Create a test database with full schema
    fn setup_test_db() -> (TempDir, Connection) {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let conn = Connection::open(&db_path).unwrap();

        // Create minimal schema needed for the pipeline
        conn.execute_batch(
            r#"
            CREATE TABLE ai_memory (
                id                TEXT PRIMARY KEY,
                layer             TEXT NOT NULL DEFAULT '',
                project_id        TEXT,
                user_id           TEXT,
                fact              TEXT NOT NULL DEFAULT '',
                category          TEXT,
                source_message_id TEXT,
                is_active         INTEGER NOT NULL DEFAULT 1,
                created_at        TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at        TEXT,
                last_referenced   TEXT,
                routed_at         TEXT
            );
            
            CREATE TABLE ai_agents (
                id              TEXT PRIMARY KEY,
                name            TEXT NOT NULL,
                slug            TEXT NOT NULL UNIQUE,
                role            TEXT NOT NULL DEFAULT 'sub',
                description     TEXT,
                model           TEXT,
                is_default         INTEGER NOT NULL DEFAULT 0,
                is_active       INTEGER NOT NULL DEFAULT 1,
                workspace_path  TEXT NOT NULL,
                soul_hash       TEXT,
                memory_hash     TEXT,
                config_json     TEXT DEFAULT '{}',
                owner_id        TEXT,
                source          TEXT DEFAULT 'import',
                created_at      TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at      TEXT NOT NULL DEFAULT (datetime('now')),
                runtime_ref     TEXT
            );
            
            CREATE TABLE ai_settings (
                key   TEXT PRIMARY KEY NOT NULL,
                value TEXT NOT NULL
            );
            "#,
        )
        .expect("Failed to create test schema");

        (temp_dir, conn)
    }

    /// Helper: Set up a test workspace with .pane/system/{uuid}/ structure
    fn setup_test_workspace(temp_dir: &TempDir, agent_uuid: &str) -> PathBuf {
        let workspace_root = temp_dir.path().join("workspace");
        let system_dir = workspace_root.join(".pane").join("system").join(agent_uuid);
        std::fs::create_dir_all(&system_dir).unwrap();
        
        // Point the production filesystem policy at this isolated Pane root.
        std::env::set_var("PANE_TEST_ROOT", workspace_root.to_str().unwrap());
        
        workspace_root
    }

    #[test]
    #[serial_test::serial(pane_test_root)]
    fn test_memory_router_threshold_triggers_with_single_fact() {
        // Issue #719: Router should trigger with 1 unrouted fact, not 5+
        // This test verifies the threshold bug fix in extraction_worker.rs
        
        eprintln!("\n[TEST] #719: Testing router threshold logic with 1 unrouted fact");
        
        // 1. Set up test database and workspace
        let (temp_dir, conn) = setup_test_db();
        let agent_uuid = "TEST_AGENT_THRESHOLD";
        let workspace_root = setup_test_workspace(&temp_dir, agent_uuid);
        
        eprintln!("[TEST] Workspace root: {}", workspace_root.display());
        
        // 2. Insert a main agent record
        conn.execute(
            "INSERT INTO ai_agents (id, name, slug, is_default, is_active, workspace_path, runtime_ref) 
             VALUES ('agent1', 'Test Agent', 'test', 1, 1, ?1, 'test-main')",
            [agent_uuid],
        ).unwrap();
        
        eprintln!("[TEST] Created main agent record");
        
        // 3. Insert EXACTLY ONE unrouted fact (this was the broken case with old threshold=5)
        conn.execute(
            "INSERT INTO ai_memory (id, fact, category, is_active, routed_at) 
             VALUES ('fact-single', 'User prefers concise responses', 'preference', 1, NULL)",
            [],
        )
        .unwrap();
        
        eprintln!("[TEST] Inserted 1 unrouted fact");
        
        // 4. Verify exactly 1 unrouted fact exists
        let unrouted_count: i64 = conn
            .query_row(
            "SELECT COUNT(*) FROM ai_memory WHERE routed_at IS NULL AND is_active = 1",
            [],
            |row| row.get(0),
            )
            .unwrap();
        
        eprintln!("[TEST] Unrouted facts: {}", unrouted_count);
        assert_eq!(unrouted_count, 1, "Should have exactly 1 unrouted fact");
        
        // 5. TEST THE PRODUCTION THRESHOLD CHECK LOGIC
        // Use the EXACT same function as the worker (exported for testing)
        let synthesis_threshold = crate::ai::extraction_worker::get_synthesis_threshold(&conn);
        
        eprintln!("[TEST] Synthesis threshold: {}", synthesis_threshold);
        
        let should_trigger_router = unrouted_count >= synthesis_threshold;
        eprintln!(
            "[TEST] Should trigger router? {} >= {} = {}",
            unrouted_count, synthesis_threshold, should_trigger_router
        );
        
        // THIS ASSERTION FAILS with old threshold=5, PASSES with new threshold=1
        assert!(
            should_trigger_router,
            "FAILED: With 1 unrouted fact, router should trigger (threshold={}). \
             OLD BUG: threshold=5 prevented routing until 5+ facts accumulated.",
            synthesis_threshold
        );
        
        eprintln!("[TEST] ✓ Threshold check logic PASSED");
        
        // 6. Now verify the router actually writes to Memory.md
        use crate::ai::memory_router::{route_facts_to_file, UnroutedFact};
        
        let expected_memory_path = workspace_root
            .join(".pane")
            .join("system")
            .join(agent_uuid)
            .join("MEMORY.md");
        
        eprintln!(
            "[TEST] Expected Memory.md path: {}",
            expected_memory_path.display()
        );
        
        let fact = UnroutedFact {
            id: "fact-single".to_string(),
            category: "preference".to_string(),
            project_id: None,
            fact: "User prefers concise responses".to_string(),
        };
        
        // Call the production routing function
        eprintln!("[TEST] Calling route_facts_to_file (production function)...");
        route_facts_to_file(&conn, &expected_memory_path, &[fact]).expect("Routing should succeed");
        
        eprintln!("[TEST] ✓ Routing succeeded");
        
        // 7. Verify Memory.md was created and contains the fact
        assert!(
            expected_memory_path.exists(),
            "Memory.md should have been created on disk"
        );
        
        let content =
            std::fs::read_to_string(&expected_memory_path).expect("Failed to read Memory.md");
        
        eprintln!("[TEST] Memory.md content:\n{}", content);
        
        assert!(
            content.contains("User prefers concise responses"),
            "Memory.md should contain the extracted fact"
        );
        
        // 8. Verify the fact was marked as routed in the database
        let routed_at: Option<String> = conn
            .query_row(
            "SELECT routed_at FROM ai_memory WHERE id = 'fact-single'",
            [],
            |row| row.get(0),
            )
            .unwrap();
        
        eprintln!("[TEST] Fact routed_at: {:?}", routed_at);
        assert!(
            routed_at.is_some(),
            "Fact should have been marked as routed"
        );
        
        eprintln!("\n[TEST] ═══════════════════════════════════════════════════════════");
        eprintln!("[TEST] ✅ #719 FIX VERIFIED");
        eprintln!("[TEST] ✓ Threshold check: 1 unrouted fact triggers router");
        eprintln!("[TEST] ✓ Router writes fact to Memory.md");
        eprintln!("[TEST] ✓ Database marks fact as routed");
        eprintln!("[TEST] ═══════════════════════════════════════════════════════════\n");
    }
}
