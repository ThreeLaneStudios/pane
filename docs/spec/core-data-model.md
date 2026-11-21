# Core Data Model — Unified Entity System

**Last Updated:** 2026-08-02  
**Source:** Extracted from pane-architecture-spec.md  
**Sections:** §1

---

## 1. Data Model — Unified Entity System
> **Status:** BUILT (v11.28.24) — SQLite + CRR, unified note/task entity

### 1.1 Core Principle: Notes and Tasks Are the Same Entity
> **Status:** BUILT

A **note** is a markdown document with metadata. Every note carries an explicit `status`; a newly created note defaults to `'cabinet'` — the backlog / uncategorized state. A **task** is a note whose status is a *workflow* value: `status` is set and not `'cabinet'`. There is no separate `tasks` table — the `notes` table carries the task fields directly (`status` is always set; `priority`, `due_date`, and `assignee` remain optional). `'cabinet'` is a real, named status value, not a NULL sentinel, so "plain note vs task" is a single explicit distinction rather than null-vs-non-null.

**Status vocabulary (closed set):** `cabinet | todo | in_progress | review | done`. There is no `'task'` value and no NULL status (the column is `NOT NULL DEFAULT 'cabinet'`).

**Creation contract:** a single create path; new notes default to `status = 'cabinet'`. There is no `createTask` wrapper — "create a task" is "create a note, then set a workflow status" (or pass a workflow `status` to the one create path). Un-tasking a note sets `status = 'cabinet'` (never NULL).

**Canonical query predicates (mandated — SQL NULL gotcha):**
- **Tasks** (kanban / `get_tasks`): `status IS NOT NULL AND status != 'cabinet'`.
- **Cabinet / non-tasks**: `status = 'cabinet'`.
- Never detect tasks with `status NOT IN ('cabinet', …)`: in SQL three-valued logic, `NOT IN` (and bare `!= 'cabinet'`) silently drop any NULL-status row, so a future nullable regression would quietly lose notes. The `IS NOT NULL` conjunct is mandated as defense-in-depth even though the column is `NOT NULL`.

This eliminates the current dual-table problem (`notes` + `tasks`) and means every piece of content in Pane is a single entity type with a consistent ID, location, and behavior.

### 1.2 Entity Hierarchy
> **Status:** BUILT

```
Workspace (~/Pane/)
  └── Project
        └── Folder (project with parent_id — recursive)
              └── Note (markdown file + DB row)
                    ├── Tags (cross-cutting metadata)
                    └── Task fields (optional: status, priority, due_date, assignee)
```

- **Projects** are top-level containers. They map to directories on disk.
- **Folders** are projects with a `parent_id` pointing to another project. They nest recursively.
- **Notes** belong to exactly one folder (or project root). They are markdown files on disk + metadata rows in SQLite.
- **Tags** are global (workspace-wide) and can be applied to any note.

### 1.3 Unified Notes Table
> **Status:** BUILT

