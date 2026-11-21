CREATE TABLE sqlite_sequence(name,seq);
CREATE TABLE migration_flags (flag TEXT PRIMARY KEY, completed_at TEXT);
CREATE TABLE crsql_tracked_peers ("site_id" BLOB NOT NULL, "version" INTEGER NOT NULL, "seq" INTEGER DEFAULT 0, "tag" INTEGER, "event" INTEGER, PRIMARY KEY ("site_id", "tag", "event")) STRICT;
CREATE TABLE IF NOT EXISTS "crsql_master" ("key" TEXT PRIMARY KEY, "value" ANY);
CREATE TABLE IF NOT EXISTS "crsql_site_id" (site_id BLOB NOT NULL, ordinal INTEGER PRIMARY KEY);
CREATE UNIQUE INDEX crsql_site_id_site_id ON "crsql_site_id" (site_id);
CREATE TABLE IF NOT EXISTS "note_links__crsql_clock" (
      key INTEGER NOT NULL,
      col_name TEXT NOT NULL,
      col_version INTEGER NOT NULL,
      db_version INTEGER NOT NULL,
      site_id INTEGER NOT NULL DEFAULT 0,
      seq INTEGER NOT NULL,
      PRIMARY KEY (key, col_name)
    ) WITHOUT ROWID, STRICT;
CREATE INDEX "note_links__crsql_clock_dbv_idx" ON "note_links__crsql_clock" ("db_version");
CREATE TABLE IF NOT EXISTS "note_links__crsql_pks" (__crsql_key INTEGER PRIMARY KEY, "source_note_id","target_note_id");
CREATE UNIQUE INDEX "note_links__crsql_pks_pks" ON "note_links__crsql_pks" ("source_note_id","target_note_id");
CREATE TABLE simple_crr (
                    id TEXT PRIMARY KEY NOT NULL,
                    data TEXT
                );
CREATE TABLE IF NOT EXISTS "simple_crr__crsql_clock" (
      key INTEGER NOT NULL,
      col_name TEXT NOT NULL,
      col_version INTEGER NOT NULL,
      db_version INTEGER NOT NULL,
      site_id INTEGER NOT NULL DEFAULT 0,
      seq INTEGER NOT NULL,
      PRIMARY KEY (key, col_name)
    ) WITHOUT ROWID, STRICT;
CREATE INDEX "simple_crr__crsql_clock_dbv_idx" ON "simple_crr__crsql_clock" ("db_version");
CREATE TABLE IF NOT EXISTS "simple_crr__crsql_pks" (__crsql_key INTEGER PRIMARY KEY, "id");
CREATE UNIQUE INDEX "simple_crr__crsql_pks_pks" ON "simple_crr__crsql_pks" ("id");
CREATE TRIGGER "simple_crr__crsql_itrig"
      AFTER INSERT ON "simple_crr" WHEN crsql_internal_sync_bit() = 0
      BEGIN
        VALUES (crsql_after_insert('simple_crr', NEW."id"));
      END;
CREATE TRIGGER "simple_crr__crsql_utrig"
      AFTER UPDATE ON "simple_crr" WHEN crsql_internal_sync_bit() = 0
      BEGIN
        VALUES (crsql_after_update('simple_crr', NEW."id", OLD."id", NEW."data", OLD."data"));
      END;
CREATE TRIGGER "simple_crr__crsql_dtrig"
    AFTER DELETE ON "simple_crr" WHEN crsql_internal_sync_bit() = 0
    BEGIN
      VALUES (crsql_after_delete('simple_crr', OLD."id"));
    END;
CREATE TABLE IF NOT EXISTS "notes__crsql_clock" (
      key INTEGER NOT NULL,
      col_name TEXT NOT NULL,
      col_version INTEGER NOT NULL,
      db_version INTEGER NOT NULL,
      site_id INTEGER NOT NULL DEFAULT 0,
      seq INTEGER NOT NULL,
      PRIMARY KEY (key, col_name)
    ) WITHOUT ROWID, STRICT;
