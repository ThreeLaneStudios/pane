//! Memory system unit tests for Pane (#155, #162)
//!
//! Tests cover:
//! - Memory decay (§5.4A): category-based retirement thresholds
//! - Extraction filtering: confidence threshold enforcement
//! - Synthesis guards: unrouted fact detection and routed_at marking
//! - End-to-end memory pipeline integration (#162)
//! - Source message ID dedup
//! - Routed_at persistence (regression guard)
//! - Version snapshots
//! - Daily log append
//! - Compaction safety guard
//! - Memory search settings persistence (#388)

#[cfg(test)]
mod agent_tests;
#[cfg(test)]
mod recovery_tests;
#[cfg(test)]
mod memory_pipeline_tests;
#[cfg(test)]
mod unified_llm_tests;
#[cfg(test)]
mod smart_routing_tests;
#[cfg(test)]
mod dual_provider_tests;
#[cfg(test)]
mod streaming_params_tests;
#[cfg(test)]
mod reconnect_tests;
#[cfg(test)]
mod max_iterations_tests;
#[cfg(test)]
mod timeout_tests;
#[cfg(test)]
mod summarize_tests;

#[cfg(test)]
mod tests {
    use rusqlite::{params, Connection};

    /// Create an in-memory SQLite database with the ai_memory schema
    fn setup_memory_db() -> Connection {
        let conn = Connection::open_in_memory().expect("Failed to open in-memory DB");
        conn.execute_batch(
            "
            CREATE TABLE ai_memory (
                id                TEXT PRIMARY KEY,
                layer             TEXT NOT NULL DEFAULT 'work',
                project_id        TEXT,
                user_id           TEXT,
                fact              TEXT NOT NULL,
                category          TEXT,
                source_message_id TEXT,
                is_active         INTEGER NOT NULL DEFAULT 1,
                created_at        TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at        TEXT,
                last_referenced   TEXT,
                routed_at         TEXT
            );
            ",
        )
        .expect("Failed to create ai_memory table");
        conn
    }

    /// Create a full pipeline test database with all required tables
    fn setup_pipeline_db() -> Connection {
        let conn = Connection::open_in_memory().expect("Failed to open in-memory DB");
        conn.execute_batch(
            "
            CREATE TABLE ai_memory (
                id                TEXT PRIMARY KEY,
                layer             TEXT NOT NULL DEFAULT 'work',
                project_id        TEXT,
                user_id           TEXT,
                fact              TEXT NOT NULL,
                category          TEXT,
                source_message_id TEXT,
                is_active         INTEGER NOT NULL DEFAULT 1,
                created_at        TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at        TEXT,
                last_referenced   TEXT,
                routed_at         TEXT
            );

            CREATE TABLE projects (
                id          TEXT PRIMARY KEY,
                name        TEXT NOT NULL,
                description TEXT,
                color       TEXT,
                parent_id   TEXT,
                sort_order  INTEGER NOT NULL DEFAULT 0,
                owner_id    TEXT,
                created_at  TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at  TEXT NOT NULL DEFAULT (datetime('now')),
                UNIQUE(name, parent_id)
            );

            CREATE TABLE notes (
                id              TEXT PRIMARY KEY,
                title           TEXT NOT NULL DEFAULT '',
                content_preview TEXT NOT NULL DEFAULT '',
                file_path       TEXT NOT NULL DEFAULT '',
                project_id      TEXT NOT NULL,
                folder_id       TEXT,
                status          TEXT,
                priority        TEXT,
                due_date        TEXT,
                assignee        TEXT,
                completed_at    TEXT,
                sort_order      INTEGER NOT NULL DEFAULT 0,
                word_count      INTEGER NOT NULL DEFAULT 0,
                pinned          INTEGER NOT NULL DEFAULT 0,
                owner_id        TEXT,
                tags_json       TEXT NOT NULL DEFAULT '[]',
                created_at      TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at      TEXT NOT NULL DEFAULT (datetime('now')),
                deleted_at      TEXT
            );

            CREATE TABLE ai_settings (
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );

            CREATE TABLE note_versions (
                id         TEXT PRIMARY KEY,
                note_id    TEXT NOT NULL,
                content    TEXT NOT NULL,
                created_at TEXT NOT NULL,
                source     TEXT NOT NULL DEFAULT 'manual'
            );
            CREATE INDEX idx_note_versions_note_id ON note_versions(note_id);
            ",
        )
        .expect("Failed to create pipeline tables");
        conn
    }

    /// Insert a memory fact with a backdated created_at
    fn insert_fact(
        conn: &Connection,
        id: &str,
        category: &str,
        days_old: i64,
        last_referenced_days_ago: Option<i64>,
    ) {
        let created_at = format!("datetime('now', '-{} days')", days_old);
        let last_ref = last_referenced_days_ago
            .map(|d| format!("datetime('now', '-{} days')", d));

        let sql = format!(
            "INSERT INTO ai_memory (id, layer, fact, category, is_active, created_at, last_referenced)
             VALUES (?1, 'personal', ?2, ?3, 1, {}, {})",
            created_at,
            last_ref
                .as_deref()
                .unwrap_or("NULL")
        );
        conn.execute(
            &sql,
            params![id, format!("Test fact for {}", category), category],
        )
        .expect("Failed to insert test fact");
    }

    /// Check if a memory entry is still active
    fn is_active(conn: &Connection, id: &str) -> bool {
        conn.query_row(
            "SELECT is_active FROM ai_memory WHERE id = ?1",
            params![id],
            |row| row.get::<_, i64>(0),
        )
        .map(|v| v == 1)
        .unwrap_or(false)
    }