**CRR Constraint Handling:** Because `notes`, `projects`, and `tags` are CRR (Conflict-free Replicated Records) tables for cr-sqlite sync, they cannot have `FOREIGN KEY` or `UNIQUE` constraints beyond the primary key. These constraints are enforced in application code instead (see #493).

```sql
CREATE TABLE notes (
    id              TEXT PRIMARY KEY,
    title           TEXT NOT NULL DEFAULT '',
    body            TEXT,                       -- [#486 v0.59.0] full content for user notes, NULL for system notes
    content_preview TEXT NOT NULL DEFAULT '',   -- first ~200 chars for list views
    file_path       TEXT NOT NULL DEFAULT '',   -- relative path from ~/Pane/
    project_id      TEXT NOT NULL,
    folder_id       TEXT,                       -- NULL = project root
    
    -- Task fields — status is always set (default 'cabinet'); priority/due_date/assignee optional
    status          TEXT NOT NULL DEFAULT 'cabinet', -- 'cabinet' | 'todo' | 'in_progress' | 'review' | 'done' (never NULL, never 'task')
    priority        TEXT,                       -- NULL | 'low' | 'medium' | 'high' | 'urgent'
    due_date        TEXT,                       -- ISO 8601 date
    assignee        TEXT,                       -- user_id (for teams)
    completed_at    TEXT,                       -- timestamp when marked done
    sort_order      INTEGER NOT NULL DEFAULT 0,
    
    -- Metadata
    word_count      INTEGER NOT NULL DEFAULT 0,
    pinned          INTEGER NOT NULL DEFAULT 0,
    owner_id        TEXT,                       -- user who created (auth-ready)
    tags_json       TEXT NOT NULL DEFAULT '[]', -- denormalized for fast reads
    
    created_at      TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at      TEXT NOT NULL DEFAULT (datetime('now')),
    deleted_at      TEXT,                       -- soft delete (trash)
    
    parent_id       TEXT,                       -- hierarchical notes (subtasks)
    note_type       TEXT,                       -- system-generated note type
    is_vital        INTEGER NOT NULL DEFAULT 0, -- vital system note
    agent_id        TEXT                        -- owning agent for system notes
    
    -- NOTE: No FOREIGN KEY constraints — CRR table, enforced in app code (#493)
);

CREATE INDEX idx_notes_project ON notes(project_id);
CREATE INDEX idx_notes_folder ON notes(folder_id);
CREATE INDEX idx_notes_status ON notes(status) WHERE status != 'cabinet';
CREATE INDEX idx_notes_priority ON notes(priority) WHERE priority IS NOT NULL;
CREATE INDEX idx_notes_due_date ON notes(due_date) WHERE due_date IS NOT NULL;
CREATE INDEX idx_notes_updated ON notes(updated_at);
CREATE INDEX idx_notes_deleted ON notes(deleted_at) WHERE deleted_at IS NOT NULL;
```

#### 1.3.1 Task-field writes — one function, human and agent paths alike

> **Status:** BUILT (#3053/#3054) — `completed_at` stamping specified here for the first time (the column existed, §1.3's own table above, but its write semantics were never written down anywhere in this spec, unlike the parallel subtask rule at §1.9.3); the human/agent parity statement below documents a real behavior change, not a restatement of something already covered elsewhere.

**`completed_at` (notes, not subtasks — see §1.9.3 for the subtask analog):** stamped with the current timestamp the first time a note's `status` is set to `'done'` (`current.completed_at.is_none()` guards it — idempotent, won't re-stamp or clobber an already-set value on a redundant "set done" call). **Write-once, by design:** nothing in the codebase clears `completed_at` when `status` later moves away from `'done'` (confirmed: the only `completed_at = NULL` write anywhere in the crate is `subtasks.rs`'s, a different table) — a note's `completed_at` records the first time it was marked done, not "currently done since." This is pre-existing behavior, named here because it was never specified, not because this PR changes it.

**Notification dismiss/clear on `status`/`due_date` writes:** also pre-existing, generically attributed to the write path itself (§19.5.1 step 5b of `notifications.md` already documents this as `update_note`'s behavior, not any one caller's) — restated here only to make the cross-reference explicit: setting `status` to `'done'` dismisses open notifications for the note; reopening from `'done'`, or changing `due_date` to a new non-empty value, companion-clears prior notification rows so the due-date watcher can re-emit. Both are scoped to the single note (`target_id`/`target_kind='note'`), never broader.

**Human/agent parity (the actual change in #3053/#3054):** both of the above are enforced inside the single function `update_note_internal` (`notes.rs`) — called by the user-facing `update_note` Tauri command, and, as of #3053/#3054, also called directly by the AI agent's own `execute_set_status` tool executor (`ai::tools`), replacing that executor's previous second, drifted copy (`UPDATE notes SET status = ...` raw SQL) which stamped neither `completed_at` nor the notification side effects. The agent's `execute_move_note` tool executor is the equivalent fix for `move_note_internal` (project/folder moves, plus the system-note-move guard, `reject_system_note_move` — see `file-system.md` §2A.6b for that guard's own multi-entry-point spec, unchanged by this PR). Before this fix, an agent-driven "mark done" or "move" silently diverged from the identical user-driven action; after it, there is no asymmetry between the two call paths for either operation.

### 1.4 Migration from Current Schema
> **Status:** BUILT (v11.28.24) — migration completed, CRR migration audit (#160 closed)

The current codebase has separate `notes` and `tasks` tables, plus a `note_task_fields` bridge table. The migration:

1. Create the new unified `notes` table
2. Migrate existing `notes` rows (task fields = NULL)
3. Migrate existing `tasks` rows as notes with task fields populated
4. For tasks that had a `note_id` link, merge into the note row
5. Drop old `tasks` and `note_task_fields` tables
6. Update all Tauri commands and frontend stores

### 1.5 Note Version History

> **Status:** BUILT for AI edit paths (`note_versions`); manual-edit snapshots specified here, wiring tracked by #309.

Version history captures restorable past states of a note's `body`, stored in the `note_versions` table. AI edit paths already write it; this section specifies **manual user-edit** snapshots. Entries form one unified chronological list, distinguished by `source` tag.

This is **distinct from** memory-pipeline.md §5.8 Memory Protection, which versions Memory/Soul notes into the separate `ai_memory_versions` table. The two systems do not share a table. (See Session Notes below — there is an open spec-vs-reality question about overlap on memory notes; out of scope for #309.)

#### Table

`note_versions` — **PK-only** (no `UNIQUE`, no `FOREIGN KEY`); single **non-unique** index on `note_id`; `source` column schema-default `'manual'`. **Not CRR** — listed in `SYNC_SKIP_TABLES` (`migrations.rs:32`), so **version history is local-only and does not sync.**

```sql
CREATE TABLE IF NOT EXISTS note_versions (
    id         TEXT NOT NULL PRIMARY KEY,
    note_id    TEXT NOT NULL DEFAULT '',
    content    TEXT NOT NULL DEFAULT '',
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    source     TEXT NOT NULL DEFAULT 'manual'
);

CREATE INDEX IF NOT EXISTS idx_note_versions_note_id ON note_versions(note_id);
```

- *Future multi-machine/team history* = deliberately move `note_versions` out of `SYNC_SKIP_TABLES`; today's local pruning does not block that.

#### Scope — user notes only (`note_type IS NULL`)

Manual snapshots fire **only for user notes** (`note_type IS NULL`). System notes (`soul`/`memory`/`rules`/`daily`/`identity`/`user`/`tools`/`agents`/`heartbeat`) and project Memory notes (`note_type='memory'`) are excluded.

Rationale:

- System/Memory/Soul notes are **read-only in the editor (ADR-0011)** — manual edits can't occur through the normal path, so manual versioning is moot.
- Memory/Soul retain their §5.8 `ai_memory_versions` protection — no double-versioning from the manual path.
- Excludes AI-managed files (consolidator/router rewrite Memory + daily logs frequently) from manual-snapshot noise.

**Discriminator is `note_type`, not `folder_id`.** Project Memory lives at project root (`folder_id = NULL`) but carries `note_type='memory'`, so the `note_type IS NULL` gate correctly excludes it.

#### System-Scope Search Exclusion

> Status: BUILT (v0.68.89, PR #2427) — Matt's ruling 2026-08-05

`search_notes` (FTS5 full-text search, `notes.rs`) excludes all system-scope notes from results. System-scope is defined as:

- `note_type IN ('daily', 'memory', 'soul', 'rules')` — agent daily logs, project Memory, agent Soul/Rules
- `project_id = '__workshop__'` — workshop sentinel scope
- `is_system = 1` on either `project_id` or `folder_id` — AI folders and system projects

All four live consumers (`api.searchNotes`, `api.searchNotesByTitle`, `api.searchTasksFts`, `QuickSwitcher`) were verified as not needing system notes (Reed scoping audit 2026-08-05). A user's manual search, wiki-link picker, task search, and quick-switcher should never surface an agent's internal files.

#### Title-Prefix Search

> Status: BUILT (PR #2435) — title-prefix search via `title_prefix` request flag on `search_notes`

#### Storage model

- **Full snapshot per version, not diff.** Matches the AI paths; keeps dedup a byte-compare and restore trivial; the retention cap bounds storage.
- Persisted via `save_version(conn: &Connection, note_id: &str, content: &str, source: &str) -> Result<String, String>` (stores full content).

#### Source tags

`note_versions.source` values:

| Source tag | Origin |
| --- | --- |
| `pending_ai_edit`, `ai_edit`, `ai_append`, `ai_synthesis` | AI edit paths (`ai::tools`, `ai::scheduler`) |
| `pre_restore` | restore pre-state snapshot (`versions.rs`) |
| **`manual_edit`** | **manual editor checkpoint (new — #309)** |

- Manual snapshots **MUST** pass `source = 'manual_edit'` explicitly.
- The bare schema default `'manual'` is intentionally distinct; it should never appear via normal paths. Its presence signals an insert that bypassed the wiring (diagnostic).
- (`note_versions.source` is unrelated to §5.8 `ai_memory_versions.modified_by` — different table.)

#### Trigger — Shape A (backend time-throttle)

The manual hook lives in `update_note`, which is also the editor autosave path (`saveNote` wraps `invoke('update_note')`). An unconditional snapshot there would flood `note_versions` during active typing. The snapshot is therefore **throttled in the backend**, not driven by frontend cadence:

A manual version is captured **only when all hold**, evaluated inside `update_note`:

1. `request.content` is `Some` (a body write — metadata-only updates via `updateEditorMeta` never reach this branch → mechanically enforces **body-only** versioning), AND
2. the note's `note_type IS NULL` (user note), AND
3. the new `body` differs byte-for-byte from the note's most-recent `note_versions` row (**dedup**), AND
4. at least **~60 s** (tunable) have elapsed since the note's most-recent version (any source) — the **throttle** / checkpoint cadence.

**Accepted tradeoff:** there is no explicit blur/close snapshot. The final sub-60 s state before close may not get its own historical row; this is low-harm — the live `body` is always current, and history is for *past* states.

#### Dedup

Before insert, compare the candidate `body` byte-for-byte against the note's most-recent version row (any source). If identical, skip. (Subsumed into trigger condition 3 above; stated separately as the invariant.)

#### Retention — 50 versions per note, shared pool (all sources)

`save_version` (`versions.rs`) already enforces a **50-row all-sources prune per note** (lines 36–43): after every insert, it deletes the oldest rows for that `note_id` beyond the newest 50 by `created_at`, regardless of `source`. This cap applies to AI and manual versions equally.

- **The #309 hook adds no separate prune.** It calls `save_version`, which handles retention. No scoped `manual_edit` DELETE needed.
- **`save_version` is not modified by #309** (red line preserved) — the existing prune behavior is relied upon, not changed.
- **Practical effect:** a note's 50-slot pool is shared between AI versions (`ai_edit`/`ai_append`/`ai_synthesis`/`pending_ai_edit`/`pre_restore`) and manual versions (`manual_edit`). For a note with only manual edits, it keeps 50 manual snapshots. For a note with both AI and manual edits, the most recent 50 of either type survive.
- Local-only (table is sync-skipped) — pruning has no cross-machine effect.

#### Empty / new note

Do **not** snapshot an empty baseline. The first version fires on the first real content checkpoint (trigger conditions met), never on note creation. Enforced naturally by the dedup + changed-content conditions.

#### Restore interaction

`restore_version` snapshots the pre-restore state as `'pre_restore'`, then makes the restored content the live `body`. Restore does not produce a redundant duplicate of the restored content: after restore the body is unchanged relative to the version restored from, so the next `update_note` either has no body delta (dedup/condition 3 fails) or is within the throttle window (condition 4 fails). **Net: one new row per restore (the `pre_restore` snapshot).**

#### Wiring point (for #309 code fix)

Single canonical hook: **`save_version(conn, note_id, content, "manual_edit")` inside `notes.rs::update_note`**, in the `request.content = Some` branch, **immediately after the body write (`UPDATE notes SET body=…`) succeeds**, gated by trigger conditions 2–4 above.

- `saveNote` is a thin wrapper over `invoke('update_note')`; `updateEditorMeta` carries metadata only (no content). So `update_note` is the single guaranteed body-write path — no triple-wire. (`saveNoteJson` and `readNoteJson` stubs were removed in v0.67.x as dead code — JSON persistence was never implemented; the editor saves markdown only via the 500ms debounced `saveContent(md)` path.)
- The hook must read `note_type` to apply the `IS NULL` gate. `update_note` already reads `note_type` today (for the §24 flat-file write-back path), so the #309 code fix reuses that read — no new query needed.

#### Session notes

1. **Two version systems — confirmed distinct (do not conflate).** `note_versions` (this section): general notes, `source` column, PK-only, sync-skipped. `ai_memory_versions` (§5.8): Memory/Soul scope, `modified_by` column. §5.8 left untouched.
2. **AI-path scope asymmetry — approved.** The AI version path is not scoped to `note_type IS NULL`: synthesis writes `ai_synthesis` pre-versions of Memory notes into `note_versions`, and `ai_append` guards against `memory`/`daily` but versions other system notes (soul/rules/etc.). Manual is user-notes-only. The asymmetry is intentional — system/memory/soul files are AI-managed, never manually edited (read-only in editor per ADR-0011).
3. **Spec-vs-reality gap (tracked separately in #1726).** Memory notes appear versioned in `note_versions` (via `ai_synthesis`) regardless of §5.8's `ai_memory_versions`. Out of scope for #309.
4. **AI-path retention capped at 50 combined per note.** `save_version` already enforces a 50-row all-sources prune (versions.rs:36–43). The prior claim that AI-path retention was unbounded was incorrect — #1727 filed on that premise needs correction (close or repurpose).

### 1.6 Tags
> **Status:** BUILT

Tags remain a separate table for normalization and color management:

```sql
CREATE TABLE tags (
    id    TEXT PRIMARY KEY,
    name  TEXT NOT NULL,  -- No UNIQUE: CRR table, enforced in app code (#493)
    color TEXT NOT NULL DEFAULT '#6B7280'
);

CREATE TABLE note_tags (
    note_id TEXT NOT NULL,
    tag_id  INTEGER NOT NULL,
    PRIMARY KEY (note_id, tag_id),
    FOREIGN KEY (note_id) REFERENCES notes(id) ON DELETE CASCADE,
    FOREIGN KEY (tag_id) REFERENCES tags(id) ON DELETE CASCADE
);
```

The `tags_json` column on `notes` is a denormalized cache for fast list rendering. It's updated whenever `note_tags` changes.

### 1.7 Projects and Folders
> **Status:** BUILT

```sql
CREATE TABLE projects (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    description TEXT,
    color       TEXT,
    parent_id   TEXT,               -- NULL = top-level project; set = folder
    sort_order  INTEGER NOT NULL DEFAULT 0,
    owner_id    TEXT,               -- auth-ready
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at  TEXT NOT NULL DEFAULT (datetime('now')),
    
    category              TEXT,     -- project category
    is_system             INTEGER NOT NULL DEFAULT 0,
    is_hidden             INTEGER NOT NULL DEFAULT 0,
    is_favorite           INTEGER NOT NULL DEFAULT 0, -- sidebar pin / favorite flag (#2009, replaces localStorage)
    
    -- NOTE: No FOREIGN KEY on parent_id — CRR table, enforced in app code (#493)
    -- NOTE: No UNIQUE(name, parent_id) — CRR limitation, enforced in app code (#493). See §1.7.2.
);
```

A **folder** is simply a project row where `parent_id` is not NULL. The UI renders them differently, but the data model is the same. This allows arbitrary nesting.

#### 1.7.1 Default Folder (`default_folder_id`)

> **Status:** BUILT (v1.0.60, #2874 item 50a; extended v1.0.61, #2874, PR #3040 — default folder is now reassignable and hide-guarded, see the two rulings after "Not movable either" below)

Every top-level project has exactly one default folder — the folder a note/sheet lands in when no folder is specified. **Ruling (Matt, 2026-09-24): the default folder is found by ID, never by matching the name "General."** Matching by name silently breaks the moment a user renames their default folder away from "General" — a `parent_id`/`id` pointer has no such failure mode, which is exactly why renaming the default folder is safe (§1.7.1 "Renamable, not deletable" below) while matching its name never was.

```sql
ALTER TABLE projects ADD COLUMN default_folder_id TEXT;
```

- `default_folder_id` lives on the **project** row (`parent_id IS NULL`); it is `NULL`/unused on folder rows.
- Nullable — no CRR `NOT NULL`-default concern. `projects` is already a CRR table enforcing its constraints in app code (no FK, no UNIQUE, #493); this column follows the same discipline. The upgrade step below (ruling 3) guarantees every project has a value before any read needs one.
- **No FOREIGN KEY** (CRR table, #493) — enforced in app code: every writer of `default_folder_id` validates the target row exists and has `parent_id` equal to the project being updated, i.e. it is really one of that project's own folders. This is the same "a folder ID from another project is refused" posture as ruling 10 (ai-integration.md §4.1), applied here to the data-layer write path instead of the agent-facing tool path.

**Current state being superseded.** Five call sites resolve the default folder by the literal name `'General'` today — `SELECT id FROM projects WHERE parent_id = ?1 AND name = 'General'` — all re-verified present and byte-identical at `main` (`2a34a988a303538bdd2d546cced24feaaccf201c`):

| Site | Line (at `main`) | Context |
|---|---|---|
| `notes.rs` | 155 | `create_note_internal` — folder omitted, non-system project |
| `notes.rs` | 401 | sheet creation's own folder-omitted fallback (same guard shape) |
| `notes.rs` | 3188 | `move_note_internal`'s target-folder-omitted fallback |
| `watcher.rs` | 457 | file watcher import — no folder segment in the on-disk path |
| `import.rs` | 416 | bulk import — no folder segment in the import source |

Each becomes `SELECT default_folder_id FROM projects WHERE id = ?1` — no `name` column, no `'General'` literal — once this lands. This is the entire migration surface; nothing else resolves a default folder by name.

**Upgrade (ruling 3).** On the `v90 → v91` migration (`SCHEMA_VERSION`, `migrations.rs`), after the `ALTER TABLE` above:

1. For every top-level project (`parent_id IS NULL`) with at least one folder, set `default_folder_id` to its **oldest folder** — `MIN(created_at)` among its direct children (`parent_id = project.id`).
2. For every top-level project with **zero** folders, create one — named `"General"`, the same fallback `create_project_internal` already uses for an unspecified `first_folder_name` (§1.7) — and point `default_folder_id` at it.
3. **No review log. No tie handling.** If two folders share the exact same `created_at`, either is an acceptable default; pick deterministically (e.g. lowest `id`, for migration idempotency only — not a user-facing rule) and move on. Pane has two users; a reconciliation pass isn't worth building.

**Renamable, not deletable (ruling 4).** Renaming the default folder is unrestricted — `default_folder_id` is a stable pointer, so a rename never invalidates it (this is the entire motivation for moving the lookup off name matching). `delete_folder_internal` (`projects.rs`) gains a guard parallel to its existing `is_system` check at the top of that function: before deleting, read the parent project's `default_folder_id`; if the folder being deleted IS the default, reject (mirroring the existing `"Cannot delete system folder"` short-circuit) with an equivalent message, e.g. `"Cannot delete the default folder"`. There was no flow in #2874 items 50a/50b's scope for re-pointing `default_folder_id` to a different folder first — that UI/tool action didn't exist yet. **It exists now** (PR #3040, see "Reassignable via Project Settings" below); until a project's default folder is deliberately reassigned through that flow, it stays exactly where the upgrade step (ruling 3) or project creation (`create_project_internal`) put it — a folder can never delete, move, or hide (see below) its own way out from under being the default.

**Not movable either, not even within its own project (#2874 item 50b).** `move_folder_internal` (`projects.rs`) gains the identical guard, mirrored from `delete_folder_internal`'s above: before reparenting, read the folder's *current* parent project's `default_folder_id`; if the folder being moved IS that project's default, reject with an equivalent message, e.g. `"Cannot move the default folder"` — this project's default folder must remain one of its own folders at all times, and a move (even to a destination that is itself still within the same project) is exactly the operation that would put that invariant at risk, so it is refused unconditionally rather than re-litigated per destination.

**Reassignable via Project Settings (#2874, PR #3040).** `default_folder_id` is no longer permanent once set — `update_project`'s `UpdateProjectRequest` gains `default_folder_id: Option<String>` (absent/`None` = don't touch, matching `name`/`description`/`color`'s existing convention on the same request); a `Some(folder_id)` re-points the project's default. Before the write lands, `validate_default_folder_choice` (`projects.rs`) refuses three cases, each its own branch so the error names which rule fired, and each read straight off the `projects` row rather than trusted from the caller:
- the folder doesn't exist, or belongs to a **different project** (`parent_id` must equal the project being updated, exactly)
- the folder is a **system** folder (`is_system`)
- the folder is a **hidden** folder (`is_hidden`)

The UI entry point is the "Default folder" `<select>` in `ProjectSettings.svelte` (edit mode only), populated from `get_folders(project_id)` filtered to the same visibility bar `list_visible_folders` (ai-integration.md §4.1, #2874 item 50b) already established for the agent — system and hidden folders excluded client-side too, so the dropdown never offers a choice the backend would refuse anyway — pre-selected from a fresh `get_project_or_folder` read, and saved through the same `update_project` call the project's name-change path already uses. No new command.

**Hiding the current default is refused too (#2874, PR #3040).** `toggle_folder_hidden` (`projects.rs`) gains the inverse of the delete/move guards above: when `hidden == true` (un-hiding is always allowed) and the target folder is the project's *current* `default_folder_id`, the call is refused — `"Cannot hide the default folder. Choose a different default in Project Settings first."` — rather than silently leaving `default_folder_id` pointing at a folder now excluded from `list_visible_folders`'s agent-facing listing and from `validate_default_folder_choice`'s own "not hidden" bar above. Hiding a **non**-default folder is unaffected. This guard and `delete_folder_internal`'s share one lookup, `project_default_folder_id(conn, project_id)` (`projects.rs`) — one helper answering "is this folder the project's current default," not two copies that could drift apart on what "default" means.

**Viewing-folder-missing fallback (ruling 6).** When a note/sheet-creation caller supplies "the folder the user is currently viewing" as context rather than an explicit `folder_id` (the context-derived branch, `ai::tools::list_visible_folders`'s caller area) and that folder no longer exists (deleted since the context was captured), the write does **not** fail and does **not** fall through to project-root placement — it lands in the project's `default_folder_id`, and the caller is told this happened. See ai-integration.md §4.1 for the exact tool-result shape this produces.

#### 1.7.2 Folder Name Uniqueness

> **Status:** BUILT (v1.0.60, #2874 items 50a/50b) — enforcement is going forward only; the agent-facing duplicate error shape (naming every colliding folder by ID, `execute_create_folder`'s `check_folder_name_unique_for_agent`) is #2874 item 50b, now BUILT too.

**Ruling 8: two folders in the same project can never share a name.** Today this is unenforced everywhere — `create_folder` (sidebar, `projects.rs:565`) and `execute_create_folder` (agent, `ai::tools`) both insert unconditionally with no sibling-name check, `move_folder_internal` (`projects.rs:448`) and `execute_move_folder` (`ai::tools`) reparent a folder with no sibling-name check at the destination either, and `import.rs`/`watcher.rs`'s folder-creation branches only reuse an existing folder when their own by-name lookup happens to succeed — a race or a case-sensitivity mismatch can still produce two same-named siblings today.

Per the existing CRR constraint note on `projects` above ("No UNIQUE(name, parent_id) — CRR limitation"), this cannot be a database-level `UNIQUE` index — cr-sqlite rejects `UNIQUE` on CRR tables. It is enforced by **one shared backend check**, called from every path that creates, renames, or moves a folder:

- the sidebar (`create_folder`, and the rename path, `update_folder` (`projects.rs:654`), both `projects.rs`)
- the agent (`execute_create_folder`, and the move path, `execute_move_folder`, both `ai::tools`)
- import (`import.rs`'s folder-creation branch)
- the watcher (`watcher.rs`'s folder-creation branch)
- the shared move primitive, `move_folder_internal` (`projects.rs:448`) — called by `execute_move_folder` above, and by the Tauri `move_folder` command (`notes.rs:3271`), which has **no current frontend caller** (`moveProject`, the sidebar's would-be entry point in `api.ts`, is an unimplemented v9 stub that returns the project unchanged); the sidebar itself has no live folder-move UI today, so this is the agent's gap to close, not the sidebar's

The check: `SELECT 1 FROM projects WHERE parent_id = ?1 AND LOWER(name) = LOWER(?2) AND id != ?3 LIMIT 1` — the `id != ?3` exclusion (the folder's own id, or a sentinel for "not yet created") lets the identical check cover create, rename, and move alike. For a move, `?1` is the **destination** parent (`target_project_id`, not the folder's current parent) and `?2` is the folder's own unchanged name — the same query, just run against where the folder is going rather than where it already lives. A hit rejects the write.

**Pre-existing duplicates** (from the unguarded paths above, predating this check): no backfill de-duplication is specified here; existing duplicate siblings are left as-is. **For the agent specifically**, when an attempted create/rename collides with an existing sibling, the error names **both** colliding folders by ID, and the agent is expected to retry **by ID**, not by trying another name — see ai-integration.md §4.1.

**Hidden and system folders participate in this check** even though they're invisible to by-name *lookup* (ruling 9, ai-integration.md §4.1) — an agent-visible name still collides with a hidden/system folder holding that same name at the DB level; ruling 9 only governs whether a hidden/system folder can be *found* by name, never whether its name blocks a new sibling from taking it.

### 1.8 Attachments

```sql
CREATE TABLE IF NOT EXISTS attachments (
    id TEXT PRIMARY KEY,                          -- UUIDv4, server-generated per upload; also the on-disk filename stem
    project_id TEXT NOT NULL DEFAULT '',           -- project UUID (was: name/thread-id — fixed at call sites)
    project_name TEXT NOT NULL DEFAULT '',         -- denormalized for disk-path build; updated on project rename
    original_filename TEXT NOT NULL DEFAULT '',    -- human name; stored in markdown alt text, not on disk
    mime_type TEXT NOT NULL DEFAULT '',
    size_bytes INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
    -- No FOREIGN KEY, no UNIQUE beyond PK — CRR table once added to CRR_TABLES; enforce in app code (#493)
);
```

**Changes vs. live (pre-#1775):**
1. `project_id` now holds a **UUID** (call-site fixes in `Editor.svelte` and `ChatSidebar.svelte` per #1775 Q2; column type unchanged, gains `DEFAULT ''`).
2. New `project_name TEXT NOT NULL DEFAULT ''` (Reed #1775 Q1 denorm — avoids a join on every path build; updated on project rename alongside `notes.file_path`).
3. **DEFAULTs added to `project_id`, `original_filename`, `mime_type`, `size_bytes`.** Required because the table is being added to `CRR_TABLES`: the `test_crr_tables_not_null_columns_have_defaults` invariant rejects any `NOT NULL` column without a `DEFAULT` on a CRR table. Without this, the build breaks. (Session push-back P2.)

**Not added:** `render_mode`, `attachment_type`, `duration`, `page_count`, `width`, `height`. Render-mode and type are **per-embed TipTap node attributes** (file-system §2.1A / #401 PR1), not per-file DB columns. Dimensions are dropped per the locked #1774 design. Do not add these to `attachments`.

**Reference linkage (unchanged):** attachments are not FK-linked to notes. Note embeds reference by relative markdown path (file-system §2.1) + `data-attachment-id` node attribute; chat references via `chat_messages_local.attachment_ids` (comma-separated).

#### Migration contract (v66 → v67)

**Version:** `SCHEMA_VERSION` `66` → `67` (`migrations.rs`). Per ADR-0017, add a v66→v67 match arm **and** append the v66→v67 step to every terminal arm that currently lands at the v66 state; the version-coverage test enforces completeness.

**v66 → v67 steps (idempotent):**

1. **Add columns.** `ALTER TABLE attachments ADD COLUMN project_name TEXT NOT NULL DEFAULT '';` plus the DEFAULT backfill for `project_id`/`original_filename`/`mime_type`/`size_bytes` (column DEFAULTs apply to new rows; existing rows already have values). Follow the existing idempotent `ALTER` pattern (`migrate_embeddings_engine_column` — swallow "duplicate column name").
2. **Backfill `project_name`** from `projects` where resolvable: `UPDATE attachments SET project_name = (SELECT name FROM projects WHERE id = attachments.project_id) WHERE project_name = '' AND project_id IN (SELECT id FROM projects);` — run **after** step 3 if project_id is still a name (see ordering note).
3. **name → UUID** for editor-origin rows (#1775 Q2):
   ```sql
   UPDATE attachments
   SET project_id = (SELECT id FROM projects WHERE name = attachments.project_id LIMIT 1)
   WHERE project_id IN (SELECT name FROM projects);
   ```
   Rows whose `project_id` matches no project name (chat-thread ids, `__global__`, `workspace`) **remain unmigrated** — these are the out-of-scope thread/global attachments (push-back P3); do not invent a project for them here.
4. **One-time file move** (production installs with existing files): for each row, move `~/.pane/attachments/{old_segment}/{uuid}.{ext}` → `Projects/{project_name}/Assets/{uuid}.{ext}`, then archive/remove the legacy dir. Run before the app serves any attachment read. Inside a transaction; move files before DB commit; roll back on file error (mirror the #470 migration safety pattern).
5. **CRR enable:** add `"attachments"` to `CRR_TABLES` (`[20]`→`[21]`). `enable_crr()` will call `crsql_as_crr('attachments')`, creating `attachments__crsql_clock` / `attachments__crsql_pks` (keyed on `id`). Gate on the DEFAULTs from the AFTER schema being present (step 1) or `verify_crr_tables` + the NOT-NULL-default test will fail.

**Ordering note:** step 3 (name→UUID) must precede step 2's UUID-based backfill, or sequence step 2 to read the name directly before overwrite. Simplest correct order: capture name → set `project_name` → set `project_id` = UUID.

**Constraint-collision:** the only key is `PRIMARY KEY (id)`; `id` is untouched by both the name→UUID `UPDATE` and the file move, so neither operation can collide. No UNIQUE indexes exist.

---

### 1.9 Subtasks

> **Status:** TARGET — NOT BUILT

Subtasks are the line items of the Advanced Checklist note component (§31.3). A subtask is **not a note and not a task** — it is a component line item owned by a note. It lives in a dedicated table, never in `notes`, and never appears on a task surface. See ADR-0057.

> **📌 Why This Exists — a separate table, not child notes (2026-07-18, ADR-0057):** Modeling subtasks as child notes (`notes.parent_id`) was considered and rejected. Child notes are notes: every task surface — `get_tasks`, the kanban loader (`get_project_notes_for_kanban`), All Tasks, task counts, FTS search, and Due Today itself — would have to add a `parent_id IS NULL` guard to keep them out, and a single missed guard leaks subtasks onto the board or into counts. A dedicated table excludes subtasks from every notes query **by construction** — there is nothing to guard. This is the mechanization-over-discipline choice (ADR-0008 lineage): the shared-query blast radius is removed structurally rather than by a rule every future query author must remember.

```sql
CREATE TABLE subtasks (
 id TEXT NOT NULL PRIMARY KEY,
 note_id TEXT NOT NULL DEFAULT '', -- parent note; no FK (CRR table, enforce in app code #493)
 text TEXT NOT NULL DEFAULT '',
 due_date TEXT, -- ISO 8601 date; NULL = no due date
 done INTEGER NOT NULL DEFAULT 0,
 completed_at TEXT, -- timestamp when done flipped 0→1; NULL otherwise
 sort_order INTEGER NOT NULL DEFAULT 0,
 created_at TEXT NOT NULL DEFAULT (datetime('now')),
 updated_at TEXT NOT NULL DEFAULT (datetime('now'))
 -- NOTE: No FOREIGN KEY on note_id — CRR table, enforce in app code (#493)
 -- NOTE: No UNIQUE beyond PK — CRR limitation (#493)
);

CREATE INDEX idx_subtasks_note ON subtasks(note_id);
CREATE INDEX idx_subtasks_due_date ON subtasks(due_date) WHERE due_date IS NOT NULL;
```

**CRR membership.** `subtasks` is a CRR table (subtasks sync across a user's devices like notes do). It is added to `CRR_TABLES` (21 → 22). Every `NOT NULL` column carries a `DEFAULT` so the table passes `test_crr_tables_not_null_columns_have_defaults` (`note_id`, `text`, `done`, `sort_order`, `created_at`, `updated_at` all defaulted; `id` is PK and exempt; `due_date` and `completed_at` are nullable). The default-`''` on `note_id` satisfies the CRR invariant; application code always writes a real parent id on insert.

**Schema wiring (implementation notes — BUILT-side, for the code PR):**

- Add `subtasks` to `CRR_TABLES` in `migrations.rs` (count 21 → 22; the `enable_crr` / `verify_crr_tables` log lines and the `test_fresh_install` table-count assertion update accordingly).
- Add `create_subtasks_table(conn)` to `init_schema` (a new "AI/content" table creation call).
- Bump `SCHEMA_VERSION` "72" → "73".
- **Pre-launch: no v72→v73 migration arm.** Per the zero-users principle, the fresh-install `init_schema` path is the only one that matters; nuke the dev DB. A v72→v73 arm would never execute for a real user. Matt's existing dev DB (v72) is nuked, not migrated.

#### 1.9.1 Constraints and collision behavior

`subtasks` has exactly one constraint: `PRIMARY KEY(id)`. No composite keys, no UNIQUE indexes.

| Constraint | Collision behavior |
| ----------------- | ------------------------------------------------------------ |
| `PRIMARY KEY(id)` | `id` is a fresh UUIDv4 per insert. No natural-key collision is possible; a PK collision would be a UUID reuse bug (error/rollback), never expected in normal operation. |

There is no secondary UNIQUE constraint, so there is no other collision surface. (`note_id` is intentionally non-unique — a note has many subtasks.)

#### 1.9.2 Reads

- **Checklist component** (§31.3): `SELECT … FROM subtasks WHERE note_id = ?1 ORDER BY sort_order`.
- **Due-date watcher** (§19.5.1b): scans `subtasks` joined to `notes` for due, undone items under non-trashed parents.
- **Due Today** (§31.2): subtask half of the union query.

#### 1.9.3 Writes

Item operations map to writers (built by the code PR, not here): create, update text, set/clear `due_date`, toggle `done` (sets/clears `completed_at`), reorder (`sort_order`), delete. Each write also triggers the scoped notification reconciliation in §19.5.1b/§19.5.1c.

#### 1.9.4 Deletion and cascade (data-layer rules)

Subtasks have **no soft-delete** (`deleted_at`) of their own — they are line items; the trashable unit is the parent note. The rules:

1. **Item deleted** (user removes a checklist line): hard-`DELETE` the `subtasks` row. Companion-clear its notifications (§19.5.1c).
2. **Parent note trashed** (`notes.deleted_at` set): subtasks are **not** deleted — they ride the parent. They stop surfacing because every read joins on `notes.deleted_at IS NULL`. Their pending notifications auto-dismiss in the same write tick (§19.5.1c).
3. **Parent note restored** (`notes.deleted_at` cleared): subtasks resurface. Their prior notification rows are companion-cleared (`DELETE`) so the watcher re-emits fresh — mirrors the note done→todo re-fire path (§19.5.1a); `DELETE` not `UPDATE` is load-bearing so no tombstone blocks re-emit.
4. **Parent note hard-deleted** (permanent): cascade hard-`DELETE` the note's `subtasks` rows (app-code cascade — CRR = no FK) and `DELETE` their notification rows. No audit value in retaining line items of a permanently-removed note.

---

### 1.10 Templates

> **Status:** TARGET — NOT BUILT

Templates are named snapshots of note content (§32, ADR-0058): body markdown plus the checklist item payload the markdown cannot carry (§31.3.3). A template is not a note and not a task — it never occupies a `notes` row, has no file on disk, and appears on no note or task surface. Structural exclusion per the ADR-0057 pattern.

```sql
CREATE TABLE templates (
 id TEXT NOT NULL PRIMARY KEY,
 name TEXT NOT NULL DEFAULT '',
 content TEXT NOT NULL DEFAULT '', -- body markdown; due-today sentinels stored dateless (§32.1)
 checklist_json TEXT NOT NULL DEFAULT '[]', -- [{"text": "...", "sort_order": N}] — text + order only, never done/due (§32.1)
 created_at TEXT NOT NULL DEFAULT (datetime('now')),
 updated_at TEXT NOT NULL DEFAULT (datetime('now'))
 -- NOTE: No UNIQUE on name — CRR table, uniqueness app-enforced (#493, §32.2.2)
);

CREATE INDEX idx_templates_name ON templates(name);
```

**CRR membership.** `templates` is a CRR table (templates sync across a user's devices). `CRR_TABLES` 22 → 23. Every `NOT NULL` column carries a `DEFAULT` (`name`, `content`, `checklist_json`, `created_at`, `updated_at`; `id` is PK and exempt), passing `test_crr_tables_not_null_columns_have_defaults`.

**Schema wiring (implementation notes — for the code PR):**

- Add `templates` to `CRR_TABLES` in `migrations.rs` (22 → 23; `enable_crr`/`verify_crr_tables` log lines and the `test_fresh_install` table-count assertion update accordingly).
- Add `create_templates_table(conn)` to `init_schema`.
- Bump `SCHEMA_VERSION` "73" → "74".
- **Pre-launch: no v73→v74 migration arm.** Fresh-install path only; Matt nukes the dev DB.

#### 1.10.1 Constraints and collision behavior

`templates` has exactly one constraint: `PRIMARY KEY(id)`.

| Constraint | Collision behavior |
| ------------------------- | ------------------------------------------------------------ |
| `PRIMARY KEY(id)` | Fresh UUIDv4 per new row; the Overwrite path (§32.2.2) is an UPDATE targeting an existing id, never an insert. A PK collision would be a UUID-reuse bug (error/rollback). |
| `name` (no DB constraint) | App-enforced case-insensitive check at save: exactly one match → Overwrite-or-Rename prompt; two or more matches (sync-merged duplicates) → Overwrite requires an explicit user-picked target from the listed matches, or Rename — never an implicit target (§32.2.2). Sync-merged duplicates coexist as distinct rows; user resolves via rename/delete or the multi-match pick. |

#### 1.10.2 Reads

List all templates (grid + Settings tab, §32.4); get by id (apply path, §32.3.2). `checklist_json` is opaque payload — never queried by field, never scanned by the watcher.

#### 1.10.3 Writes

Create (save-as-template), overwrite (UPDATE content/checklist_json/updated_at on an existing id), rename, hard-DELETE. No soft-delete. No write to `templates` touches `notes`, `subtasks`, or `notifications`.

#### 1.10.4 Relationship to notes and subtasks — none, by construction

Applying a template writes into the target note's body (normal editor path) and inserts fresh `subtasks` rows on that note (fresh UUIDs, `done=0`, `due_date=NULL`, appended `sort_order` — §32.3.2). No FK, no back-reference, no cascade in either direction: template deletion touches nothing; note deletion touches no template. Because a template stores no date and no done-state, no template operation can emit a notification (§32.5).

---

### 1.11 Cabinet Drag-to-Reorder

> **Status:** BUILT (#2334)

Cabinet notes (§1.1: `status = 'cabinet'`) are a user-arranged shelf, not a system-ordered queue. This section defines how that ordering is stored, computed, and mutated — and draws the explicit line between Cabinet's manual ordering and the task columns' system ordering, which remain untouched.

#### 1.11.1 Ordering Semantics: Cabinet vs. Task-Column

The `notes` table has always carried `sort_order INTEGER NOT NULL DEFAULT 0` (§1.3), but before #2334 it was write-only for Cabinet notes — nothing read it for the Kanban board. Two ordering regimes now coexist in a single query (`get_project_notes_for_kanban`), keyed off `status`:

- **Cabinet column** (`status IS NULL OR status = 'cabinet'` — the `IS NULL` disjunct exists because pre-#1935 rows and a handful of import/query paths still produce a NULL status that means the same thing as `'cabinet'`, per the canonical predicate note in §1.1): a **user-arranged shelf**. Order is `sort_order ASC`, ties broken by `created_at DESC` (newest first). This order is driven entirely by explicit user drag actions (§1.11.3) and by where new notes land (§1.11.2) — there is no system-computed component.
- **Task columns** (`todo`, `in_progress`, `review`, `done`): a **system-ordered workboard**, unchanged by #2334. Order is `priority` (`urgent > high > medium > low`, NULL last) then `due_date` ascending — identical to the pre-#2334 behavior and to `get_tasks`'s own ordering. `sort_order` has **zero effect** on task-column order; it is written (see §1.11.2) but never read for these rows.

This is intentional, not an oversight: a Cabinet is a backlog a human curates by hand ("this idea matters more than that idea, no algorithm can know that"); a task column is a workboard where priority/due-date already encode the ordering signal that matters, and manual reordering there would fight the system order on every reload.

**The reorder surface is project-global, not per-folder.** The two UIs #2334 targets — the project Kanban board (`get_project_notes_for_kanban`, no `folder_id` filter at all) and the folder view's Cabinet column when the project itself is being viewed with no folder selected (`get_notes`'s folder_id-less branch, also `project_id`-only) — are both *aggregated* views: they show every Cabinet note in the project, spanning every folder (and the bare-project bucket), as one flat drag-reorderable list. This is *why* the reorder scope (§1.11.2) is `project_id` and not `(project_id, folder_id)`: the scope was chosen to match the surface users actually drag on. A single-folder-scoped view (folder view with a folder selected) still shows a strict subset of that same project-wide ordering, filtered by `folder_id` at query time — it does not get its own independent ordering regime.

Both regimes are computed by **one SQL query** with per-branch `CASE WHEN status IS NULL OR status = 'cabinet' THEN … ELSE … END` expressions for each of the four `ORDER BY` terms (sort_order, priority, created_at, due_date) — a single source of ordering truth, no post-query re-sort in Rust or the frontend *for freshly-fetched data*. The frontend (`KanbanBoard.svelte`'s `getTasksForColumn`, `NotesBrowser.svelte`'s Cabinet/Tasks filters) then partitions this one result set into per-column arrays by `status`; because each `CASE` branch only ever compares rows *within* its own status group, cross-group row interleaving in the raw SQL result is irrelevant — only intra-group order is a contract.

**Frontend render contract (post-#2334-fix):** the "no post-query re-sort" rule above holds only for data that just came back from the backend already ordered by the query above. It does **not** hold for the frontend's own optimistic in-memory reorder (§1.11.3): after a drag-reorder, the optimistic update rewrites each affected row's `sort_order`/`task_sort_order` **field** but does not (and structurally cannot, via `.map()`) change the row's **position** in the `tasks`/`taskNotes` array. Cabinet column views MUST render notes sorted by `sort_order ASC` (with `created_at DESC` tiebreak) to reflect user-arranged manual ordering — this sort must be applied at the render/partition boundary (`getTasksForColumn('cabinet')` in KanbanBoard.svelte, the `cabinetNotesLive` derivation in NotesBrowser.svelte), not assumed from array order. Task columns render by priority/due_date system ordering and are explicitly exempt from this sort — array/backend order is sufficient there since nothing ever optimistically reorders a task column's array position.

The folder view's `get_notes` query (`ORDER BY sort_order, created_at DESC`, unconditional) requires no change: it was already correct for Cabinet reordering before #2334, since the folder view shows exactly the notes for which `sort_order` is the intended sort key. It does not separately encode task-column ordering, so there is no conditional to add.

#### 1.11.2 Persistence Model

**Column:** `notes.sort_order` (§1.3), already `NOT NULL DEFAULT 0` — no schema change, no migration.

**New-note-on-top placement (gap-based):** a newly created Cabinet note (`create_note_internal`, and `import_note_file` for the filesystem-rescan path) is placed **above every existing Cabinet note in its project** by computing `COALESCE(MIN(sort_order), 0) - 1000`, scoped to the note's `project_id` **only** — `sort_order` is one number line per project, not per folder (ruled semantics, superseding the original per-folder scoping in the initial #2334 landing; see the scope-mismatch fix below). This mirrors the existing `COALESCE(MAX(sort_order), -1)` append-offset precedent in `templates.rs`'s `apply_template` (subtask append) — same pattern, opposite end of the list, since new Cabinet notes go to the *top* rather than the bottom. A new note in any folder therefore tops the **entire project's** Cabinet list, not just its own folder's subset — this is what makes it appear at the top of the aggregated views (§1.11.1) it was designed for, rather than landing mid-list there while only appearing on top within its own folder. Task-column-status notes are unaffected: they get `sort_order = 0` (irrelevant per §1.11.1, since task columns never read it).

The `-1000` gap (rather than `-1`) leaves 999 integer slots between any two adjacent Cabinet notes for future insertions or fractional reordering without a renumbering pass, and matches the step size used by the drag-reorder persistence below — one convention across both write paths.

**Reorder persistence (`reorder_notes`):** takes a batch of `(note_id, sort_order)` pairs and writes them in a single call. #2334 hardens this over its pre-existing bare update-loop with two invariants, both covered by `reorder_notes_hardening_tests`:

1. **Transaction with rollback.** The batch runs inside `BEGIN`/`COMMIT`; any individual `UPDATE` failure triggers `ROLLBACK`, so a mid-batch failure can never leave some rows reordered and others not.
2. **Project-scope validation.** Every note in a batch must resolve to the same `project_id` (looked up before any write begins) — `folder_id` is free to vary within a batch. A batch spanning two different **projects** is rejected outright — no query executes, nothing is written. **Ruled semantics (supersedes the original per-`(project_id, folder_id)` scoping):** the initial #2334 landing scoped reorder batches by `(project_id, folder_id)`, which rejected every batch produced by the aggregated views themselves (§1.11.1) — both the Kanban board and the folder-view-at-project-root Cabinet column construct their drag batches from *every* Cabinet note in the project, spanning multiple folders, by design (see §1.11.1's "reorder surface is project-global" note). The fix relaxes the validation to `project_id` alone so it matches the scope the frontend actually reorders within, while still rejecting the one case that indicates a genuine caller bug: a batch spanning two different projects (cross-project rejection, and unknown-note-id rejection, are both unchanged from the original hardening).

The frontend's drag handlers always renumber the *entire* visible Cabinet list on every successful drop (gap step 1000, matching §1.11.2's new-note gap), then persist the whole batch via `entityService.reorderNotes`. This trades a slightly larger write per drag for never having to reason about partial-list renumbering or fractional-order edge cases at the UI layer.

#### 1.11.3 Drag Interaction Surface

Both Cabinet-bearing views — `NotesBrowser.svelte`'s folder view and `KanbanBoard.svelte` — gained **per-card** drag handlers on Cabinet cards (previously, only column-level drop targets existed, for status-change drags). The two implementations are structurally identical, following the splice-and-renumber pattern already established by `ChecklistNodeView.svelte`'s subtask reorder:

1. **Card-level drop, not column-level.** Each Cabinet card is wrapped in a small `.cabinet-drag-wrap` element carrying its own `ondragover`/`ondragleave`/`ondrop`/`ondragend` handlers, layered *inside* the pre-existing column-level status-change drop target (`handleColumnDrop` in NotesBrowser, `handleDrop` in KanbanBoard).
2. **stopPropagation on the card-level handlers.** Every card-level `dragover`/`drop` handler calls `stopPropagation()`, so a drop landing directly on a Cabinet card never also fires the column-level handler for the same gesture (the #2325 lesson: a naive card-in-column drop target double-fires both handlers without this).
3. **Drag source determines mode, not drop target.** A drag that *originates* on a Cabinet card sets a Cabinet-only reorder identity (`cabinetDragSourceId` in NotesBrowser, same name in KanbanBoard) in addition to the pre-existing status-drag identity (`draggedNote` / `draggedTask`) — both are set together at drag-start, deliberately, because a Cabinet-origin drag that gets dropped *outside* any card (into empty Cabinet space, or another column entirely) still needs to fall through to the ordinary column-level status-change handler unmodified. A drag that originates on a **task-column** card only ever sets the status-drag identity, never the Cabinet-only one. The card-level drop handler branches on which identity is present for the specific gesture: `cabinetDragSourceId` set → reorder; absent (only the status-drag identity present) → status-change to `'cabinet'`, identical in effect to dropping in empty Cabinet column space.
4. **CSS drop-indicator exception for same-column Cabinet drags.** The column-level highlight in KanbanBoard.svelte was originally suppressed for same-column drags (`draggedTask?.status !== column.status`, since a same-column drop used to mean "no-op, nothing changes"). That suppression now carries an explicit `|| column.status === 'cabinet'` exception, because a same-column Cabinet drag is no longer a no-op — it's a reorder, and needs its own border-top drop indicator (`.cabinet-drag-wrap.is-drag-over`), separate from the column-level background wash so the two affordances stay visually distinct.
5. **Cleanup clears both identities.** `cabinetDragEnd`/`cabinetCardDragEnd` always clear the Cabinet-only reorder identity, the drag-over-id, *and* the shared status-drag identity together, so a cancelled or completed drag never leaves stale state that a later, unrelated gesture could misread.

**Non-goal:** task columns (`todo`/`in_progress`/`review`/`done`) get no new drag-reorder affordance. Their cards remain drag-*sources* only for the pre-existing cross-column status-change drop (§1.11.1's ordering justification applies here too — there is no user-curated order to preserve on a system-ordered board).

#### 1.11.4 Sync Behavior

`sort_order` rides the existing CRR sync path for `notes` unchanged — no new sync logic, no new conflict-resolution code. This means the standard cr-sqlite last-writer-wins column-level merge applies to concurrent Cabinet reorders exactly as it does to any other note field.

**Accepted risk (not fixed by #2334):** if two devices reorder the same Cabinet concurrently while offline, each device's batch of `sort_order` writes merges independently, column-by-column, under last-writer-wins. The *result* is deterministic (every device converges to the same final state) but is not guaranteed to reflect either user's *intended* final order — it can interleave the two reorderings in a way neither user actually performed. This is the same class of risk that already exists for any concurrent multi-field edit under CRR and is treated the same way: accepted, not specially mitigated, because the alternative (an operational-transform or CRDT-sequence type for ordering) is out of scope for a personal/small-team product at this stage. A future revisit is plausible if reorder conflicts prove disruptive in practice, but there is no ticket for it as of this PR.

#### 1.11.5 Backward Compatibility

Every Cabinet note that existed before #2334 has `sort_order = 0` (the column's pre-existing schema default — no migration ran, none was needed). On first load after upgrade, this means:

- All pre-existing Cabinet notes tie on `sort_order = 0` and fall back to the `created_at DESC` secondary sort (§1.11.1) — i.e., cold-start Cabinet order is "newest first," identical to the pre-#2334 default `ORDER BY sort_order, created_at DESC` folder-view behavior notes already used. No visual jump, no re-sort surprise on upgrade.
- The gap-based new-note placement (§1.11.2) still works correctly against an all-zero baseline: the first new note after upgrade computes `COALESCE(MIN(sort_order), 0) - 1000 = -1000`, landing above every pre-existing (0-valued) note as expected.
- The first user drag on a never-before-reordered Cabinet renumbers the *entire visible list* at gap-1000 spacing (§1.11.2), so from that point on every note in that scope has a distinct, meaningful `sort_order` — the transition from "everything is 0" to "gap-spaced order" happens atomically on first drag, not incrementally.
- No forced migration, no backfill script, no version bump required for existing installs to pick up this behavior — purely a function of the ordering query and the write paths described above.

---

### 1.12 Sheets — Grid Side Tables

> **Status:** Schema BUILT (v1.0.47, #2339, schema-only — CRUD/predicates/sync
> wiring still TARGET, later Sheets PRs). See §33, ADR-0066.

A sheet is a `notes` row with `note_type = 'sheet'` (ADR-0066). The notes row
carries placement, task metadata, tags, pinning, and trash; it is plumbing,
not presentation. Grid data lives in three CRR side tables keyed by
`note_id`, mirroring the `subtasks` pattern (§1.9). Canonical sheet content
is these tables — a sheet has no file on disk (`file_path = ''`) and
`notes.body` holds only a generated flat-text copy (§33.4). Per-sheet CSV
export is the v1 out-of-app access mechanism (ADR-0066 D8).

```sql
CREATE TABLE sheet_columns (
    id TEXT NOT NULL PRIMARY KEY,
    note_id TEXT NOT NULL DEFAULT '',
    name TEXT NOT NULL DEFAULT '',
    col_type TEXT NOT NULL DEFAULT 'text', -- 'text'|'number'|'currency'|'date'|'checkbox'|'select'
    options_json TEXT NOT NULL DEFAULT '{}', -- type config: select choices, currency code (§33.3.2)
    width INTEGER NOT NULL DEFAULT 0, -- px; 0 = type default; syncs (Matt ruling 2026-09-03)
    sort_order INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
    -- No FK on note_id — CRR table, app-enforced (#493)
);
CREATE INDEX idx_sheet_columns_note ON sheet_columns(note_id);

CREATE TABLE sheet_rows (
    id TEXT NOT NULL PRIMARY KEY,
    note_id TEXT NOT NULL DEFAULT '',
    sort_order INTEGER NOT NULL DEFAULT 0,
    height INTEGER NOT NULL DEFAULT 0, -- px; 0 = default; syncs (Matt ruling 2026-09-03)
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
    -- No FK on note_id — CRR table, app-enforced (#493)
);
CREATE INDEX idx_sheet_rows_note ON sheet_rows(note_id);

CREATE TABLE sheet_cells (
    row_id TEXT NOT NULL,
    column_id TEXT NOT NULL,
    note_id TEXT NOT NULL DEFAULT '', -- denormalized for one-scan reads/cascades; NOT part of the PK
    value TEXT NOT NULL DEFAULT '', -- raw user value, or canonical formula string ('=' prefix, §33.5.2)
    style_json TEXT NOT NULL DEFAULT '{}', -- bg/text color, border (§33.6)
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (row_id, column_id)
    -- No FKs — CRR table, app-enforced (#493)
);
CREATE INDEX idx_sheet_cells_note ON sheet_cells(note_id);
```

**Sparse storage.** Empty cells are never stored. Clearing a cell is a
`DELETE`, not a write of `''`. A `sheet_cells` row exists iff the cell has a
value or a non-default style.

**Computed values are never persisted here.** No column on any of these
tables holds a formula result. Results live in the in-memory grid store and
are recomputed locally (§33.5.4). This is structural enforcement of the
never-sync invariant — there is no synced column a computed value could
occupy.

**CRR membership.** All three tables are CRR (`CRR_TABLES` 24 → 27). Every
`NOT NULL` column carries a `DEFAULT` (PKs exempt), passing
`test_crr_tables_not_null_columns_have_defaults`. `sheet_cells` uses a
composite PK — in-repo precedent `note_tags (note_id, tag_id)`, verified
live (`migrations.rs:1165–1173`, source-read 2026-09-03): composite-PK CRR
member since inception; cr-sqlite encodes all PK columns into the `pk` BLOB
in `crsql_changes`.

**Schema wiring (implementation notes — BUILT, v1.0.47, #2339):**

- Added a `create_sheet_tables(conn)` fn (combined, per file convention) to
  `init_schema`; appended `"sheet_columns"`, `"sheet_rows"`, `"sheet_cells"`
  to `CRR_TABLES` (`[&str; 24]` → `[&str; 27]`; `enable_crr`/
  `verify_crr_tables` log lines updated accordingly).
- Bumped `SCHEMA_VERSION` `"89"` → `"90"`.
- **Post-launch policy: real v89→v90 migration arm** (ADR-0017 shape,
  matching the v88→v89 arm: migration backup, idempotent
  `CREATE TABLE IF NOT EXISTS` × 3, `crsql_as_crr` × 3,
  `verify_crr_tables`, version stamp), delegating to a new `migrate_v90` fn.
  New upgrade fixture `tests/fixtures/upgrade-from-v89.db` +
  `scripts/build-v89-fixture.sh`.
- `test_fresh_install` table-count floor 42 → 45;
  `test_all_indexes_exist` count 63 → 66 (3 new sheet indexes).
- Schema-only PR — no business logic, CRUD, or predicates. Those land in
  later Sheets PRs (CRUD backend, sync engine, integrity sweep allowlist).

#### 1.12.1 Constraints and collision behavior

| Constraint | Collision behavior |
| ------------------------------------------------ | ------------------------------------------------------------ |
| `sheet_columns` / `sheet_rows` `PRIMARY KEY(id)` | Fresh UUIDv4 per insert; collision = UUID-reuse bug → error/rollback, never expected. |
| `sheet_cells` `PRIMARY KEY(row_id, column_id)` | The natural key IS the PK: one row per grid coordinate, duplicates impossible by construction. Local cell write is an UPSERT on the pair. Two devices concurrently creating the same coordinate merge via CRR LWW per column — expected behavior, not a violation. |
| `note_id` on all three (no DB constraint) | App-enforced: writers require an existing, non-deleted `notes` row with `note_type = 'sheet'`. Orphans possible via concurrent sync deletes; swept by the cascade + orphan pass (§33.8). |

#### 1.12.2 Deletion path (app-code cascade — CRR = no FK)

**Delete mechanism.** Cascade steps below use bare hard-`DELETE` on the three
CRR tables. This is a stated, mechanism-named exemption from the
tombstone-then-delete house rule: for CRR tables, delete tracking is provided
by cr-sqlite itself — a DELETE is a first-class changefeed event (the
causal-length `cl` field in `crsql_changes`, `INTEGER NOT NULL` per the
shipped binary's schema, extracted and applied by `engine.rs`), and the
delete record persists in the table's shadow metadata after the row is gone.
The ledger the house rule protects is, here, maintained by the extension;
there is no app-side dedup ledger a bare DELETE erases. The sync-client
session cleanup that motivated the rule deletes from a **non-CRR** app
ledger — different system, rule still applies there. In-repo precedent:
§1.9.4 subtasks cascade (hard-`DELETE` on the CRR `subtasks` table;
"`DELETE` not `UPDATE` is load-bearing"). Known residual: cr-sqlite deletes
are LWW, not hard tombstones — a concurrent edit with a higher causal clock
resurrects the row (sync.md:100, "Edit wins - resurrect with edits") —
acknowledged, deferred to the multi-device arc, and backstopped by the
orphan sweep (§33.8, §33.10). Version basis: spec pin `v0.16.0+`
(sync.md:36), bundled `crsqlite.dylib`; binary version not independently
extractable — standing gap flagged in session notes, not Sheets-blocking.

1. **Sheet soft-deleted** (`notes.deleted_at` set): side tables untouched;
   grid hidden via parent, same as subtasks riding their note (§1.9.4).
2. **Sheet restored**: side tables were never touched; grid resurfaces.
3. **Sheet hard-deleted**: cascade hard-`DELETE` all `sheet_columns`,
   `sheet_rows`, `sheet_cells` rows for the `note_id`, in the same
   transaction as the notes-row delete.
4. **Column deleted**: `DELETE` the column row + all `sheet_cells` with that
   `column_id`, one transaction. Row deletion: same shape on `row_id`.
5. **Orphan sweep**: one integrity-module implementation, two triggers — at launch on the startup integrity path, and after each `apply_changesets` batch touching sheet tables, scoped to affected `note_id`s and ordered before post-sync recompute (§33.8).