CREATE INDEX "notes__crsql_clock_dbv_idx" ON "notes__crsql_clock" ("db_version");
CREATE TABLE IF NOT EXISTS "notes__crsql_pks" (__crsql_key INTEGER PRIMARY KEY, "id");
CREATE UNIQUE INDEX "notes__crsql_pks_pks" ON "notes__crsql_pks" ("id");
CREATE TABLE IF NOT EXISTS "tasks__crsql_clock" (
      key INTEGER NOT NULL,
      col_name TEXT NOT NULL,
      col_version INTEGER NOT NULL,
      db_version INTEGER NOT NULL,
      site_id INTEGER NOT NULL DEFAULT 0,
      seq INTEGER NOT NULL,
      PRIMARY KEY (key, col_name)
    ) WITHOUT ROWID, STRICT;
CREATE INDEX "tasks__crsql_clock_dbv_idx" ON "tasks__crsql_clock" ("db_version");
CREATE TABLE IF NOT EXISTS "tasks__crsql_pks" (__crsql_key INTEGER PRIMARY KEY, "id");
CREATE UNIQUE INDEX "tasks__crsql_pks_pks" ON "tasks__crsql_pks" ("id");
CREATE TABLE IF NOT EXISTS "projects__crsql_clock" (
      key INTEGER NOT NULL,
      col_name TEXT NOT NULL,
      col_version INTEGER NOT NULL,
      db_version INTEGER NOT NULL,
      site_id INTEGER NOT NULL DEFAULT 0,
      seq INTEGER NOT NULL,
      PRIMARY KEY (key, col_name)
    ) WITHOUT ROWID, STRICT;
CREATE INDEX "projects__crsql_clock_dbv_idx" ON "projects__crsql_clock" ("db_version");
CREATE TABLE IF NOT EXISTS "projects__crsql_pks" (__crsql_key INTEGER PRIMARY KEY, "id");
CREATE UNIQUE INDEX "projects__crsql_pks_pks" ON "projects__crsql_pks" ("id");
CREATE TABLE IF NOT EXISTS "users__crsql_clock" (
      key INTEGER NOT NULL,
      col_name TEXT NOT NULL,
      col_version INTEGER NOT NULL,
      db_version INTEGER NOT NULL,
      site_id INTEGER NOT NULL DEFAULT 0,
      seq INTEGER NOT NULL,
      PRIMARY KEY (key, col_name)
    ) WITHOUT ROWID, STRICT;
CREATE INDEX "users__crsql_clock_dbv_idx" ON "users__crsql_clock" ("db_version");
CREATE TABLE IF NOT EXISTS "users__crsql_pks" (__crsql_key INTEGER PRIMARY KEY, "id");
CREATE UNIQUE INDEX "users__crsql_pks_pks" ON "users__crsql_pks" ("id");
CREATE TABLE ai_settings (
            key TEXT PRIMARY KEY NOT NULL,
            value TEXT NOT NULL DEFAULT ''
        );
CREATE TABLE project_categories (
            project_id TEXT PRIMARY KEY NOT NULL,
            category TEXT NOT NULL DEFAULT ''
        , color TEXT DEFAULT '#818CF8');
CREATE TABLE category_definitions (
            name TEXT PRIMARY KEY NOT NULL,
            color TEXT NOT NULL DEFAULT '#818CF8',
            sort_order INTEGER NOT NULL DEFAULT 0
        );
CREATE TABLE note_task_fields (
            note_id TEXT PRIMARY KEY NOT NULL,
            task_status TEXT DEFAULT 'idea',
            task_priority TEXT DEFAULT NULL,
            task_due_date TEXT DEFAULT NULL,
            task_assignee TEXT DEFAULT NULL,
            task_completed_at TEXT DEFAULT NULL,
            task_sort_order INTEGER DEFAULT 0,
            updated_at TEXT DEFAULT ''
        , parent_id TEXT, title TEXT, content_preview TEXT);
CREATE TABLE home_modules (
            id TEXT PRIMARY KEY,
            module_type TEXT NOT NULL,
            sort_order INTEGER NOT NULL DEFAULT 0,
            config_json TEXT NOT NULL DEFAULT '{}',
            data_json TEXT NOT NULL DEFAULT '{}',
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );
CREATE TABLE onboarding_state (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            completed INTEGER NOT NULL DEFAULT 0,
            skipped INTEGER NOT NULL DEFAULT 0,
            current_step INTEGER NOT NULL DEFAULT 0,
            updated_at TEXT NOT NULL
        );