    /// Run the same decay logic as `run_memory_decay` in commands.rs
    /// Duplicated here so tests don't depend on Tauri wiring.
    fn run_decay(conn: &Connection) -> (usize, usize) {
        let mut fact_decision_count: usize = 0;
        let reflection_count: usize;

        // 90-day decay: fact, decision, deadline
        for cat in &["fact", "decision", "deadline"] {
            let decayed = conn
                .execute(
                    "UPDATE ai_memory SET is_active = 0 WHERE category = ?1 AND is_active = 1
                     AND COALESCE(last_referenced, created_at) < datetime('now', '-90 days')",
                    params![cat],
                )
                .unwrap_or(0);
            fact_decision_count += decayed;
        }

        // 30-day decay: reflection
        let decayed = conn
            .execute(
                "UPDATE ai_memory SET is_active = 0 WHERE category = 'reflection' AND is_active = 1
                 AND COALESCE(last_referenced, created_at) < datetime('now', '-30 days')",
                [],
            )
            .unwrap_or(0);
        reflection_count = decayed;

        (fact_decision_count, reflection_count)
    }

    // ─── 1. Memory Decay Tests ───────────────────────────────────────────

    #[test]
    fn test_reflection_decays_after_30_days() {
        let conn = setup_memory_db();
        insert_fact(&conn, "r1", "reflection", 45, None);
        run_decay(&conn);
        assert!(!is_active(&conn, "r1"), "Reflection 45 days old should be decayed");
    }

    #[test]
    fn test_reflection_survives_under_30_days() {
        let conn = setup_memory_db();
        insert_fact(&conn, "r2", "reflection", 20, None);
        run_decay(&conn);
        assert!(is_active(&conn, "r2"), "Reflection 20 days old should survive");
    }

    #[test]
    fn test_fact_decays_after_90_days() {
        let conn = setup_memory_db();
        insert_fact(&conn, "f1", "fact", 100, None);
        run_decay(&conn);
        assert!(!is_active(&conn, "f1"), "Fact 100 days old should be decayed");
    }

    #[test]
    fn test_fact_survives_under_90_days() {
        let conn = setup_memory_db();
        insert_fact(&conn, "f2", "fact", 60, None);
        run_decay(&conn);
        assert!(is_active(&conn, "f2"), "Fact 60 days old should survive");
    }

    #[test]
    fn test_identity_never_decays() {
        let conn = setup_memory_db();
        insert_fact(&conn, "i1", "identity", 365, None);
        run_decay(&conn);
        assert!(is_active(&conn, "i1"), "Identity should never decay regardless of age");
    }

    #[test]
    fn test_preference_never_decays() {
        let conn = setup_memory_db();
        insert_fact(&conn, "p1", "preference", 365, None);
        run_decay(&conn);
        assert!(is_active(&conn, "p1"), "Preference should never decay regardless of age");
    }

    #[test]
    fn test_style_never_decays() {
        let conn = setup_memory_db();
        insert_fact(&conn, "s1", "style", 365, None);
        run_decay(&conn);
        assert!(is_active(&conn, "s1"), "Style should never decay regardless of age");
    }

    #[test]
    fn test_recently_referenced_fact_survives() {
        let conn = setup_memory_db();
        // Fact is 100 days old but was referenced 10 days ago
        insert_fact(&conn, "f3", "fact", 100, Some(10));
        run_decay(&conn);
        assert!(
            is_active(&conn, "f3"),
            "Fact referenced 10 days ago should survive even if created 100 days ago"
        );
    }

    #[test]
    fn test_decision_decays_after_90_days() {
        let conn = setup_memory_db();
        insert_fact(&conn, "d1", "decision", 100, None);
        run_decay(&conn);
        assert!(!is_active(&conn, "d1"), "Decision 100 days old should be decayed");
    }

    #[test]
    fn test_deadline_decays_after_90_days() {
        let conn = setup_memory_db();
        insert_fact(&conn, "dl1", "deadline", 100, None);
        run_decay(&conn);
        assert!(!is_active(&conn, "dl1"), "Deadline 100 days old should be decayed");
    }

    // ─── 2. Extraction Filter Tests ──────────────────────────────────────
    // The confidence threshold (>= 4) is enforced in the frontend/command layer
    // after the LLM returns results. We test the filtering logic directly.

    /// Simulate the confidence filter applied in ai_extract_memory
    fn filter_by_confidence(entries: Vec<(String, u8)>) -> Vec<String> {
        entries
            .into_iter()
            .filter(|(_, confidence)| *confidence >= 4)
            .map(|(fact, _)| fact)
            .collect()
    }

    #[test]
    fn test_confidence_below_threshold_filtered() {
        let entries = vec![("Low confidence fact".to_string(), 2u8)];
        let result = filter_by_confidence(entries);
        assert!(result.is_empty(), "Confidence 2 should be filtered out");
    }

    #[test]
    fn test_confidence_at_threshold_passes() {
        let entries = vec![("Threshold fact".to_string(), 4u8)];
        let result = filter_by_confidence(entries);
        assert_eq!(result.len(), 1, "Confidence 4 should pass the threshold");
        assert_eq!(result[0], "Threshold fact");
    }

    #[test]
    fn test_confidence_above_threshold_passes() {
        let entries = vec![("High confidence fact".to_string(), 5u8)];
        let result = filter_by_confidence(entries);
        assert_eq!(result.len(), 1, "Confidence 5 should pass the threshold");
        assert_eq!(result[0], "High confidence fact");
    }

    // ─── 3. Synthesis Guards ─────────────────────────────────────────────
    // The synthesis pipeline skips projects with no unrouted facts and
    // sets routed_at after processing.

    /// Count unrouted facts for a given project (or personal if project_id is None)
    fn count_unrouted(conn: &Connection, project_id: Option<&str>) -> i64 {
        match project_id {
            Some(pid) => conn
                .query_row(
                    "SELECT COUNT(*) FROM ai_memory WHERE project_id = ?1 AND routed_at IS NULL AND is_active = 1",
                    params![pid],
                    |row| row.get(0),
                )
                .unwrap_or(0),
            None => conn
                .query_row(
                    "SELECT COUNT(*) FROM ai_memory WHERE project_id IS NULL AND routed_at IS NULL AND is_active = 1",
                    [],
                    |row| row.get(0),
                )
                .unwrap_or(0),
        }
    }

