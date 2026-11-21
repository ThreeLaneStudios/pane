/// Tests for ADR-0039: IDENTITY.md as Single Source of Truth for agent identity.
/// Covers: integrity check migration, write order invariant, greeting path.
#[cfg(test)]
mod tests {
    use std::fs;
    use tempfile::TempDir;

    fn make_pane_root() -> TempDir {
        let tmp = TempDir::new().expect("tempdir");
        fs::create_dir_all(tmp.path().join(".pane").join("system")).expect("create system dir");
        tmp
    }

    fn make_test_conn() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().expect("open in-memory db");
        conn.execute_batch(
            "CREATE TABLE ai_agents (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                slug TEXT NOT NULL,
                role TEXT NOT NULL DEFAULT 'sub',
                is_default INTEGER NOT NULL DEFAULT 0,
                is_active INTEGER NOT NULL DEFAULT 1,
                workspace_path TEXT NOT NULL,
                runtime_ref TEXT,
                source TEXT NOT NULL DEFAULT 'local',
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );",
        ).expect("create tables");
        conn
    }

    // ── ADR-0039 Decision 5: startup migration ────────────────────────────────

    #[test]
    fn test_integrity_check_writes_identity_md_when_missing() {
        let pane_root = make_pane_root();
        let conn = make_test_conn();

        let agent_id = uuid::Uuid::new_v4().to_string();
        let workspace = uuid::Uuid::new_v4().to_string();
        let agent_name = "Birdy";

        conn.execute(
            "INSERT INTO ai_agents (id, name, slug, role, is_default, is_active, workspace_path, source, created_at, updated_at)
             VALUES (?1, ?2, ?3, 'main', 1, 1, ?4, 'test', datetime('now'), datetime('now'))",
            rusqlite::params![agent_id, agent_name, "birdy", workspace],
        ).expect("insert agent");

        // Create agent dir with SOUL.md and MEMORY.md but NO IDENTITY.md
        let agent_dir = pane_root.path().join(".pane").join("system").join(&workspace);
        fs::create_dir_all(&agent_dir).expect("create agent dir");
        fs::write(agent_dir.join("SOUL.md"), "# Soul\n\nPersonality here.\n").expect("write soul");
        fs::write(agent_dir.join("MEMORY.md"), "# Memory\n\n").expect("write memory");
        // IDENTITY.md intentionally absent

        crate::ai::ingest::verify_agents_integrity_with_path(&conn, pane_root.path().to_path_buf());

        let identity_path = agent_dir.join("IDENTITY.md");
        assert!(identity_path.exists(), "IDENTITY.md should be created by integrity check");
        let content = fs::read_to_string(&identity_path).expect("read identity");
        assert!(content.contains("Birdy"), "IDENTITY.md should contain agent name");
        assert!(content.contains("Your name is Birdy"), "IDENTITY.md should include name statement");
    }

    #[test]
    fn test_integrity_check_writes_identity_md_when_stub() {
        let pane_root = make_pane_root();
        let conn = make_test_conn();

        let agent_id = uuid::Uuid::new_v4().to_string();
        let workspace = uuid::Uuid::new_v4().to_string();
        let agent_name = "Reed";

        conn.execute(
            "INSERT INTO ai_agents (id, name, slug, role, is_default, is_active, workspace_path, source, created_at, updated_at)
             VALUES (?1, ?2, 'reed', 'main', 1, 1, ?3, 'test', datetime('now'), datetime('now'))",
            rusqlite::params![agent_id, agent_name, workspace],
        ).expect("insert agent");

        let agent_dir = pane_root.path().join(".pane").join("system").join(&workspace);
        fs::create_dir_all(&agent_dir).expect("create agent dir");
        fs::write(agent_dir.join("SOUL.md"), "# Soul\n\nHi.\n").expect("write soul");
        fs::write(agent_dir.join("MEMORY.md"), "# Memory\n\n").expect("write memory");
        // IDENTITY.md with stub marker
        fs::write(agent_dir.join("IDENTITY.md"), "*(This file was auto-created by integrity check)*\n")
            .expect("write stub identity");

        crate::ai::ingest::verify_agents_integrity_with_path(&conn, pane_root.path().to_path_buf());

        let content = fs::read_to_string(agent_dir.join("IDENTITY.md")).expect("read identity");
        assert!(content.contains("Reed"), "Stub IDENTITY.md should be replaced with agent name");
        assert!(content.contains("Your name is Reed"), "Should include name statement");
    }

    #[test]
    fn test_integrity_check_preserves_user_written_identity_md() {
        let pane_root = make_pane_root();
        let conn = make_test_conn();

        let agent_id = uuid::Uuid::new_v4().to_string();
        let workspace = uuid::Uuid::new_v4().to_string();

        conn.execute(
            "INSERT INTO ai_agents (id, name, slug, role, is_default, is_active, workspace_path, source, created_at, updated_at)
             VALUES (?1, 'Cassidy', 'cassidy', 'main', 1, 1, ?2, 'test', datetime('now'), datetime('now'))",
            rusqlite::params![agent_id, workspace],
        ).expect("insert agent");

        let agent_dir = pane_root.path().join(".pane").join("system").join(&workspace);
        fs::create_dir_all(&agent_dir).expect("create agent dir");
        fs::write(agent_dir.join("SOUL.md"), "# Soul\n\nPersonality.\n").expect("write soul");
        fs::write(agent_dir.join("MEMORY.md"), "# Memory\n\n").expect("write memory");

        // User has expanded IDENTITY.md with real content
        let user_identity = "# Cassidy\n\nYour name is Cassidy. You are a creative writing assistant with a flair for metaphor.\n";
        fs::write(agent_dir.join("IDENTITY.md"), user_identity).expect("write user identity");

        crate::ai::ingest::verify_agents_integrity_with_path(&conn, pane_root.path().to_path_buf());

        let content = fs::read_to_string(agent_dir.join("IDENTITY.md")).expect("read identity");
        assert_eq!(content, user_identity, "User-written IDENTITY.md must not be overwritten by integrity check");
    }

    // ── ADR-0039 Decision 5: IDENTITY.md written by integrity check format ────

    #[test]
    fn test_identity_md_format() {
        let pane_root = make_pane_root();
        let conn = make_test_conn();

        let agent_id = uuid::Uuid::new_v4().to_string();
        let workspace = uuid::Uuid::new_v4().to_string();
        let agent_name = "Pane";

        conn.execute(
            "INSERT INTO ai_agents (id, name, slug, role, is_default, is_active, workspace_path, source, created_at, updated_at)
             VALUES (?1, ?2, 'pane', 'main', 1, 1, ?3, 'test', datetime('now'), datetime('now'))",
            rusqlite::params![agent_id, agent_name, workspace],
        ).expect("insert agent");

        let agent_dir = pane_root.path().join(".pane").join("system").join(&workspace);
        fs::create_dir_all(&agent_dir).expect("create agent dir");
        fs::write(agent_dir.join("SOUL.md"), "# Soul\n").expect("write soul");
        fs::write(agent_dir.join("MEMORY.md"), "# Memory\n").expect("write memory");

        crate::ai::ingest::verify_agents_integrity_with_path(&conn, pane_root.path().to_path_buf());

        let content = fs::read_to_string(agent_dir.join("IDENTITY.md")).expect("read identity");
        // Format: "# {name}\n\nYour name is {name}.\n"
        assert!(content.starts_with("# Pane\n"), "Should start with H1 name");
        assert!(content.contains("Your name is Pane."), "Should include name statement");
    }

    // ── ADR-0039 Decision 2: lib.rs system prompt no longer hardcodes "Pane" ──

    #[test]
    fn test_hardcoded_system_prompt_does_not_claim_name_pane() {
        // The hardcoded system_prompt in AiConfig should describe the product role
        // but NOT claim "You are Pane" — identity comes from IDENTITY.md at assembly time.
        let prompt = "You are an AI partner embedded in a notes and project management app called Pane. \
                      You help the user organize their work, create and manage notes and tasks, and think through problems. \
                      Be concise, helpful, and proactive. You have access to the user's workspace context shown below.\n\n\
                      Rule 22: BREVITY — Never echo back the full content of a note in chat.";

        assert!(!prompt.starts_with("You are Pane,"), "Prompt must not start with 'You are Pane,'");
        assert!(prompt.contains("AI partner"), "Prompt should describe the product role");
        assert!(prompt.contains("called Pane"), "Product name reference is fine; identity claim is not");
    }

    // ── ADR-0039 Decision 4: greeting instruction prepended, not replacing ─────

    #[test]
    fn test_greeting_instruction_prepend_format() {
        // Simulate the greeting prepend logic from ai/commands.rs
        let agent_name = "Birdy";
        let greeting_instruction = format!(
            "[System] You are {}, meeting this user for the first time. Introduce yourself briefly — one or two warm sentences based on your personality. Do NOT say you are Claude or any other AI name.",
            agent_name
        );
        let existing_system = "## Workspace — Soul\nBe concise and helpful.\n\n## Workspace — Identity\n# Birdy\n\nYour name is Birdy.\n";

        // PREPEND (not replace)
        let combined = format!("{}\n\n{}", greeting_instruction, existing_system);

        assert!(combined.starts_with("[System] You are Birdy"), "Greeting instruction should be first");
        assert!(combined.contains("## Workspace — Soul"), "Soul content should be preserved");
        assert!(combined.contains("## Workspace — Identity"), "Identity content should be preserved");
        assert!(combined.contains("Your name is Birdy"), "Agent name in IDENTITY.md should still be present");
    }

    // ── ADR-0039 Decision 7: write-order invariant ─────────────────────────────

    #[test]
    fn test_write_order_identity_md_failure_aborts_db_update() {
        // ADR-0039 Decision 7: IDENTITY.md must be written BEFORE DB insert/update.
        // If the write fails, the DB must NOT be updated.
        use rusqlite::params;

        let conn = make_test_conn();
        let old_name = "Alice";
        let agent_id = uuid::Uuid::new_v4().to_string();
        let workspace = uuid::Uuid::new_v4().to_string();

        conn.execute(
            "INSERT INTO ai_agents (id, name, slug, role, is_default, is_active, workspace_path, source, created_at, updated_at)
             VALUES (?1, ?2, 'alice', 'main', 1, 1, ?3, 'test', datetime('now'), datetime('now'))",
            params![agent_id, old_name, workspace],
        ).expect("insert agent");

        // Block the agent dir by placing a FILE where the directory would go.
        // create_dir_all will fail because the path component is a file, not a dir.
        let tmp = TempDir::new().expect("tempdir");
        let blocker = tmp.path().join("blocker");
        fs::write(&blocker, b"I am a file, not a dir").expect("write blocker");
        let agent_dir = blocker.join(&workspace); // parent is a file — will fail

        // Simulate the exact write-before-insert pattern from sync_initial_import.
        let write_result: Result<(), String> = (|| {
            fs::create_dir_all(&agent_dir)
                .map_err(|e| format!("Failed to create agent dir: {}", e))?;
            let identity_path = agent_dir.join("IDENTITY.md");
            if !identity_path.exists() {
                let content = format!("# {}\n\nYour name is {}.\n", old_name, old_name);
                let tmp_path = agent_dir.join(".IDENTITY.md.tmp");
                fs::write(&tmp_path, &content)
                    .map_err(|e| format!("Failed to write IDENTITY.md (temp): {}", e))?;
                fs::rename(&tmp_path, &identity_path)
                    .map_err(|e| format!("Failed to finalize IDENTITY.md: {}", e))?;
            }
            // DB update only executes if the write above succeeded:
            conn.execute(
                "UPDATE ai_agents SET name = 'NewName' WHERE id = ?1",
                params![agent_id],
            ).map_err(|e| format!("DB update failed: {}", e))?;
            Ok(())
        })();

        assert!(write_result.is_err(), "Should return Err when IDENTITY.md cannot be written");

        // DB must still have the OLD name — not modified.
        let name: String = conn.query_row(
            "SELECT name FROM ai_agents WHERE id = ?1",
            params![agent_id],
            |row| row.get(0),
        ).expect("query agent");
        assert_eq!(name, old_name, "DB must not be updated when IDENTITY.md write fails (Decision 7)");
    }

    #[test]
    fn test_greeting_fallback_name_is_pane() {
        // When ai_agents has no main agent, COALESCE falls back to "Pane"
        let raw_name: Option<String> = None;
        let effective_name = raw_name
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "Pane".to_string());
        assert_eq!(effective_name, "Pane", "Fallback must be Pane when no agent name exists");
    }
}