CREATE TABLE user_dashboard_config (
            id TEXT PRIMARY KEY DEFAULT (hex(randomblob(16))),
            onboarding_completed INTEGER DEFAULT 0,
            onboarding_template TEXT,
            greeting_name TEXT,
            theme_variant TEXT DEFAULT 'default',
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
CREATE TABLE dashboard_modules (
            id TEXT PRIMARY KEY DEFAULT (hex(randomblob(16))),
            module_type TEXT NOT NULL,
            sort_order INTEGER NOT NULL DEFAULT 0,
            is_enabled INTEGER NOT NULL DEFAULT 1,
            config_json TEXT DEFAULT '{}',
            static_content TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
CREATE INDEX idx_modules_sort ON dashboard_modules(sort_order) WHERE is_enabled = 1;
CREATE TABLE module_items (
            id TEXT PRIMARY KEY DEFAULT (hex(randomblob(16))),
            module_id TEXT NOT NULL REFERENCES dashboard_modules(id) ON DELETE CASCADE,
            item_type TEXT NOT NULL,
            label TEXT NOT NULL,
            description TEXT,
            emoji TEXT,
            config_json TEXT DEFAULT '{}',
            sort_order INTEGER NOT NULL DEFAULT 0,
            is_active INTEGER NOT NULL DEFAULT 1,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
CREATE INDEX idx_items_module ON module_items(module_id, sort_order);
CREATE TABLE daily_entries (
            id TEXT PRIMARY KEY DEFAULT (hex(randomblob(16))),
            module_id TEXT NOT NULL REFERENCES dashboard_modules(id),
            entry_date TEXT NOT NULL,
            data_json TEXT NOT NULL,
            completed_at TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE(module_id, entry_date)
        );
CREATE INDEX idx_daily_date ON daily_entries(entry_date);
CREATE INDEX idx_daily_module_date ON daily_entries(module_id, entry_date);
CREATE TABLE habit_entries (
            id TEXT PRIMARY KEY DEFAULT (hex(randomblob(16))),
            item_id TEXT NOT NULL REFERENCES module_items(id),
            entry_date TEXT NOT NULL,
            completed INTEGER NOT NULL DEFAULT 0,
            completed_at TEXT,
            UNIQUE(item_id, entry_date)
        );
CREATE INDEX idx_habit_date ON habit_entries(item_id, entry_date);
CREATE TABLE reflection_entries (
            id TEXT PRIMARY KEY DEFAULT (hex(randomblob(16))),
            question_id TEXT NOT NULL,
            entry_date TEXT NOT NULL,
            value_numeric REAL,
            value_bool INTEGER,
            value_text TEXT,
            UNIQUE(question_id, entry_date)
        );
CREATE INDEX idx_reflection_date ON reflection_entries(question_id, entry_date);
CREATE TABLE gratitude_entries (
            id TEXT PRIMARY KEY DEFAULT (hex(randomblob(16))),
            entry_date TEXT NOT NULL,
            entry_index INTEGER NOT NULL DEFAULT 0,
            text TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE(entry_date, entry_index)
        );
CREATE INDEX idx_gratitude_date ON gratitude_entries(entry_date);
CREATE TABLE user_profiles (
            id                      TEXT PRIMARY KEY DEFAULT (hex(randomblob(16))),
            user_id                 TEXT NOT NULL REFERENCES users(id),
            display_name            TEXT,
            bio                     TEXT,
            working_style           TEXT,
            ai_provider             TEXT,
            ai_model                TEXT,
            onboarding_method       TEXT,
            onboarding_completed    INTEGER NOT NULL DEFAULT 0,
            profile_json            TEXT DEFAULT '{}',
            created_at              TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at              TEXT NOT NULL DEFAULT (datetime('now'))
        );
CREATE TABLE user_roles (
            id          TEXT PRIMARY KEY DEFAULT (hex(randomblob(16))),
            user_id     TEXT NOT NULL,
            project_id  TEXT,
            role        TEXT NOT NULL DEFAULT 'viewer',
            granted_by  TEXT,
            created_at  TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE(user_id, project_id)
        );
CREATE INDEX idx_user_roles_user ON user_roles(user_id);
CREATE INDEX idx_user_roles_project ON user_roles(project_id);
CREATE TABLE role_model_permissions (
            role    TEXT NOT NULL,
            model   TEXT NOT NULL,
            allowed INTEGER NOT NULL DEFAULT 1,
            PRIMARY KEY (role, model)
        );
CREATE TABLE ai_chat_messages (
            id                TEXT PRIMARY KEY,
            project_id        TEXT NOT NULL,
            role              TEXT NOT NULL,
            content           TEXT NOT NULL,
            context_note_id   TEXT,
            context_folder_id TEXT,
            context_mode      TEXT DEFAULT 'project',
            token_count       INTEGER,
            model             TEXT,
            created_at        TEXT NOT NULL DEFAULT (datetime('now')),
            FOREIGN KEY (project_id) REFERENCES projects(id)
        );
CREATE INDEX idx_chat_project ON ai_chat_messages(project_id, created_at);
CREATE TABLE ai_context_markers (
            id                TEXT PRIMARY KEY,
            project_id        TEXT NOT NULL,
            marker_type       TEXT NOT NULL,
            label             TEXT NOT NULL,
            target_note_id    TEXT,
            target_folder_id  TEXT,
            created_at        TEXT NOT NULL,
            inserted_after    TEXT,
            FOREIGN KEY (project_id) REFERENCES projects(id)
        );
CREATE TABLE ai_pinned_messages (
            id          TEXT PRIMARY KEY,
            project_id  TEXT NOT NULL,
            message_id  TEXT NOT NULL,
            pinned_at   TEXT NOT NULL,
            sort_order  INTEGER DEFAULT 0,
            FOREIGN KEY (message_id) REFERENCES ai_chat_messages(id),
            UNIQUE(project_id, message_id)
        );
CREATE TABLE ai_conversation_summaries (
            id                TEXT PRIMARY KEY,
            project_id        TEXT NOT NULL,
            summary_text      TEXT NOT NULL,
            start_message_id  TEXT NOT NULL,
            end_message_id    TEXT NOT NULL,
            token_count       INTEGER,
            created_at        TEXT NOT NULL DEFAULT (datetime('now')),
            FOREIGN KEY (project_id) REFERENCES projects(id)
        );
CREATE TABLE ai_memory (
            id                TEXT PRIMARY KEY DEFAULT (hex(randomblob(16))),
            layer             TEXT NOT NULL,
            project_id        TEXT,
            user_id           TEXT,
            fact              TEXT NOT NULL,
            category          TEXT,
            source_message_id TEXT,
            is_active         INTEGER NOT NULL DEFAULT 1,
            created_at        TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at        TEXT
        );
CREATE INDEX idx_memory_layer ON ai_memory(layer);
CREATE INDEX idx_memory_project ON ai_memory(project_id) WHERE project_id IS NOT NULL;
CREATE INDEX idx_memory_user ON ai_memory(user_id) WHERE user_id IS NOT NULL;
CREATE TABLE ai_actions (
            id              TEXT PRIMARY KEY DEFAULT (hex(randomblob(16))),
            user_id         TEXT NOT NULL,
            action_type     TEXT NOT NULL,
            target_id       TEXT,
            target_type     TEXT,
            details_json    TEXT NOT NULL DEFAULT '{}',
            project_id      TEXT,
            model           TEXT,
            message_id      TEXT,
            status          TEXT NOT NULL DEFAULT 'success',
            error_message   TEXT,
            created_at      TEXT NOT NULL DEFAULT (datetime('now'))
        );
CREATE INDEX idx_ai_actions_user ON ai_actions(user_id, created_at);
CREATE INDEX idx_ai_actions_project ON ai_actions(project_id, created_at);
CREATE INDEX idx_ai_actions_type ON ai_actions(action_type);
CREATE INDEX idx_ai_actions_target ON ai_actions(target_id);
CREATE INDEX idx_ai_actions_date ON ai_actions(created_at);
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
            FOREIGN KEY (parent_id) REFERENCES projects(id) ON DELETE SET NULL,
            UNIQUE(name, parent_id)
        );
CREATE INDEX idx_projects_name ON projects(name);
CREATE INDEX idx_projects_parent ON projects(parent_id);
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
            deleted_at      TEXT,
            
            FOREIGN KEY (project_id) REFERENCES projects(id) ON DELETE CASCADE,
            FOREIGN KEY (folder_id) REFERENCES projects(id) ON DELETE SET NULL
        );
CREATE INDEX idx_notes_project ON notes(project_id);
CREATE INDEX idx_notes_folder ON notes(folder_id);
CREATE INDEX idx_notes_status ON notes(status) WHERE status IS NOT NULL;
CREATE INDEX idx_notes_priority ON notes(priority) WHERE priority IS NOT NULL;
CREATE INDEX idx_notes_due_date ON notes(due_date) WHERE due_date IS NOT NULL;
CREATE INDEX idx_notes_updated ON notes(updated_at);
CREATE INDEX idx_notes_deleted ON notes(deleted_at) WHERE deleted_at IS NOT NULL;
CREATE TABLE tags (
            id    INTEGER PRIMARY KEY AUTOINCREMENT,
            name  TEXT NOT NULL UNIQUE,
            color TEXT NOT NULL DEFAULT '#6B7280'
        );
CREATE TABLE note_tags (
            note_id TEXT NOT NULL,
            tag_id  INTEGER NOT NULL,
            PRIMARY KEY (note_id, tag_id),
            FOREIGN KEY (note_id) REFERENCES notes(id) ON DELETE CASCADE,
            FOREIGN KEY (tag_id) REFERENCES tags(id) ON DELETE CASCADE
        );
CREATE INDEX idx_note_tags_note ON note_tags(note_id);
CREATE INDEX idx_note_tags_tag ON note_tags(tag_id);
CREATE TABLE note_links (
            source_note_id TEXT NOT NULL,
            target_note_id TEXT NOT NULL,
            PRIMARY KEY (source_note_id, target_note_id)
        );
CREATE INDEX idx_note_links_source ON note_links(source_note_id);
CREATE INDEX idx_note_links_target ON note_links(target_note_id);
CREATE TABLE users (
            id          TEXT PRIMARY KEY,
            name        TEXT NOT NULL,
            email       TEXT,
            avatar_url  TEXT,
            created_at  TEXT NOT NULL DEFAULT (datetime('now'))
        );
CREATE TABLE ai_message_queue (
            id           TEXT PRIMARY KEY DEFAULT (hex(randomblob(16))),
            project_id   TEXT NOT NULL,
            content      TEXT NOT NULL,
            context_json TEXT NOT NULL,
            queued_at    TEXT NOT NULL DEFAULT (datetime('now')),
            sent_at      TEXT,
            status       TEXT NOT NULL DEFAULT 'queued'
        );
CREATE TABLE sync_state (
            id          TEXT PRIMARY KEY DEFAULT (hex(randomblob(16))),
            device_id   TEXT NOT NULL,
            device_name TEXT,
            platform    TEXT,
            last_sync_at TEXT,
            sync_version INTEGER NOT NULL DEFAULT 0,
            is_current  INTEGER NOT NULL DEFAULT 0,
            created_at  TEXT NOT NULL DEFAULT (datetime('now'))
        );
CREATE TABLE sync_changelog (
            id              TEXT PRIMARY KEY DEFAULT (hex(randomblob(16))),
            table_name      TEXT NOT NULL,
            row_id          TEXT NOT NULL,
            operation       TEXT NOT NULL,
            changeset_json  TEXT,
            device_id       TEXT NOT NULL,
            created_at      TEXT NOT NULL DEFAULT (datetime('now'))
        );
CREATE INDEX idx_sync_changelog_time ON sync_changelog(created_at);
CREATE TABLE extensions (
            id          TEXT PRIMARY KEY,
            name        TEXT NOT NULL UNIQUE,
            version     TEXT NOT NULL,
            manifest    TEXT NOT NULL,
            enabled     INTEGER NOT NULL DEFAULT 1,
            settings    TEXT DEFAULT '{}',
            installed_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
CREATE VIRTUAL TABLE notes_fts USING fts5(
            title,
            content,
            content='notes',
            content_rowid='rowid',
            tokenize='porter unicode61'
        )
/* notes_fts(title,content) */;
CREATE TABLE IF NOT EXISTS 'notes_fts_data'(id INTEGER PRIMARY KEY, block BLOB);
CREATE TABLE IF NOT EXISTS 'notes_fts_idx'(segid, term, pgno, PRIMARY KEY(segid, term)) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS 'notes_fts_docsize'(id INTEGER PRIMARY KEY, sz BLOB);
CREATE TABLE IF NOT EXISTS 'notes_fts_config'(k PRIMARY KEY, v) WITHOUT ROWID;