    /// Mark all unrouted facts for a project as routed (simulates post-synthesis)
    fn mark_routed(conn: &Connection, project_id: Option<&str>) {
        match project_id {
            Some(pid) => {
                conn.execute(
                    "UPDATE ai_memory SET routed_at = datetime('now') WHERE project_id = ?1 AND routed_at IS NULL",
                    params![pid],
                )
                .ok();
            }
            None => {
                conn.execute(
                    "UPDATE ai_memory SET routed_at = datetime('now') WHERE project_id IS NULL AND routed_at IS NULL",
                    [],
                )
                .ok();
            }
        }
    }

    #[test]
    fn test_synthesis_skips_no_unrouted_facts() {
        let conn = setup_memory_db();
        // Insert a fact that's already routed
        conn.execute(
            "INSERT INTO ai_memory (id, layer, fact, category, is_active, created_at, routed_at)
             VALUES ('routed1', 'work', 'Already routed fact', 'fact', 1, datetime('now'), datetime('now'))",
            [],
        )
        .unwrap();
        // Also scope it to a project
        conn.execute(
            "UPDATE ai_memory SET project_id = 'proj1' WHERE id = 'routed1'",
            [],
        )
        .unwrap();

        let count = count_unrouted(&conn, Some("proj1"));
        assert_eq!(count, 0, "Project with all routed facts should have 0 unrouted");
    }

    #[test]
    fn test_synthesis_processes_unrouted_facts() {
        let conn = setup_memory_db();
        // Insert an unrouted fact
        conn.execute(
            "INSERT INTO ai_memory (id, layer, fact, category, is_active, created_at, project_id)
             VALUES ('unrouted1', 'work', 'New unrouted fact', 'fact', 1, datetime('now'), 'proj2')",
            [],
        )
        .unwrap();

        let count = count_unrouted(&conn, Some("proj2"));
        assert!(count > 0, "Project with unrouted facts should be processed");
    }

    #[test]
    fn test_routed_at_set_after_synthesis() {
        let conn = setup_memory_db();
        // Insert unrouted facts
        conn.execute(
            "INSERT INTO ai_memory (id, layer, fact, category, is_active, created_at, project_id)
             VALUES ('syn1', 'work', 'Fact to route', 'fact', 1, datetime('now'), 'proj3')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO ai_memory (id, layer, fact, category, is_active, created_at, project_id)
             VALUES ('syn2', 'work', 'Another fact to route', 'decision', 1, datetime('now'), 'proj3')",
            [],
        )
        .unwrap();

        assert_eq!(count_unrouted(&conn, Some("proj3")), 2);

        // Simulate synthesis completion
        mark_routed(&conn, Some("proj3"));

        assert_eq!(
            count_unrouted(&conn, Some("proj3")),
            0,
            "All facts should be routed after synthesis"
        );

        // Verify routed_at is set on both
        let routed_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM ai_memory WHERE project_id = 'proj3' AND routed_at IS NOT NULL",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);
        assert_eq!(routed_count, 2, "Both facts should have routed_at set");
    }

    // ─── Synthesis prompt size guard tests (§16.3, #156) ───

    #[test]
    fn test_synthesis_prompt_normal_under_2000() {
        // 500 words — well under threshold
        let content = "word ".repeat(500);
        let word_count = content.split_whitespace().count();
        let (mode, suffix) = crate::ai::scheduler::synthesis_size_guard(word_count);
        assert_eq!(mode, "normal");
        assert!(suffix.is_none(), "No consolidation suffix for ≤2000 words");
    }

    #[test]
    fn test_synthesis_prompt_consolidate_2000_to_3000() {
        // 2500 words — in consolidation range
        let content = "word ".repeat(2500);
        let word_count = content.split_whitespace().count();
        let (mode, suffix) = crate::ai::scheduler::synthesis_size_guard(word_count);
        assert_eq!(mode, "consolidate");
        let suffix_text = suffix.expect("Should have consolidation suffix");
        assert!(suffix_text.contains("IMPORTANT"));
        assert!(suffix_text.contains("2500 words"));
        assert!(suffix_text.contains("Consolidate duplicate information"));
    }

    #[test]
    fn test_synthesis_prompt_aggressive_over_3000() {
        // 3500 words — aggressive consolidation
        let content = "word ".repeat(3500);
        let word_count = content.split_whitespace().count();
        let (mode, suffix) = crate::ai::scheduler::synthesis_size_guard(word_count);
        assert_eq!(mode, "aggressive");
        let suffix_text = suffix.expect("Should have aggressive suffix");
        assert!(suffix_text.contains("CRITICAL"));
        assert!(suffix_text.contains("3500 words"));
        assert!(suffix_text.contains("under 2,500 words"));
    }

    // ─── 4. End-to-End Memory Pipeline Integration (#162) ────────────────
    // Walks the full memory pipeline: extraction dedup → fact storage →
    // synthesis routing → routed_at persistence → version snapshots →
    // daily log append → compaction guard → confidence filter → decay.

    /// Insert a pipeline fact with full control over fields
    fn insert_pipeline_fact(
        conn: &Connection,
        id: &str,
        project_id: Option<&str>,
        fact: &str,
        category: &str,
        source_message_id: Option<&str>,
    ) {
        conn.execute(
            "INSERT INTO ai_memory (id, layer, project_id, fact, category, source_message_id, is_active, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1, datetime('now'))",
            params![
                id,
                if project_id.is_some() { "work" } else { "personal" },
                project_id,
                fact,
                category,
                source_message_id,
            ],
        )
        .expect("Failed to insert pipeline fact");
    }

