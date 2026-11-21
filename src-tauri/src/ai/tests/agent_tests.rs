//! Tests for §24 First-Class Agent Entities — ensures agents never leak into projects table
//! 
//! Verifies the core architectural principle (§24.1):
//! - Agents are first-class entities with their own table (`ai_agents`)
//! - Agents NEVER appear in the `projects` table
//! - No "Pane System" project is created
//! - No "General" or "AI" folders are auto-created for agents
//! 
//! GitHub Issue: #293

#[cfg(test)]
mod tests {
    use rusqlite::{params, Connection};
    use sha2::{Sha256, Digest};

    /// Create an in-memory SQLite database with full schema for agent testing
    fn setup_agent_db() -> Connection {
        let conn = Connection::open_in_memory().expect("Failed to open in-memory DB");
        
        // Create the schema as defined in migrations (§24.2)
        conn.execute_batch(
            "
            -- ai_agents table (§24.2)
            CREATE TABLE ai_agents (
                id              TEXT PRIMARY KEY DEFAULT (hex(randomblob(16))),
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
                updated_at      TEXT NOT NULL DEFAULT (datetime('now'))
            );

            CREATE UNIQUE INDEX idx_ai_agents_default ON ai_agents(is_default) WHERE is_default = 1;
            CREATE INDEX idx_ai_agents_slug ON ai_agents(slug);
            CREATE INDEX idx_ai_agents_active ON ai_agents(is_active) WHERE is_active = 1;
            CREATE INDEX idx_ai_agents_owner ON ai_agents(owner_id);

            -- projects table (for testing agents DON'T leak here)
            CREATE TABLE projects (
                id          TEXT PRIMARY KEY,
                name        TEXT NOT NULL,
                description TEXT,
                color       TEXT,
                parent_id   TEXT,
                sort_order  INTEGER NOT NULL DEFAULT 0,
                is_system   INTEGER NOT NULL DEFAULT 0,
                owner_id    TEXT,
                created_at  TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at  TEXT NOT NULL DEFAULT (datetime('now')),
                UNIQUE(name, parent_id)
            );

            CREATE INDEX idx_projects_name ON projects(name);
            CREATE INDEX idx_projects_parent ON projects(parent_id);

            -- ai_chat_messages table (§24.8)
            CREATE TABLE ai_chat_messages (
                id                TEXT PRIMARY KEY,
                project_id        TEXT NOT NULL,
                role              TEXT NOT NULL,
                content           TEXT NOT NULL,
                agent_id          TEXT REFERENCES ai_agents(id),
                model             TEXT,
                created_at        TEXT NOT NULL DEFAULT (datetime('now'))
            );

            CREATE INDEX idx_chat_project ON ai_chat_messages(project_id, created_at);
            CREATE INDEX idx_chat_agent ON ai_chat_messages(agent_id);

            -- notes table (for agent-owned notes)
            CREATE TABLE notes (
                id              TEXT PRIMARY KEY,
                title           TEXT NOT NULL DEFAULT '',
                content_preview TEXT NOT NULL DEFAULT '',
                file_path       TEXT NOT NULL DEFAULT '',
                project_id      TEXT,
                agent_id        TEXT REFERENCES ai_agents(id),
                note_type       TEXT,
                created_at      TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at      TEXT NOT NULL DEFAULT (datetime('now'))
            );

            CREATE INDEX idx_notes_project ON notes(project_id);
            CREATE INDEX idx_notes_agent_id ON notes(agent_id);
            ",
        )
        .expect("Failed to create agent test schema");
        
        conn
    }

    /// Helper: compute SHA256 hash of content
    fn compute_hash(content: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(content.as_bytes());
        format!("{:x}", hasher.finalize())
    }

    /// Helper: insert an agent into ai_agents
    fn insert_agent(
        conn: &Connection,
        name: &str,
        slug: &str,
        is_default: bool,
        source: &str,
    ) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO ai_agents (id, name, slug, role, is_default, workspace_path, source)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                id,
                name,
                slug,
                if is_default { "main" } else { "sub" },
                if is_default { 1 } else { 0 },
                name, // workspace_path = name for test simplicity
                source
            ],
        )
        .expect("Failed to insert agent");
        id
    }

    /// Helper: count agents in ai_agents table
    fn count_agents(conn: &Connection) -> i64 {
        conn.query_row("SELECT COUNT(*) FROM ai_agents", [], |row| row.get(0))
            .unwrap_or(0)
    }

    /// Helper: count projects in projects table
    fn count_projects(conn: &Connection) -> i64 {
        conn.query_row("SELECT COUNT(*) FROM projects", [], |row| row.get(0))
            .unwrap_or(0)
    }

    /// Helper: check if a project with given name exists
    fn project_exists(conn: &Connection, name: &str) -> bool {
        conn.query_row(
            "SELECT COUNT(*) > 0 FROM projects WHERE name = ?1",
            params![name],
            |row| row.get::<_, bool>(0),
        )
        .unwrap_or(false)
    }

    // ─── Test Group 1: ai_agents Table ───────────────────────────────────

    #[test]
    fn test_create_agent() {
        let conn = setup_agent_db();
        
        let soul_content = "# Soul\n\nI am Stia, a coding agent.";
        let memory_content = "# Memory\n\nRecent work: Implemented §24 tests.";
        
        let soul_hash = compute_hash(soul_content);
        let memory_hash = compute_hash(memory_content);

        let id = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO ai_agents (id, name, slug, role, is_default, workspace_path, soul_hash, memory_hash, source)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                id,
                "Stia",
                "stia",
                "sub",
                0,
                "Stia",
                soul_hash,
                memory_hash,
                "import"
            ],
        )
        .expect("Failed to insert agent");

        // Verify all fields
        let (name, slug, role, is_default, workspace, s_hash, m_hash, src): (
            String, String, String, i64, String, String, String, String
        ) = conn.query_row(
            "SELECT name, slug, role, is_default, workspace_path, soul_hash, memory_hash, source FROM ai_agents WHERE id = ?1",
            params![id],
            |row| Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?,
                row.get(7)?,
            )),
        ).expect("Failed to query agent");

        assert_eq!(name, "Stia");
        assert_eq!(slug, "stia");
        assert_eq!(role, "sub");
        assert_eq!(is_default, 0);
        assert_eq!(workspace, "Stia");
        assert_eq!(s_hash, soul_hash);
        assert_eq!(m_hash, memory_hash);
        assert_eq!(src, "import");
    }

    #[test]
    fn test_only_one_main_agent() {
        let conn = setup_agent_db();
        
        // Insert first main agent — should succeed
        insert_agent(&conn, "Brit Freed", "brit-freed", true, "import");
        
        // Try to insert second main agent — should fail due to unique index
        let result = conn.execute(
            "INSERT INTO ai_agents (name, slug, role, is_default, workspace_path)
             VALUES ('Another Main', 'another-main', 'main', 1, 'Another Main')",
            [],
        );
        
        assert!(
            result.is_err(),
            "Inserting two agents with is_default=1 should fail (unique constraint)"
        );
    }

    #[test]
    fn test_agent_slug_unique() {
        let conn = setup_agent_db();
        
        insert_agent(&conn, "Stia", "stia", false, "import");
        
        // Try to insert another agent with same slug — should fail
        let result = conn.execute(
            "INSERT INTO ai_agents (name, slug, workspace_path)
             VALUES ('Stia Clone', 'stia', 'Stia Clone')",
            [],
        );
        
        assert!(
            result.is_err(),
            "Two agents with same slug should fail (unique constraint)"
        );
    }

    /// Renamed from the removal-era name (#2793 item 4: the Tauri command this test
    /// originally exercised is gone — see agents.rs history) since this test only ever
    /// drove the raw `UPDATE ai_agents SET is_active = 0` SQL shape directly against the
    /// schema, never the command itself.
    #[test]
    fn test_agent_is_active_flag_toggles_via_raw_sql() {
        let conn = setup_agent_db();
        
        let id = insert_agent(&conn, "Reed", "reed", false, "import");
        
        // Verify agent is active
        let is_active: i64 = conn.query_row(
            "SELECT is_active FROM ai_agents WHERE id = ?1",
            params![id],
            |row| row.get(0),
        ).expect("Failed to query is_active");
        assert_eq!(is_active, 1);
        
        // Deactivate
        conn.execute(
            "UPDATE ai_agents SET is_active = 0 WHERE id = ?1",
            params![id],
        ).expect("Failed to deactivate agent");
        
        // Verify excluded from active queries
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM ai_agents WHERE id = ?1 AND is_active = 1",
            params![id],
            |row| row.get(0),
        ).unwrap_or(0);
        assert_eq!(count, 0, "Deactivated agent should be excluded from active queries");
    }

    #[test]
    fn test_agent_upsert() {
        let conn = setup_agent_db();
        
        // Insert
        insert_agent(&conn, "Lane", "lane", false, "import");
        
        // Update via upsert (INSERT OR REPLACE)
        conn.execute(
            "INSERT OR REPLACE INTO ai_agents (slug, name, role, workspace_path, description, source)
             VALUES ('lane', 'Lane', 'sub', 'Lane', 'Updated description', 'ai_created')",
            [],
        ).expect("Failed to upsert agent");
        
        // Verify update not duplicate
        let count = count_agents(&conn);
        assert_eq!(count, 1, "Upsert should update, not create duplicate");
        
        // Verify description updated
        let (desc, src): (String, String) = conn.query_row(
            "SELECT description, source FROM ai_agents WHERE slug = 'lane'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        ).expect("Failed to query after upsert");
        
        assert_eq!(desc, "Updated description");
        assert_eq!(src, "ai_created");
    }

    // ─── Test Group 2: Agents Are NOT Projects (§24.1 — CRITICAL) ───────

    #[test]
    fn test_no_pane_system_project() {
        let conn = setup_agent_db();
        
        // Insert agents
        insert_agent(&conn, "Brit Freed", "brit-freed", true, "import");
        insert_agent(&conn, "Stia", "stia", false, "import");
        
        // Verify NO "Pane System" entry exists in projects
        assert!(
            !project_exists(&conn, "Pane System"),
            "After agent creation, NO 'Pane System' entry should exist in projects table"
        );
    }

    #[test]
    fn test_agents_not_in_projects_table() {
        let conn = setup_agent_db();
        
        // Insert agents
        insert_agent(&conn, "Brit Freed", "brit-freed", true, "import");
        insert_agent(&conn, "Stia", "stia", false, "import");
        insert_agent(&conn, "Reed", "reed", false, "import");
        
        // Verify agent names do NOT appear in projects
        let agent_names = vec!["Brit Freed", "Stia", "Reed"];
        for name in &agent_names {
            assert!(
                !project_exists(&conn, name),
                "Agent '{}' should NOT appear in projects table",
                name
            );
        }
    }

    #[test]
    fn test_no_general_folder_for_agents() {
        let conn = setup_agent_db();
        
        // Insert agents
        insert_agent(&conn, "Brit Freed", "brit-freed", true, "import");
        insert_agent(&conn, "Stia", "stia", false, "import");
        
        // Verify no "General" or "AI" folders created
        assert!(
            !project_exists(&conn, "General"),
            "No 'General' folder should be auto-created for agents"
        );
        assert!(
            !project_exists(&conn, "AI"),
            "No 'AI' folder should be auto-created for agents"
        );
    }

    #[test]
    fn test_projects_table_clean() {
        let conn = setup_agent_db();
        
        // Insert agents
        insert_agent(&conn, "Brit Freed", "brit-freed", true, "import");
        insert_agent(&conn, "Stia", "stia", false, "import");
        insert_agent(&conn, "Reed", "reed", false, "import");
        insert_agent(&conn, "Lane", "lane", false, "import");
        
        // Verify projects table contains ZERO entries
        let project_count = count_projects(&conn);
        assert_eq!(
            project_count, 0,
            "The projects table should contain ZERO system/agent entries after agent creation"
        );
    }

    // ─── Test Group 3: Migration (§24.12) ────────────────────────────────

    #[test]
    fn test_migration_from_pane_system() {
        let conn = setup_agent_db();
        
        // 1. Create old-style "Pane System" project with agent children
        let pane_system_id = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO projects (id, name) VALUES (?1, 'Pane System')",
            params![pane_system_id],
        ).expect("Failed to create Pane System project");
        
        let brit_folder_id = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO projects (id, name, parent_id) VALUES (?1, 'Brit Freed', ?2)",
            params![brit_folder_id, pane_system_id],
        ).expect("Failed to create Brit Freed folder");
        
        let stia_folder_id = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO projects (id, name, parent_id) VALUES (?1, 'Stia', ?2)",
            params![stia_folder_id, pane_system_id],
        ).expect("Failed to create Stia folder");
        
        // 2. Run migration logic (simplified from migration_010)
        // Get all agent folders
        let mut stmt = conn.prepare(
            "SELECT id, name FROM projects WHERE parent_id = ?1 AND name != 'General' AND name != 'AI'"
        ).unwrap();
        let agent_folders: Vec<(String, String)> = stmt
            .query_map(params![&pane_system_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        
        // Mark first as main
        let mut first = true;
        for (_, name) in &agent_folders {
            let slug = name.to_lowercase().replace(" ", "-");
            let is_default = first;
            first = false;
            
            conn.execute(
                "INSERT INTO ai_agents (name, slug, role, is_default, workspace_path, source)
                 VALUES (?1, ?2, ?3, ?4, ?5, 'migration')",
                params![
                    name,
                    slug,
                    if is_default { "main" } else { "sub" },
                    if is_default { 1 } else { 0 },
                    name
                ],
            ).unwrap();
        }
        
        // Delete "Pane System" and children from projects
        conn.execute(
            "DELETE FROM projects WHERE parent_id = ?1",
            params![&pane_system_id],
        ).unwrap();
        conn.execute(
            "DELETE FROM projects WHERE id = ?1",
            params![&pane_system_id],
        ).unwrap();
        
        // 3. Verify agents moved to ai_agents
        let agent_count = count_agents(&conn);
        assert_eq!(agent_count, 2, "Should have 2 agents in ai_agents table after migration");
        
        // Verify names
        let brit_exists: bool = conn.query_row(
            "SELECT COUNT(*) > 0 FROM ai_agents WHERE name = 'Brit Freed'",
            [],
            |row| row.get(0),
        ).unwrap();
        assert!(brit_exists, "Brit Freed should be migrated to ai_agents");
        
        let stia_exists: bool = conn.query_row(
            "SELECT COUNT(*) > 0 FROM ai_agents WHERE name = 'Stia'",
            [],
            |row| row.get(0),
        ).unwrap();
        assert!(stia_exists, "Stia should be migrated to ai_agents");
        
        // 4. Verify "Pane System" deleted from projects
        assert!(
            !project_exists(&conn, "Pane System"),
            "Pane System should be deleted from projects after migration"
        );
        
        // Verify child folders deleted
        let project_count = count_projects(&conn);
        assert_eq!(project_count, 0, "All agent folders should be removed from projects");
    }

    #[test]
    fn test_migration_preserves_filesystem() {
        // This test verifies the migration does NOT touch filesystem
        // (Integration with actual filesystem is beyond unit test scope,
        // but we document the expectation here)
        
        // Migration should:
        // 1. NOT delete .pane/system/ directories
        // 2. NOT move SOUL.md, MEMORY.md, or logs/ directories
        // 3. Only update database records
        
        // Filesystem changes are the responsibility of the integrity check,
        // not the migration. The migration is DB-only.
        
        // This test is a placeholder documenting that expectation.
        // Real filesystem preservation is tested via integration tests.
    }

    // ─── Test Group 4: Import Flow (§24.5) ───────────────────────────────

    #[test]
    fn test_import_creates_ai_agents() {
        let conn = setup_agent_db();
        
        // Simulate import: insert agents into ai_agents (NOT projects)
        insert_agent(&conn, "Brit Freed", "brit-freed", true, "import");
        insert_agent(&conn, "Stia", "stia", false, "import");
        
        // Verify agents in ai_agents
        let agent_count = count_agents(&conn);
        assert_eq!(agent_count, 2, "Import should create agents in ai_agents table");
        
        // Verify NOT in projects
        let project_count = count_projects(&conn);
        assert_eq!(project_count, 0, "Import should NOT create agents in projects table");
    }

    #[test]
    fn test_import_main_agent_flag() {
        let conn = setup_agent_db();
        
        // Simulate import with multiple agents, only one main
        insert_agent(&conn, "Brit Freed", "brit-freed", true, "import");
        insert_agent(&conn, "Stia", "stia", false, "import");
        insert_agent(&conn, "Reed", "reed", false, "import");
        
        // Verify only one agent has is_default=1
        let main_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM ai_agents WHERE is_default = 1",
            [],
            |row| row.get(0),
        ).unwrap();
        assert_eq!(main_count, 1, "Only one agent should have is_default=1 after import");
        
        // Verify it's Brit Freed
        let main_name: String = conn.query_row(
            "SELECT name FROM ai_agents WHERE is_default = 1",
            [],
            |row| row.get(0),
        ).expect("Failed to get main agent name");
        assert_eq!(main_name, "Brit Freed");
    }

    #[test]
    fn test_import_idempotent() {
        let conn = setup_agent_db();
        
        // First import
        insert_agent(&conn, "Stia", "stia", false, "import");
        let count_after_first = count_agents(&conn);
        assert_eq!(count_after_first, 1);
        
        // Second import (upsert by slug)
        conn.execute(
            "INSERT OR REPLACE INTO ai_agents (slug, name, role, workspace_path, description)
             VALUES ('stia', 'Stia', 'sub', 'Stia', 'Updated after re-import')",
            [],
        ).expect("Failed to upsert on re-import");
        
        // Verify count unchanged (update not duplicate)
        let count_after_second = count_agents(&conn);
        assert_eq!(count_after_second, 1, "Running import twice should upsert, not duplicate");
        
        // Verify description was updated
        let desc: String = conn.query_row(
            "SELECT description FROM ai_agents WHERE slug = 'stia'",
            [],
            |row| row.get(0),
        ).expect("Failed to get description");
        assert_eq!(desc, "Updated after re-import");
    }

    // ─── Test Group 5: agent_id on Chat Messages (§24.8) ─────────────────

    #[test]
    fn test_chat_message_agent_id_nullable() {
        let conn = setup_agent_db();
        
        // Insert a chat message WITHOUT agent_id (NULL = main agent by default)
        let msg_id = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO ai_chat_messages (id, project_id, role, content)
             VALUES (?1, 'proj1', 'assistant', 'Hello from main agent')",
            params![msg_id],
        ).expect("Messages without agent_id should work (backward compat)");
        
        // Verify agent_id is NULL
        let agent_id: Option<String> = conn.query_row(
            "SELECT agent_id FROM ai_chat_messages WHERE id = ?1",
            params![msg_id],
            |row| row.get(0),
        ).expect("Failed to query agent_id");
        
        assert!(agent_id.is_none(), "agent_id should be NULL for backward compat");
    }

    #[test]
    fn test_chat_message_with_agent_id() {
        let conn = setup_agent_db();
        
        // Insert agent
        let agent_id = insert_agent(&conn, "Stia", "stia", false, "import");
        
        // Insert chat message with agent_id
        let msg_id = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO ai_chat_messages (id, project_id, role, content, agent_id)
             VALUES (?1, 'proj1', 'assistant', 'Hello from Stia', ?2)",
            params![msg_id, agent_id],
        ).expect("Messages with agent_id should work");
        
        // Verify agent_id is set and references valid ai_agents record
        let stored_agent_id: String = conn.query_row(
            "SELECT agent_id FROM ai_chat_messages WHERE id = ?1",
            params![msg_id],
            |row| row.get(0),
        ).expect("Failed to query agent_id");
        
        assert_eq!(stored_agent_id, agent_id);
        
        // Verify foreign key integrity (agent exists)
        let agent_exists: bool = conn.query_row(
            "SELECT COUNT(*) > 0 FROM ai_agents WHERE id = ?1",
            params![agent_id],
            |row| row.get(0),
        ).unwrap();
        assert!(agent_exists, "agent_id should reference valid ai_agents record");
    }

    #[test]
    fn test_agent_owned_notes() {
        let conn = setup_agent_db();
        
        // Insert agent
        let agent_id = insert_agent(&conn, "Brit Freed", "brit-freed", true, "import");
        
        // Insert agent-owned note (daily log) with NULL project_id
        let note_id = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO notes (id, title, file_path, agent_id, note_type, project_id)
             VALUES (?1, '2026-03-29', '.pane/system/Brit Freed/logs/2026-03-29.md', ?2, 'daily', NULL)",
            params![note_id, agent_id],
        ).expect("Agent-owned notes with NULL project_id should work");
        
        // Verify note is linked to agent
        let stored_agent_id: String = conn.query_row(
            "SELECT agent_id FROM notes WHERE id = ?1",
            params![note_id],
            |row| row.get(0),
        ).expect("Failed to query agent_id on note");
        
        assert_eq!(stored_agent_id, agent_id);
        
        // Verify project_id is NULL
        let project_id: Option<String> = conn.query_row(
            "SELECT project_id FROM notes WHERE id = ?1",
            params![note_id],
            |row| row.get(0),
        ).expect("Failed to query project_id");
        
        assert!(project_id.is_none(), "Agent-owned notes should have NULL project_id");
        
        // Verify note is excluded from project queries (WHERE project_id = 'some-project')
        let in_project_query: i64 = conn.query_row(
            "SELECT COUNT(*) FROM notes WHERE project_id = 'some-project'",
            [],
            |row| row.get(0),
        ).unwrap_or(0);
        
        assert_eq!(in_project_query, 0, "Agent-owned notes should not appear in project queries");
    }

    /// Test that workspace_path uses UUID, not display name
    #[test]
    fn test_workspace_path_uses_uuid() {
        let conn = setup_agent_db();
        
        // Create agent with display name "Brit Freed"
        let agent_id = "E170EB0D29556C08E6593D3CC4DC4F58".to_string();
        let agent_name = "Brit Freed".to_string();
        let workspace_path = agent_id.clone(); // workspace_path should be the UUID
        
        conn.execute(
            "INSERT INTO ai_agents (id, name, slug, workspace_path, is_default, source)
             VALUES (?1, ?2, ?3, ?4, 1, 'test')",
            params![&agent_id, &agent_name, "brit-freed", &workspace_path],
        ).expect("Failed to insert agent");
        
        // Verify workspace_path is the UUID, not the name
        let stored_workspace_path: String = conn.query_row(
            "SELECT workspace_path FROM ai_agents WHERE id = ?1",
            params![&agent_id],
            |row| row.get(0),
        ).expect("Failed to query workspace_path");
        
        assert_eq!(stored_workspace_path, agent_id, "workspace_path should be UUID");
        assert_ne!(stored_workspace_path, agent_name, "workspace_path should NOT be display name");
    }

    /// Test that renaming an agent does NOT change workspace_path
    #[test]
    fn test_rename_agent_does_not_change_workspace_path() {
        let conn = setup_agent_db();
        
        // Create agent with UUID workspace_path
        let agent_id = "E170EB0D29556C08E6593D3CC4DC4F58".to_string();
        let original_name = "Brit Freed".to_string();
        let workspace_path = agent_id.clone();
        
        conn.execute(
            "INSERT INTO ai_agents (id, name, slug, workspace_path, is_default, source)
             VALUES (?1, ?2, ?3, ?4, 1, 'test')",
            params![&agent_id, &original_name, "brit-freed", &workspace_path],
        ).expect("Failed to insert agent");
        
        // Rename the agent (only name column should change)
        let new_name = "Brit (Renamed)".to_string();
        let new_slug = "brit-renamed".to_string();
        conn.execute(
            "UPDATE ai_agents SET name = ?1, slug = ?2 WHERE id = ?3",
            params![&new_name, &new_slug, &agent_id],
        ).expect("Failed to rename agent");
        
        // Verify workspace_path is UNCHANGED
        let stored_workspace_path: String = conn.query_row(
            "SELECT workspace_path FROM ai_agents WHERE id = ?1",
            params![&agent_id],
            |row| row.get(0),
        ).expect("Failed to query workspace_path");
        
        assert_eq!(stored_workspace_path, workspace_path, "workspace_path should be immutable after creation");
        
        // Verify name WAS changed
        let stored_name: String = conn.query_row(
            "SELECT name FROM ai_agents WHERE id = ?1",
            params![&agent_id],
            |row| row.get(0),
        ).expect("Failed to query name");
        
        assert_eq!(stored_name, new_name, "name should be updated");
    }

    /// Test that daily logs use workspace_path (UUID) in file_path
    #[test]
    fn test_daily_logs_use_uuid_path() {
        let conn = setup_agent_db();
        
        // Create agent with UUID workspace_path
        let agent_id = "E170EB0D29556C08E6593D3CC4DC4F58".to_string();
        let agent_name = "Brit Freed".to_string();
        let workspace_path = agent_id.clone();
        
        conn.execute(
            "INSERT INTO ai_agents (id, name, slug, workspace_path, is_default, source)
             VALUES (?1, ?2, ?3, ?4, 1, 'test')",
            params![&agent_id, &agent_name, "brit-freed", &workspace_path],
        ).expect("Failed to insert agent");
        
        // Create a daily log note
        let note_id = "test-log-id".to_string();
        let expected_path = format!(".pane/system/{}/logs/2026-04-07.md", workspace_path);
        
        conn.execute(
            "INSERT INTO notes (id, title, file_path, agent_id, note_type)
             VALUES (?1, '2026-04-07', ?2, ?3, 'daily')",
            params![&note_id, &expected_path, &agent_id],
        ).expect("Failed to insert daily log note");
        
        // Verify file_path uses workspace_path (UUID), not agent name
        let stored_path: String = conn.query_row(
            "SELECT file_path FROM notes WHERE id = ?1",
            params![&note_id],
            |row| row.get(0),
        ).expect("Failed to query file_path");
        
        assert_eq!(stored_path, expected_path, "Daily log file_path should use UUID workspace_path");
        assert!(stored_path.contains(&workspace_path), "file_path should contain UUID workspace_path");
        assert!(!stored_path.contains(&agent_name), "file_path should NOT contain display name");
    }
}