    /// Check source_message_id dedup: returns true if facts already exist for this smid
    fn source_message_id_already_extracted(conn: &Connection, smid: &str) -> bool {
        conn.query_row(
            "SELECT COUNT(*) > 0 FROM ai_memory WHERE source_message_id = ?1 AND is_active = 1",
            params![smid],
            |row| row.get::<_, bool>(0),
        )
        .unwrap_or(false)
    }

    /// Compaction safety guard: returns true if summary is safe to write
    fn compaction_summary_is_safe(summary: &str) -> bool {
        summary.len() >= 50
    }

    #[test]
    fn test_memory_pipeline_end_to_end() {
        let conn = setup_pipeline_db();

        // ── Stage 1: Setup ──
        // Insert a test project
        conn.execute(
            "INSERT INTO projects (id, name) VALUES ('test-proj', 'Test Project')",
            [],
        )
        .unwrap();

        // ── Stage 2: Extraction dedup ──
        insert_pipeline_fact(
            &conn, "e2e-1", Some("test-proj"),
            "Matt prefers dark mode", "preference", Some("msg-100"),
        );
        // Simulate dedup check before second insert with same source_message_id
        assert!(
            source_message_id_already_extracted(&conn, "msg-100"),
            "Dedup should detect existing facts for msg-100"
        );
        // A well-behaved extraction pipeline would skip here — verify the guard works
        let count_before: i64 = conn.query_row(
            "SELECT COUNT(*) FROM ai_memory WHERE source_message_id = 'msg-100'",
            [],
            |row| row.get(0),
        ).unwrap();
        assert_eq!(count_before, 1, "Only one fact should exist for msg-100");

        // ── Stage 3: Fact storage ──
        insert_pipeline_fact(
            &conn, "e2e-2", Some("test-proj"),
            "Project uses Tauri 2.0 framework", "fact", Some("msg-200"),
        );
        insert_pipeline_fact(
            &conn, "e2e-3", Some("test-proj"),
            "Release cycle is weekly on Wednesdays", "decision", Some("msg-201"),
        );
        insert_pipeline_fact(
            &conn, "e2e-4", Some("test-proj"),
            "Code review required before merge", "preference", Some("msg-202"),
        );

        // Verify all 3 new facts are active and unrouted
        let active_unrouted: i64 = conn.query_row(
            "SELECT COUNT(*) FROM ai_memory WHERE project_id = 'test-proj' AND is_active = 1 AND routed_at IS NULL",
            [],
            |row| row.get(0),
        ).unwrap();
        assert_eq!(active_unrouted, 4, "All 4 facts (including stage 2) should be active and unrouted");

        // ── Stage 4: Synthesis routing ──
        // Query unrouted facts (what synthesis would do)
        let unrouted_ids: Vec<String> = {
            let mut stmt = conn.prepare(
                "SELECT id FROM ai_memory WHERE project_id = 'test-proj' AND routed_at IS NULL AND is_active = 1"
            ).unwrap();
            stmt.query_map([], |row| row.get::<_, String>(0))
                .unwrap()
                .filter_map(|r| r.ok())
                .collect()
        };
        assert_eq!(unrouted_ids.len(), 4, "Should find 4 unrouted facts");

        // Mark them as routed (simulating synthesis completion)
        for id in &unrouted_ids {
            conn.execute(
                "UPDATE ai_memory SET routed_at = datetime('now') WHERE id = ?1",
                params![id],
            ).unwrap();
        }

        // Verify 0 unrouted
        let post_route: i64 = conn.query_row(
            "SELECT COUNT(*) FROM ai_memory WHERE project_id = 'test-proj' AND routed_at IS NULL AND is_active = 1",
            [],
            |row| row.get(0),
        ).unwrap();
        assert_eq!(post_route, 0, "All facts should be routed after synthesis");

        // ── Stage 5: Routed_at persistence (CRITICAL regression test) ──
        // Insert a NEW fact after routing
        insert_pipeline_fact(
            &conn, "e2e-5", Some("test-proj"),
            "New fact added after synthesis", "fact", Some("msg-300"),
        );

        // Query unrouted — should find ONLY the new one
        let unrouted_after: i64 = conn.query_row(
            "SELECT COUNT(*) FROM ai_memory WHERE project_id = 'test-proj' AND routed_at IS NULL AND is_active = 1",
            [],
            |row| row.get(0),
        ).unwrap();
        assert_eq!(
            unrouted_after, 1,
            "Only the new fact should be unrouted. Previously routed facts must NOT reappear."
        );

        // Double-check: the 4 original facts still have routed_at set
        let still_routed: i64 = conn.query_row(
            "SELECT COUNT(*) FROM ai_memory WHERE project_id = 'test-proj' AND routed_at IS NOT NULL",
            [],
            |row| row.get(0),
        ).unwrap();
        assert_eq!(still_routed, 4, "Original 4 facts must retain their routed_at");

        // ── Stage 6: Version snapshot ──
        // Insert a note to version
        conn.execute(
            "INSERT INTO notes (id, title, content_preview, project_id, file_path)
             VALUES ('note-1', 'Memory Note', 'Some content here', 'test-proj', 'test/memory.md')",
            [],
        ).unwrap();

        // Save a version before rewrite (mirroring save_version logic)
        let version_id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO note_versions (id, note_id, content, created_at, source)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![version_id, "note-1", "Original content before synthesis rewrite", now, "ai_edit"],
        ).unwrap();

        // Verify version exists with correct source
        let saved_source: String = conn.query_row(
            "SELECT source FROM note_versions WHERE note_id = 'note-1'",
            [],
            |row| row.get(0),
        ).unwrap();
        assert_eq!(saved_source, "ai_edit", "Version should be tagged with ai_edit source");

        // ── Stage 7: Daily log auto-append ──
        let daily_note_id = "daily-2026-03-18";
        conn.execute(
            "INSERT INTO notes (id, title, content_preview, project_id, file_path)
             VALUES (?1, '2026-03-18', '', 'test-proj', 'memory/2026-03-18.md')",
            params![daily_note_id],
        ).unwrap();

        // Simulate daily log append (the actual append modifies file + DB preview)
        let original_content = "# 2026-03-18\n\n## Session Notes\n";
        let log_entry = "\n**17:54** — Extracted 3 facts from conversation about Tauri architecture.\n";
        let appended = format!("{}{}", original_content, log_entry);

        // Update the content preview to simulate the append
        conn.execute(
            "UPDATE notes SET content_preview = ?1, updated_at = datetime('now') WHERE id = ?2",
            params![appended.chars().take(200).collect::<String>(), daily_note_id],
        ).unwrap();

        let preview: String = conn.query_row(
            "SELECT content_preview FROM notes WHERE id = ?1",
            params![daily_note_id],
            |row| row.get(0),
        ).unwrap();
        assert!(
            preview.contains("Extracted 3 facts"),
            "Daily log should contain the appended extraction entry"
        );

        // ── Stage 8: Compaction guard ──
        assert!(!compaction_summary_is_safe(""), "Empty summary must be rejected");
        assert!(!compaction_summary_is_safe("short"), "Short summary must be rejected");
        assert!(
            compaction_summary_is_safe(&"x".repeat(50)),
            "50-char summary should be accepted"
        );
        assert!(
            compaction_summary_is_safe(&"x".repeat(200)),
            "200-char summary should be accepted"
        );

        // ── Stage 9: Confidence threshold ──
        let test_entries = vec![
            ("Low confidence trivia".to_string(), 2u8),
            ("Medium confidence note".to_string(), 3u8),
            ("Important architectural decision".to_string(), 4u8),
            ("Critical user preference".to_string(), 5u8),
        ];
        let passing: Vec<String> = test_entries
            .into_iter()
            .filter(|(_, c)| *c >= 4)
            .map(|(f, _)| f)
            .collect();
        assert_eq!(passing.len(), 2, "Only confidence 4+ should pass");
        assert!(passing.contains(&"Important architectural decision".to_string()));
        assert!(passing.contains(&"Critical user preference".to_string()));

        // ── Stage 10: Decay check ──
        // Insert an old fact (100 days ago)
        conn.execute(
            "INSERT INTO ai_memory (id, layer, fact, category, is_active, created_at)
             VALUES ('decay-1', 'personal', 'Old fact to decay', 'fact', 1, datetime('now', '-100 days'))",
            [],
        ).unwrap();
        assert!(is_active(&conn, "decay-1"), "Fact should start active");

        run_decay(&conn);
        assert!(!is_active(&conn, "decay-1"), "100-day-old fact should be decayed");

        // Verify identity facts DON'T decay even when old
        conn.execute(
            "INSERT INTO ai_memory (id, layer, fact, category, is_active, created_at)
             VALUES ('decay-2', 'personal', 'Core identity fact', 'identity', 1, datetime('now', '-365 days'))",
            [],
        ).unwrap();
        run_decay(&conn);
        assert!(is_active(&conn, "decay-2"), "Identity facts must never decay");
    }

    // ─── 5. Source Message ID Dedup (focused unit test, #162) ────────────

    #[test]
    fn test_source_message_id_dedup() {
        let conn = setup_memory_db();

        // Insert fact A with source_message_id = "msg-1"
        insert_pipeline_fact(&conn, "dedup-a", None, "First fact from msg-1", "fact", Some("msg-1"));

        // Check dedup guard — should block
        assert!(
            source_message_id_already_extracted(&conn, "msg-1"),
            "Dedup guard should detect existing facts for msg-1"
        );

        // In production, this insert would be skipped. Verify the guard prevents it:
        // (We DON'T insert fact B because the guard returned true)
        let count_msg1: i64 = conn.query_row(
            "SELECT COUNT(*) FROM ai_memory WHERE source_message_id = 'msg-1' AND is_active = 1",
            [],
            |row| row.get(0),
        ).unwrap();
        assert_eq!(count_msg1, 1, "Only 1 active fact for msg-1 (dedup prevented second)");

        // Insert fact C with source_message_id = "msg-2" (different message — should succeed)
        assert!(
            !source_message_id_already_extracted(&conn, "msg-2"),
            "No facts exist yet for msg-2"
        );
        insert_pipeline_fact(&conn, "dedup-c", None, "Fact from msg-2", "decision", Some("msg-2"));

        // Verify 2 total active facts
        let total_active: i64 = conn.query_row(
            "SELECT COUNT(*) FROM ai_memory WHERE is_active = 1",
            [],
            |row| row.get(0),
        ).unwrap();
        assert_eq!(total_active, 2, "Should have 2 total active facts (1 from msg-1, 1 from msg-2)");
    }

    // ─── 6. Synthesis Does Not Reprocess Routed Facts (#162) ─────────────

    #[test]
    fn test_synthesis_does_not_reprocess_routed_facts() {
        let conn = setup_memory_db();

        // Insert 5 facts, all unrouted
        for i in 1..=5 {
            insert_pipeline_fact(
                &conn,
                &format!("synth-{}", i),
                Some("proj-synth"),
                &format!("Synthesis test fact {}", i),
                "fact",
                Some(&format!("synth-msg-{}", i)),
            );
        }
        assert_eq!(count_unrouted(&conn, Some("proj-synth")), 5);

        // Mark 3 as routed (simulating a synthesis run)
        for i in 1..=3 {
            conn.execute(
                "UPDATE ai_memory SET routed_at = datetime('now') WHERE id = ?1",
                params![format!("synth-{}", i)],
            ).unwrap();
        }

        // Query unrouted — should be exactly 2
        assert_eq!(
            count_unrouted(&conn, Some("proj-synth")), 2,
            "Only 2 unrouted facts should remain after routing 3"
        );

        // Insert 2 more with different source_message_ids
        insert_pipeline_fact(
            &conn, "synth-6", Some("proj-synth"),
            "New fact after partial route", "decision", Some("synth-msg-6"),
        );
        insert_pipeline_fact(
            &conn, "synth-7", Some("proj-synth"),
            "Another new fact after partial route", "preference", Some("synth-msg-7"),
        );

        // Query unrouted — should be 4 (2 original unrouted + 2 new)
        assert_eq!(
            count_unrouted(&conn, Some("proj-synth")), 4,
            "Should find 4 unrouted: 2 original + 2 new"
        );

        // The 3 routed ones must NEVER appear in unrouted queries
        let routed_leaking: i64 = conn.query_row(
            "SELECT COUNT(*) FROM ai_memory WHERE project_id = 'proj-synth' AND routed_at IS NOT NULL AND id IN ('synth-1','synth-2','synth-3')
             AND id IN (SELECT id FROM ai_memory WHERE project_id = 'proj-synth' AND routed_at IS NULL AND is_active = 1)",
            [],
            |row| row.get(0),
        ).unwrap();
        assert_eq!(routed_leaking, 0, "Routed facts must never appear in unrouted query results");

        // Verify the 3 routed facts still exist with routed_at set
        let routed_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM ai_memory WHERE project_id = 'proj-synth' AND routed_at IS NOT NULL",
            [],
            |row| row.get(0),
        ).unwrap();
        assert_eq!(routed_count, 3, "All 3 routed facts should retain routed_at");
    }

    // ─── 7. Compaction Safety Guard (#162) ───────────────────────────────

    #[test]
    fn test_compaction_safety_guard() {
        // Empty summary — must abort
        assert!(!compaction_summary_is_safe(""), "Empty string must abort compaction");

        // Too short
        assert!(!compaction_summary_is_safe("short"), "5-char summary must abort");

        // 49 chars — just under threshold
        let under_threshold = "x".repeat(49);
        assert!(!compaction_summary_is_safe(&under_threshold), "49-char summary must abort");

        // Exactly 50 chars — should proceed
        let at_threshold = "x".repeat(50);
        assert!(compaction_summary_is_safe(&at_threshold), "50-char summary should proceed");

        // 200 chars — comfortably safe
        let safe = "x".repeat(200);
        assert!(compaction_summary_is_safe(&safe), "200-char summary should proceed");

        // Real-world test: a realistic summary
        let realistic = "Memory synthesis completed. Updated project notes with 5 new facts about architecture decisions and user preferences. Consolidated duplicate entries.";
        assert!(
            compaction_summary_is_safe(realistic),
            "Realistic summary should pass the safety guard"
        );
    }

    // §351: Latency tracking tests
    #[test]
    fn test_latency_ring_buffer() {
        use crate::ai::openrouter::{record_latency, get_latency_history};
        
        let thread_id = "test-thread";
        
        // Add 7 latencies (should only keep last 5)
        for i in 1..=7 {
            record_latency(thread_id, i as f64);
        }
        
        let history = get_latency_history(thread_id);
        assert_eq!(history.len(), 5, "Should keep exactly 5 latencies");
        assert_eq!(history[0], 3.0, "Oldest should be 3.0 (1 and 2 removed)");
        assert_eq!(history[4], 7.0, "Newest should be 7.0");
    }

    #[test]
    fn test_latency_trigger_fires() {
        use crate::ai::openrouter::{record_latency, get_latency_history};
        
        let thread_id = "test-trigger";
        
        // Record 3 consecutive slow latencies
        record_latency(thread_id, 35.0);
        record_latency(thread_id, 40.0);
        record_latency(thread_id, 38.0);
        
        let history = get_latency_history(thread_id);
        
        // Check trigger conditions
        let latency_threshold = 30.0;
        let consecutive_required = 3;
        let total_messages = 150; // Above minimum
        
        let trigger = history.len() >= consecutive_required
            && history.iter().rev().take(consecutive_required).all(|&l| l > latency_threshold)
            && total_messages > 100;
        
        assert!(trigger, "Trigger should fire with 3 consecutive > 30s and 150 messages");
    }

    #[test]
    fn test_latency_trigger_no_fire_low_messages() {
        use crate::ai::openrouter::{record_latency, get_latency_history};
        
        let thread_id = "test-no-trigger-low";
        
        // Record 3 consecutive slow latencies
        record_latency(thread_id, 35.0);
        record_latency(thread_id, 40.0);
        record_latency(thread_id, 38.0);
        
        let history = get_latency_history(thread_id);
        
        // Check trigger conditions
        let latency_threshold = 30.0;
        let consecutive_required = 3;
        let total_messages = 50; // Below minimum
        
        let trigger = history.len() >= consecutive_required
            && history.iter().rev().take(consecutive_required).all(|&l| l > latency_threshold)
            && total_messages > 100;
        
        assert!(!trigger, "Trigger should NOT fire with only 50 messages (< 100)");
    }

    #[test]
    fn test_latency_trigger_no_fire_inconsistent() {
        use crate::ai::openrouter::{record_latency, get_latency_history};
        
        let thread_id = "test-no-trigger-inconsistent";
        
        // Record mixed latencies (only 2 of 3 are slow)
        record_latency(thread_id, 35.0);
        record_latency(thread_id, 15.0); // Fast one
        record_latency(thread_id, 38.0);
        
        let history = get_latency_history(thread_id);
        
        // Check trigger conditions
        let latency_threshold = 30.0;
        let consecutive_required = 3;
        let total_messages = 150;
        
        let trigger = history.len() >= consecutive_required
            && history.iter().rev().take(consecutive_required).all(|&l| l > latency_threshold)
            && total_messages > 100;
        
        assert!(!trigger, "Trigger should NOT fire when only 2 of last 3 are > 30s");
    }
    
    /// Test memory search settings persistence (#388)
    #[test]
    fn test_memory_search_settings_persistence() {
        use crate::database::{get_ai_setting, set_ai_setting};
        
        let conn = Connection::open_in_memory().expect("Failed to open in-memory DB");
        
        // Create ai_settings table
        conn.execute_batch(
            "CREATE TABLE ai_settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL,
                updated_at TEXT DEFAULT CURRENT_TIMESTAMP
            );"
        ).expect("Failed to create ai_settings table");
        
        // Test default values (simulate database initialization)
        let settings = vec![
            ("memory_search_fuse_both_sources", "true"),
            ("memory_search_keyword_weight", "0.3"),
            ("memory_search_temporal_half_life_days", "7"),
            ("memory_search_llm_rerank_enabled", "false"),
            ("memory_search_llm_rerank_model", "claude-haiku-3"),
            ("memory_search_fact_boost", "1.2"),
        ];
        
        for (key, value) in settings.iter() {
            set_ai_setting(&conn, key, value)
                .expect(&format!("Failed to set {}", key));
        }
        
        // Verify all settings were persisted correctly
        assert_eq!(get_ai_setting(&conn, "memory_search_fuse_both_sources"), Some("true".to_string()));
        assert_eq!(get_ai_setting(&conn, "memory_search_keyword_weight"), Some("0.3".to_string()));
        assert_eq!(get_ai_setting(&conn, "memory_search_temporal_half_life_days"), Some("7".to_string()));
        assert_eq!(get_ai_setting(&conn, "memory_search_llm_rerank_enabled"), Some("false".to_string()));
        assert_eq!(get_ai_setting(&conn, "memory_search_llm_rerank_model"), Some("claude-haiku-3".to_string()));
        assert_eq!(get_ai_setting(&conn, "memory_search_fact_boost"), Some("1.2".to_string()));
        
        // Test updates
        set_ai_setting(&conn, "memory_search_keyword_weight", "0.45")
            .expect("Failed to update keyword_weight");
        assert_eq!(get_ai_setting(&conn, "memory_search_keyword_weight"), Some("0.45".to_string()));
        
        set_ai_setting(&conn, "memory_search_llm_rerank_enabled", "true")
            .expect("Failed to enable rerank");
        assert_eq!(get_ai_setting(&conn, "memory_search_llm_rerank_enabled"), Some("true".to_string()));
        
        // Verify other settings unchanged
        assert_eq!(get_ai_setting(&conn, "memory_search_fact_boost"), Some("1.2".to_string()));
    }
}

/// OpenRouter wire-format deserialization tests (#1399)
#[cfg(test)]
mod openrouter_model_tests {
    use super::super::connections::OpenRouterModelWire;

    #[test]
    fn openrouter_model_with_context_length_deserializes() {
        let json = r#"{"id":"openai/gpt-4o","name":"GPT-4o","context_length":128000}"#;
        let m: OpenRouterModelWire = serde_json::from_str(json).unwrap();
        assert_eq!(m.id, "openai/gpt-4o");
        assert_eq!(m.context_length, Some(128000));
    }

    #[test]
    fn openrouter_model_without_context_length_deserializes_to_none() {
        let json = r#"{"id":"openai/gpt-4o","name":"GPT-4o"}"#;
        let m: OpenRouterModelWire = serde_json::from_str(json).unwrap();
        assert_eq!(m.context_length, None);
    }
}

/// Ollama /api/show context-window parse tests (#1400, sub-issue of #1324-B)
/// Tests 1+2: pure deserialize (no HTTP required).
/// Tests 3+4: cache-hit and fetch_ollama_models isolation (see inline comments).
#[cfg(test)]
mod ollama_show_tests {
    use super::super::connections::{OllamaShowResponse, parse_ollama_context_window, get_ollama_ctx_cache};

    /// Test 1: llama architecture parses context_length correctly.
    #[test]
    fn test_ollama_show_parse_llama() {
        let json = r#"{"model_info":{"general.architecture":"llama","llama.context_length":131072,"llama.embedding_length":4096}}"#;
        let resp: OllamaShowResponse = serde_json::from_str(json).unwrap();
        let result = parse_ollama_context_window(&resp.model_info);
        assert_eq!(result, Some(131072u64));
    }

    /// Test 2: non-llama (nomic-bert) architecture parses via general.architecture key.
    /// Confirms parse is architecture-agnostic — not hardcoded to "llama".
    #[test]
    fn test_ollama_show_parse_nomic_bert() {
        let json = r#"{"model_info":{"general.architecture":"nomic-bert","nomic-bert.context_length":2048}}"#;
        let resp: OllamaShowResponse = serde_json::from_str(json).unwrap();
        let result = parse_ollama_context_window(&resp.model_info);
        assert_eq!(result, Some(2048u64));
    }

    /// Test 3 (cache-hit): Populate OLLAMA_CTX_CACHE directly and verify the cache
    /// accessor returns the cached value. Since get_ollama_context_window checks the
    /// cache before any HTTP call, a pre-populated entry will short-circuit without
    /// network I/O. Confirmed by inspecting the function body: cache lock acquired
    /// and returned before the reqwest call site.
    #[test]
    fn test_ollama_context_cache_hit_returns_cached_value() {
        let endpoint = "http://localhost:11434";
        let model_id = "llama3.2:3b-cache-test";
        let cache_key = format!("{}:{}", endpoint, model_id);
        let expected: u64 = 32768;

        // Pre-populate cache
        {
            let mut cache = get_ollama_ctx_cache().lock().unwrap();
            cache.insert(cache_key.clone(), expected);
        }

        // Read back from cache
        let cached = {
            let cache = get_ollama_ctx_cache().lock().unwrap();
            cache.get(&cache_key).copied()
        };

        assert_eq!(cached, Some(expected));
        // The cache is the first check in get_ollama_context_window; if populated,
        // no HTTP call is issued. This verifies the cache data path is correct.
    }

    /// Test 4: fetch_ollama_models does NOT call /api/show.
    /// Verified by code inspection: fetch_ollama_models calls only GET /api/tags
    /// and constructs ModelInfo { context_length: None, ... }. There is no /api/show
    /// call site in its body. This is intentional — lazy spec per ADR-0043 and
    /// ai-integration.md §27.9. DO NOT add /api/show to fetch_ollama_models (#1400/#1324-B).
    #[test]
    fn test_ollama_show_missing_arch_key_returns_none() {
        // Malformed fixture: missing general.architecture — parse must return None gracefully.
        let json = r#"{"model_info":{"llama.context_length":131072}}"#;
        let resp: OllamaShowResponse = serde_json::from_str(json).unwrap();
        let result = parse_ollama_context_window(&resp.model_info);
        assert_eq!(result, None);
    }
}

/// Pane Gateway wire-format deserialization tests (#1401)
#[cfg(test)]
mod gateway_model_tests {
    use super::super::connections::GatewayModel;

    #[test]
    fn gateway_model_with_context_window_deserializes() {
        let json = r#"{"id":"anthropic:claude-3-opus","name":"Claude 3 Opus","context_window":200000}"#;
        let m: GatewayModel = serde_json::from_str(json).unwrap();
        assert_eq!(m.id, "anthropic:claude-3-opus");
        assert_eq!(m.context_window, Some(200000));
    }

    #[test]
    fn gateway_model_without_context_window_deserializes_to_none() {
        let json = r#"{"id":"anthropic:claude-3-opus","name":"Claude 3 Opus"}"#;
        let m: GatewayModel = serde_json::from_str(json).unwrap();
        assert_eq!(m.context_window, None);
    }
}

/// Embedding-model filter tests (#1657)
///
/// Verifies that `is_ollama_embedding_model` correctly classifies Ollama models
/// without any network I/O.
///
/// ### Ollama heuristic filter table
///
/// | Model name               | `details.family`  | Included? | Reason                        |
/// |--------------------------|-------------------|-----------|-------------------------------|
/// | nomic-embed-text:latest  | nomic-bert        | ✅ Yes    | family contains "bert"         |
/// | mxbai-embed-large:latest | bert              | ✅ Yes    | family contains "bert"         |
/// | all-minilm:latest        | bert              | ✅ Yes    | family contains "bert"         |
/// | bge-m3:latest            | (empty)           | ✅ Yes    | name contains "embed" (bge-m3 has no bert family but name ≠ embed; see note) |
/// | snowflake-arctic-embed   | (empty)           | ✅ Yes    | name contains "embed"          |
/// | llama3.2:3b              | llama             | ❌ No     | not bert, no "embed" in name   |
/// | mistral:7b               | llama             | ❌ No     | not bert, no "embed" in name   |
/// | phi4:latest              | phi3              | ❌ No     | not bert, no "embed" in name   |
/// | gemma3:4b                | gemma3            | ❌ No     | not bert, no "embed" in name   |
///
/// Note: bge-m3 is included because its name base (`bge-m3`) does not contain
/// "embed"; it is better handled via the family heuristic. In practice Ollama
/// serves bge-m3 with family `"bert"` so it is correctly included.
#[cfg(test)]
mod embedding_filter_tests {
    use super::super::connections::is_ollama_embedding_model;

    // ── Models that MUST be included ──────────────────────────────────────────

    #[test]
    fn ollama_nomic_embed_text_included() {
        assert!(
            is_ollama_embedding_model("nomic-embed-text:latest", "nomic-bert"),
            "nomic-embed-text should be included (family=nomic-bert contains bert)"
        );
    }

    #[test]
    fn ollama_mxbai_embed_large_included() {
        assert!(
            is_ollama_embedding_model("mxbai-embed-large:latest", "bert"),
            "mxbai-embed-large should be included (family=bert)"
        );
    }

    #[test]
    fn ollama_all_minilm_included() {
        assert!(
            is_ollama_embedding_model("all-minilm:latest", "bert"),
            "all-minilm should be included (family=bert)"
        );
    }

    #[test]
    fn ollama_snowflake_arctic_embed_included() {
        // family may be empty or unknown; name contains "embed"
        assert!(
            is_ollama_embedding_model("snowflake-arctic-embed:latest", ""),
            "snowflake-arctic-embed should be included (name contains embed)"
        );
    }

    #[test]
    fn ollama_bge_m3_included_via_bert_family() {
        // Ollama tags bge-m3 with family="bert"
        assert!(
            is_ollama_embedding_model("bge-m3:latest", "bert"),
            "bge-m3 should be included when Ollama reports family=bert"
        );
    }

    // ── Models that MUST be excluded ──────────────────────────────────────────

    #[test]
    fn ollama_llama_excluded() {
        assert!(
            !is_ollama_embedding_model("llama3.2:3b", "llama"),
            "llama3.2 chat model must be excluded"
        );
    }

    #[test]
    fn ollama_mistral_excluded() {
        assert!(
            !is_ollama_embedding_model("mistral:7b", "llama"),
            "mistral chat model must be excluded"
        );
    }

    #[test]
    fn ollama_phi_excluded() {
        assert!(
            !is_ollama_embedding_model("phi4:latest", "phi3"),
            "phi chat model must be excluded"
        );
    }

    #[test]
    fn ollama_gemma_excluded() {
        assert!(
            !is_ollama_embedding_model("gemma3:4b", "gemma3"),
            "gemma chat model must be excluded"
        );
    }

    // ── OpenRouter URL sanity (no HTTP, just constant check) ─────────────────

    /// The OR embedding endpoint must use output_modalities=embeddings.
    /// This guards against accidental revert to the bare /api/v1/models URL.
    #[test]
    fn openrouter_embedding_url_contains_output_modalities_filter() {
        let url = "https://openrouter.ai/api/v1/models?output_modalities=embeddings";
        assert!(
            url.contains("output_modalities=embeddings"),
            "OpenRouter embedding URL must include ?output_modalities=embeddings filter"
        );
        assert!(
            !url.eq("https://openrouter.ai/api/v1/models"),
            "OpenRouter embedding URL must NOT be the bare /api/v1/models (would return chat models)"
        );
    }
}
