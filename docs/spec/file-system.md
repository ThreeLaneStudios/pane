# File System Architecture

**Last Updated:** 2026-10-12  
**Source:** Extracted from pane-architecture-spec.md  
**Sections:** §2, §2A, §2B, §2C, §2D, Appendix B

---

## 2. File System Structure
> **Status:** BUILT (v11.28.24) — flat file write-back working

```
~/Pane/
├── .pane/
│   ├── pane.db                    # SQLite database — all structured data
│   ├── config.json                # App settings, AI provider config, UI prefs
│   └── ai/
│       ├── workspace-memory.md    # Cross-project AI memory (personal layer)
│       └── projects/
│           └── {project-id}.md    # Per-project AI memory (work layer)
│
├── Projects/
│   └── {project-name}/
│       ├── {folder-name}/
│       │   └── {note-slug}.md     # Markdown files — the actual notes
│       └── Assets/                # Attachments: images, video, audio, PDFs
│
└── Exports/                       # User-generated exports (JSON, CSV, markdown zips)
```

### 2.1 Design Principles
> **Status:** BUILT

- **Pane always resolves its root folder to its real path.** `get_pane_root()` canonicalizes `~/Pane` (or `PANE_TEST_ROOT` in tests) to its real, symlink-free path — the same resolution `PaneFs` already performs — so a Pane folder reached through a symlink (external volume, synced directory, home-dir symlink) never produces two differently-spelled roots for the same install (#3008).
- **Notes are real markdown files.** If Pane disappears, your notes are still browsable in Finder and editable in any text editor.
- **SQLite holds metadata only.** The actual content lives on disk. SQLite stores: title, project/folder IDs, task fields, tags, timestamps, file path.
- **Moving a note = file move + DB update.** Both must succeed atomically. If the file move fails, the DB update is rolled back.
- **Slugs for filenames.** `My Great Idea` → `my-great-idea.md`. Collision handling: append `-1`, `-2`, etc.
- **Assets are per-project.** Each project has an `Assets/` folder. The on-disk filename is the attachment UUID (`<uuid>.<ext>`); the human-readable name lives in the markdown alt text.
  - **Canonical reference = file-relative markdown path.** Attachments are referenced from a note via a path relative to that note's location, so the reference resolves in any external markdown viewer (the "browsable if Pane disappears" principle above):
    - note at project root → `![report](Assets/<uuid>.pdf)`
    - note one folder deep → `![photo](../Assets/<uuid>.jpg)`
    - note in a nested folder → `![photo](../../Assets/<uuid>.jpg)`
  - **Display resolution.** The WebView cannot load a relative disk path directly; the editor resolves the stored relative path to a loadable `src` at render time via Tauri's asset protocol (`convertFileSrc`). The resolved URL is runtime-only and is never written to the flat `.md` file.
  - **In-app handle.** The attachment node also carries `data-attachment-id` (the UUID) so in-app features (render-mode, type-specific cards, modals) can look up the attachment row without parsing the path. See §2.1A for render-mode persistence.
  - **Stability.** Relative references survive project rename and folder rename (the note and its `Assets/` move together, preserving the relative offset). They are rewritten by the move pipeline only when a note changes folder *depth*, or moves to a *different project* (which also copies the asset into the destination project's `Assets/`). Atomic move semantics from the "Moving a note" principle above apply to the ref rewrite.

  > **Status:** TARGET — NOT BUILT. Relative-path refs + `convertFileSrc` resolution are the canonical model; live code currently stores `attachment://uuid` and resolves it to inline base64 at editor mount (`resolveAttachmentUrls`). Migration to this model is owned by #1777 (ref rewrite + node attributes) on top of #1775 (storage-path/UUID fix). The `attachment://uuid` form is deprecated and removed by that work.
  >
  > **Amended 2026-06-29 (Session: Attachments, Decision #1):** prior wording implied this bullet was `BUILT`. It was aspirational — the relative-path principle is correct and now canonical, but the implementation diverged to `attachment://uuid`. This amendment makes the canonical form explicit, defines display resolution and the in-app handle, and corrects the status.

### 2.1A Render-Mode Persistence
> **Status:** BUILT v0.67.67 — PR #401-PR1 (feat/401-pr1-tokens-schema)
> Per-type render-mode enum implemented in `Attachment.ts` (`defaultRenderMode`, `resolveRenderMode`, `VALID_RENDER_MODES`). #1828 (link default `'preview'`, no `'card'` mode) fixed here.
> Previously: PARTIALLY BUILT (#1777 PR1 — binary model shipped; per-type enum not yet)
>
> **PR2 BUILT — NodeView host live (PR #1835)**
> `AttachmentNodeView.svelte` + `AttachmentCard.svelte` + `AttachmentToolbar.svelte` implemented.
> `addNodeView()` in `Attachment.ts` now mounts the Svelte NodeView instead of static HTML.
> Old `.attachment-*` CSS removed from `Editor.svelte`; NodeView owns its own styles.
> `feat/401-pr2-card-toolbar-nodeview`
>
> **PR7 BUILT — Link `embed` render mode live (PR #1896)** (ADR-0050)
> `AttachmentLinkEmbed.svelte` mounts the sole iframe for allowlisted providers (YouTube-nocookie, Vimeo). `resolveEmbed()` (in `src/lib/resolvers/`) reconstructs a canonical URL from a trusted template — non-allowlisted URLs fall back to the card. Sandbox is `allow-scripts allow-same-origin` (safe here because provider is cross-origin allowlisted — inverse of ADR-0049 D1 posture for local HTML). CSP adds `frame-src` allowlist (default-deny). Gated by `link_embeds_enabled` setting (default true, disclosed in Settings) per ADR-0050 §6.
> `feat/401-link-embed`

Render mode is stored as a per-embed TipTap node attribute (`data-render-mode`), not a DB column or a setting. `data-attachment-type` carries the attachment type. Default mode serializes to plain markdown; non-default to a raw-HTML block.

**Render mode is a per-type enum** (`data-render-mode`), valued from that type's mode set:
- image — `full-inline` (default) | `card`
- pdf — `card` (default) | `reader`
- audio — `card` (default) | `player`
- video — `card` (default) | `player`
- html — `card` (default) | `preview`
- link — `preview` (default) | `plain` | `embed`
- (unknown/other) — `card` (default), Open externally action

Link is the exception to "collapsed by default": its scannable default is the **preview card** (og: metadata), not a generic file card, and it has no `card` mode. Image is the exception in the other direction (expanded `full-inline` by default). All other types default to the collapsed `card`.

> **Amended 2026-06-30 (Session: ADR-0049 acceptance / #1779):** prior wording specified a binary `full-inline`/`collapsed-card` model with "all other types = collapsed card." That is wrong for Link (default `preview`; modes `plain`/`preview`/`embed`; no `card` mode) and cannot express the expanded modes (`reader`/`player`/`preview`/`embed`) of the other types. Corrected to a per-type enum matching the locked #1774 design.

### 2.2 Config File
> **Status:** BUILT

```json
{
  "version": 1,
  "ai": {
    "provider": "anthropic",        // "anthropic" | "openai" | "openclaw" | "custom" | "none"
    "model": "claude-sonnet-4-20250514",
    "apiKeyRef": "keychain:pane-ai-key", // reference to OS keychain, NOT the actual key
    "customEndpoint": null,
    "maxTokensBudget": 8000,
    "autoMemoryExtraction": true
  },
  "sync": {
    "enabled": false,
    "provider": "supabase",
    "lastSyncAt": null
  },
  "ui": {
    "theme": "tron-dark",
    "sidebarWidth": 260,
    "chatPanelWidth": 380
  },
  "notifications": {
    "enabled": true,
    "quietHoursStart": "23:00",
    "quietHoursEnd": "08:00"
  }
}
```

### 2.3 File Operations
> **Status:** BUILT

| Operation | File System | Database |
|-----------|------------|----------|
| Create note | Write `{slug}.md` to disk | INSERT into `notes` |
| Edit note | Overwrite `.md` file | UPDATE `updated_at`, `word_count`, `content_preview` |
| Move note | `fs::rename` to new path | UPDATE `project_id`, `folder_id`, `file_path` |
| Delete note (soft, Trash) | **File untouched** (#2874 item 1 ruling 1 — corrected from the original "move file to OS trash" design, which broke purge/restore; see §2A.2) | SET `deleted_at` (soft delete) |
| Delete note (Delete Permanently) | Move file to OS trash | DELETE row + cascades |
| Rename note | `fs::rename` the `.md` file | UPDATE `title`, `file_path` |
| Create folder | `fs::create_dir` | INSERT into `projects` with `parent_id` |
| Create project | `fs::create_dir` + `Assets/` | INSERT into `projects` |

### 2.3A Unified Atomic Write
> **Status:** BUILT (v0.65.71)
> **Created:** 2026-03-28
> **Issue:** #1298
> **Cross-references:** §22B.3 (Bridge File Sync), §22C.8 (Pane Gateway Sync Semantics)

All file writes in Pane go through a **single atomic write function**. No parallel write paths. No divergence.

**Location:** `src-tauri/src/file_ops.rs`

**WriteOrigin enum:**
```rust
pub enum WriteOrigin {
    Local,   // Pane-originated write — trigger_sync runs
    Remote,  // Ingest write from gateway — trigger_sync suppressed
}
```

**Signature:**
```rust
pub fn atomic_write(
    path: &Path,
    content: &str,
    db_conn: Option<&Connection>,
    origin: WriteOrigin,
) -> Result<(), String>
```

**What it does (in order):**
1. Write content to `{path}.tmp`
2. Read back and verify (byte-for-byte)
3. Suppress file watcher for this path
4. Atomic rename `.tmp` → target
5. If `db_conn` is provided AND path is a system file (`is_system_path()`) AND `origin == WriteOrigin::Local`: call `trigger_sync()` to push to gateway
6. Log: `[WRITE-GUARD] Atomic write: {path} ({bytes} bytes)`

**Rules:**
- **Every** file write — note content, daily logs, synthesis output, flat-file write-back, identity files — MUST go through this function.
- No module may implement its own atomic write helper. If you need atomic file I/O, call `file_ops::atomic_write()`.
- The sync trigger is part of the write path, not bolted on after the fact. The compiler enforces explicit origin declaration at every call site — sync can never be accidentally skipped.
- `db_conn` is `Option` because some writes happen before DB is available (e.g., bootstrap). Pass `None` to skip sync; pass `Some(&conn)` for normal operation.
- **Ingest writes** (pull_pending_changes, initial_sync, any future ingest/recovery flows) MUST pass `WriteOrigin::Remote`. This suppresses trigger_sync for remote-originated content.
- **Local user/AI writes** MUST pass `WriteOrigin::Local`. This ensures the content is queued for push to the gateway.
- `WriteOrigin` has no `Default` impl and no default parameter. Every caller must make an explicit choice. This is intentional — the compiler catches missing declarations.

**Why WriteOrigin suppresses trigger_sync for Remote:**
Without this, every ingest write immediately re-queues the ingested file for push back to the gateway (ingest → trigger_sync → queue → push → ingest cycle). 224 files pulled = 224 re-push attempts. `WriteOrigin::Remote` breaks the ping-pong loop without removing sync from the write path. After the ingest loop, `flush_sync_queue` is the sole mechanism for canonical write-back — it pushes any locally-modified files without re-queuing what was just received. See §22C.8 for architectural detail.

**Why:** Pane had two atomic write functions (`ai::tools::atomic_write()` and `database.rs::atomic_write_db()`) that diverged — one had the bridge sync trigger, the other didn't. Daily log writes went through the wrong one and never synced. A unified function eliminates this class of bug permanently.

---

### 2.3B Unified Settings Access
> **Status:** BUILT (v0.17.33)
> **Created:** 2026-03-28
> **Issue:** #281

All `ai_settings` reads MUST go through `database::get_ai_setting()`. All writes through `database::set_ai_setting()`.

**Rules:**
- No module may query `ai_settings` directly via raw SQL
- `get_ai_setting()` normalizes JSON-quoted values automatically
- `set_ai_setting()` stores plain values (no JSON wrapping)
- This prevents the recurring quote-mismatch bug class

```rust
// ✅ Correct
let provider = database::get_ai_setting(conn, "provider");

// ❌ Never do this
conn.query_row("SELECT value FROM ai_settings WHERE key = 'provider'", [], |row| row.get(0))
```

**Why:** `ai_settings` values were stored inconsistently — some with JSON quotes (`"\"openclaw\""`), some without (`openclaw`). Every reader implemented its own quote handling (or forgot to). This caused bugs THREE times: bridge URL trimming, bridge token normalization, and provider check in `get_gateway_url()`. The unified accessor centralizes normalization and eliminates this bug class permanently.

### §2.3C Unified Settings Persistence
> **Status:** BUILT (v0.17.38)
> **Created:** 2026-03-29
> **Issue:** #290

config.json is the single source of truth for all settings. The ai_settings DB table is a runtime cache.

**Write path:** `set_ai_setting()` → DB write → `flush_ai_to_config()` (updates ai section of config.json, preserves all other sections). **Credential redaction (ADR-0044): `flush_ai_to_config()` and every other config.json writer MUST NOT write any AI credential value to config.json.** The credentials (primary provider key, OC gateway token, Smart Features internal key) are keychain-resident per ADR-0044; config.json carries only non-secret configuration.

**Recovery path:** On DB creation, if ai_settings empty → `restore_ai_settings_from_config()` reads config.json ai section → populates ai_settings table with non-secret config only. Credentials are NOT recoverable from config.json (they are not written there) — they are read from the keychain. If the keychain lacks a credential after recovery, that credential is "not configured" and is re-entered via Settings (ADR-0044 Decision 5).

**Key mapping:**
| config.json (ai section) | ai_settings key | Notes |
|---|---|---|
| provider | provider | |
| model | model | |
| apiKeyRef | api_key | **Sentinel only (ADR-0044).** Holds a non-secret marker (e.g. `"keychain"`) indicating the credential lives in the OS keychain (account `ai_provider_key` / `ai_gateway_token` per provider). NEVER holds the credential value. The field name denotes a reference, and post-ADR-0044 the content matches the name. |
| customEndpoint | endpoint | |

**aiSettings dump blocklist (ADR-0044, normative).** Any code path that serializes `ai_settings` into config.json MUST apply a blocklist excluding the credential keys (`api_key`, `internal_api_key`, and any future credential key). The blocklist is the mechanical guard against a future field re-introducing a plaintext mirror. Adding a credential-bearing `ai_settings` key requires adding it to this blocklist in the same change.

Same principle as §2.3A (flat files for notes) and §2.3B (unified settings access).

**Why:** The previous implementation wrote flat ai_settings keys to config.json root, destroying the nested structure (theme, sync, UI). Recovery read from top-level keys but config.json has nested `ai.*` structure. This caused settings loss after DB deletion.

---

## 2A. File Lifecycle & Deletion Architecture

> **Status:** NOT BUILT — spec only

This section codifies how Pane handles the relationship between flat files on disk and database entries throughout the complete lifecycle, with particular focus on deletion, file watching, and integrity checking.

### 2A.1 Core Principle: Dual Storage Model (DB-Primary for User Notes, File-Only for System Notes)

> **Status:** BUILT (#486, v0.59.0) — dual storage implemented; watcher load_extension fix PR #1154 — same #1131 bug class (strip .dylib + explicit entry point)

**Updated in #486 (v0.59.0):** Pane uses two storage strategies depending on note type:

**User notes** (Projects/ folder):
- DB `body` column is runtime source of truth (fast, reliable, no file I/O on read)
- Flat files written on every save (portable backup, dual-write)
- FTS5 indexes body column via external content table (automatic triggers)
- Read path: body → file → content_preview (3-tier fallback)
- Editor save-on-destroy: `onUpdate` caches `lastMd` immediately (not just `lastJson`) so `onDestroy` fallback always has current content (#1735). `noteService.selectNote` awaits the in-flight save promise instead of a fixed 100ms heuristic.
- Integrity check: if body is NULL, populate from file; if file missing, regenerate from body

**System notes** (SOUL.md, MEMORY.md, RULES.md, logs/):
- Flat files are sole source of truth
- DB body column is NULL (intentional)
- Read/write paths use file I/O directly
- FTS5 manually updated for these notes (no auto-trigger for NULL body)
- Integrity check: if file missing, flag as `file_missing` (cannot regenerate)

**Why the split:** 
- System notes must survive Pane uninstall and sync via bridge plugin (file-based contract)
- User notes benefit from DB performance, reliability, and upcoming features (version history, collaboration)
- This preserves both portability (files) and performance (DB)

**This section extends that principle to deletion and reconciliation:**

- When a note is deleted, the DB entry is marked `deleted_at` (soft delete, preserves body for 30 days); the file is left exactly where it is until Delete Permanently, which is what actually moves it to the OS trash (#2874 item 1, §2A.2)
- When a file goes missing: user notes self-heal from body, system notes flagged
- During startup integrity check: bidirectional sync (files → DB, DB → files)
- File watcher must distinguish between user-initiated and app-initiated changes to prevent resurrection loops

### 2A.2 Delete Operation: Tombstone-Only Soft Delete; Trash Happens Only on Permanent Delete

> **Status:** BUILT (v1.0.61, #2874 item 1) — corrected from this section's original "file first, then
> tombstone" design below (never built as originally specified; the live pre-fix code trashed the file
> on soft delete too, which is the bug this item fixes, not a drift from this doc). Soft delete
> (`delete_note`/`force_delete_note`, `notes.rs`) is now DB-only: it never touches the filesystem.
> Only `purge_note` (Delete Permanently, `notes.rs:purge_note_internal`) moves the file to the OS
> trash, and tolerates the file already being gone. `restore_note` rebuilds a missing file from its
> latest `note_versions` snapshot when one exists, or returns an explicit `content_warning` saying the
> content was lost — never a silent, normal-looking restore. See `docs/audits/
> 2026-10-16-scope-1060-test-failures.md` item 1 for the full before/after diagnosis.
>
> **#3037 follow-up (`~/Shared/ExampleApp/docs/audits/2026-10-16-pr-3037-trash-keeps-file.md`):** Reed's
> audit of the first attempt found this item's own soft-delete half correct, but `execute_delete_note`
> (the AI assistant's `delete_note` tool, `ai::tools`) was a SEPARATE reimplementation that still
> trashed the file on soft delete, with its own narrower hand-typed system-note list (missing
> `identity`/`user`/`tools` — meaning the AI assistant could trash IDENTITY.md/USER.md/TOOLS.md on
> request). Fixed by extracting one shared `soft_delete_note_internal` (`notes.rs`) that owns the
> system-note check (against the single canonical `SYSTEM_NOTE_TYPES` list, never a second hand-typed
> one) and the tombstone mutation; `delete_note`, `force_delete_note`, AND `execute_delete_note` all
> call it now — ruling 1 below holds for all three entry points, not two of three. The same audit also
> found `empty_trash` (bulk purge) was a separate, incomplete reimplementation of ruling 2/3 below —
> see the new item-2 ruling added just below ruling 5.

#### Matt's ruling (#2874 item 1)

1. **The file stays where it is while the note sits in Pane's own Trash.** Soft delete
   (`delete_note`, and `force_delete_note` — despite its name, also a soft delete) is a pure DB
   tombstone: `UPDATE notes SET deleted_at = ...`, full stop. It never calls `delete_note_file` or
   anything else that touches the filesystem.
2. **Delete Permanently sends it to the OS trash.** `purge_note`'s internal implementation
   (`purge_note_internal`) is the ONLY path that calls `delete_note_file` (→ `PaneFs::trash` →
   `trash::delete()`) for a note's flat file — and only at that point, not at soft-delete time.
3. **A trashed note whose file is already gone purges cleanly.** `purge_note_internal` calls
   `delete_note_file_tolerating_missing` (`database.rs`), which treats a confirmed-`NotFound`
   `FsPolicyError` (not a blanket "any failure" swallow — see `FsPolicyError::is_not_found()`,
   `filesystem_policy.rs`) as "already purged, nothing to do here," while still surfacing every other
   failure mode (permissions, a real policy violation) as a hard `Err`. This covers both a file
   deleted by hand outside Pane, and a note trashed before this fix shipped (whose file the old bug
   already OS-trashed at soft-delete time).
4. **The watcher must not re-import a trashed note's still-present file.** Its dedup "already
   imported" check (`watcher.rs::process_new_file`) used to read `... AND deleted_at IS NULL`,
   EXCLUDING trashed notes — harmless at base only because a trashed note's file was always already
   gone by the time this ran (the pre-fix soft delete trashed it immediately). Now that soft delete
   leaves the file in place, the fix drops the `deleted_at` filter entirely for this dedup purpose:
   "does any row (trashed or not) already own this `file_path`" is exactly "already imported, don't
   reimport" — a purged note's row no longer exists at all, so a genuinely-free path still correctly
   reports re-importable.
5. **No recovery tool.** `restore_note` clears `deleted_at` on EVERY call (the DB row is always
   restorable) and, if the file is missing, additionally tries to rebuild it from the most recent
   `note_versions` snapshot for that note (`maybe_save_manual_version`/sheet-snapshot writes — never
   on creation, so a note trashed without a single edit afterward has zero snapshots). The command's
   response carries an explicit `content_warning: Option<String>` field: present and naming either
   "rebuilt from a saved version from {timestamp}" or "the content was lost before this fix" — the
   frontend (`Trash.svelte`'s `handleRestore`) surfaces it via an alert, never a silent success.
6. **Empty Trash is the same operation as Delete Permanently, run per item (#3037 follow-up, item 2).**
   `empty_trash` used to be a separate, incomplete reimplementation: it called `delete_note_file`
   directly with a blanket `.ok()` — ignoring a failed move to the OS trash and erasing the note's row
   anyway (stray file, no DB row) — and it skipped the `note_versions`/sheet-table cleanup
   `purge_note_internal` already does. It now calls `purge_note_internal` once per trashed item,
   exactly what the single-item `purge_note` command does. **An item whose file can't be moved stays
   in Pane's Trash** — `purge_note_internal` returns `Err` BEFORE its own `DELETE FROM notes`, so that
   item's row and `deleted_at` tombstone are left completely untouched, retryable on the next Empty
   Trash or a manual Delete Permanently — and **the user is told how many**: `empty_trash` now returns
   `EmptyTrashResult { purged: usize, failed: usize }` instead of a bare count, and `Trash.svelte`'s
   `handleEmptyTrash` surfaces a non-zero `failed` via an alert (the same pattern `handleRestore`'s
   `content_warning` already established on this screen), rather than claiming it emptied everything
   when it didn't.

> Status: `test_empty_trash_leaves_unmovable_item_in_trash_and_reports_the_count` and its sibling `#[ignore]`d (PR #3092) — Finder AppleEvent seam, pending DeleteMethod follow-up #3093.

#### Sync-Propagated Delete (unchanged by item 1)

**Sync delete** (tombstone received from another device via CRR) remains silent removal, distinct
from the user-delete path above:
1. **Silent removal** — `fs::remove_file()` (NOT trash). The file is removed without going to the OS trash.
2. **Respect the existing tombstone** — the `deleted_at` timestamp already exists from the originating device.
3. **No trash on receiving device** — the user already made the deletion decision on Device A. Trashing on Device B would clutter their OS trash with files they never directly interacted with.

**Tombstone cleanup:**
- Hard-delete tombstones older than 30 days via background job (all devices).

**Rationale:**

- Tombstones enable proper multi-device sync. If we hard-deleted immediately, another device wouldn't know the note was deleted—it would just look like a missing row.
- 30-day retention window allows for undo and gives sync time to propagate across offline devices.
- **User soft-delete → file stays put:** the user hasn't committed to losing the file yet — it's sitting in Pane's own Trash, restorable with its content intact. Trashing it early (the old, buggy design this section used to specify) is exactly what broke purge and restore.
- **User Delete Permanently → OS trash:** this IS the commitment — the file leaves Pane's tree for the OS's own recoverable trash.
- **Sync delete → silent removal:** The user decided on Device A. Trashing on Device B is noise — the file never existed in that device's active workspace from the user's perspective.

**Note (#486, v0.59.0):** User notes have full content in DB body column. Soft delete preserves this data — both the DB `body` column AND the on-disk file (item 1) stay intact while trashed; `purge_note` is what finally moves the file to the OS trash and deletes the row.

**Implementation note (reflects the real code, not aspirational):**

```rust
// Soft delete (delete_note / force_delete_note, notes.rs) — DB-only, no filesystem touch at all.
fn delete_note(note_id: &str, conn: &Connection) -> Result<(), String> {
    conn.execute(
        "UPDATE notes SET deleted_at = ?1 WHERE id = ?2",
        params![now_rfc3339(), note_id],
    )?;
    Ok(())
}

// Delete Permanently (purge_note_internal, notes.rs) — the ONLY path that trashes the file,
// tolerating it already being gone. Called by BOTH purge_note (one item) and empty_trash
// (once per item, #3037 follow-up item 2 — empty_trash used to be a separate, incomplete copy).
fn purge_note_internal(conn: &Connection, note_id: &str) -> Result<(), String> {
    let file_path: String = /* SELECT file_path FROM notes WHERE id = ?1 */;
    if !file_path.is_empty() {
        crate::database::delete_note_file_tolerating_missing(&file_path)?; // Ok on "not found"
    }
    // cascade note_versions / subtasks / notifications / sheet tables, then DELETE FROM notes
    Ok(())
}

// Empty Trash (empty_trash, notes.rs) — calls purge_note_internal per item; an Err (the file
// genuinely couldn't be moved — not "already missing", which the wrapper above tolerates)
// leaves that item's row/tombstone untouched and increments `failed` instead of `purged`.
fn empty_trash(conn: &Connection) -> Result<EmptyTrashResult, String> {
    let ids: Vec<String> = /* SELECT id FROM notes WHERE deleted_at IS NOT NULL */;
    let (mut purged, mut failed) = (0, 0);
    for id in ids {
        match purge_note_internal(conn, &id) {
            Ok(()) => purged += 1,
            Err(_) => failed += 1, // row/tombstone left exactly as-is — Matt's ruling
        }
    }
    Ok(EmptyTrashResult { purged, failed })
}
```

### 2A.3 Suppression Set: Preventing File Watcher Resurrection

> **Status:** BUILT (v0.68.x) — `SUPPRESSED_PATHS` (`watcher.rs:20`), `suppress_path`/`is_suppressed`/`acknowledge_path`
> (`watcher.rs:32-50`) are implemented and in active use across all note file-write paths:
> `atomic_write` (`file_ops.rs:135`, covers create/save), `delete_note_file` (`database.rs:952`),
> and `rename_note_file` (`database.rs:965-1000`, wired in #2381 — covers title renames, folder/project
> moves via `relocate_note_file`, and all rollback-on-error rename paths). The section below documents
> the design this implementation follows; treat it as descriptive of shipped behavior, not aspirational.
>
> **#2874 item 1 update:** `delete_note_file` is no longer called from the soft-delete commands
> (`delete_note`/`force_delete_note` — §2A.2 ruling 1, DB-only tombstone now). **#3037 follow-up
> update (§2A.2 ruling 1 now also covers `execute_delete_note`, the AI assistant's `delete_note` tool;
> ruling 6 covers `empty_trash`):** the plain `delete_note_file` (not the tolerating wrapper) has ZERO
> callers left anywhere in the crate — `execute_delete_note` no longer calls it at all (soft delete is
> DB-only there too now), and `empty_trash` no longer calls it directly either. The only remaining
> note-lifecycle caller of the trash-the-file primitive is `purge_note_internal`, via the
> missing-file-tolerant `delete_note_file_tolerating_missing` wrapper (`database.rs`) — called both by
> the single-item `purge_note` command and, once per item, by `empty_trash`. Both are Delete
> Permanently paths; soft delete (all three of its entry points) never touches the filesystem.

**Problem:** When Pane creates, saves, or deletes a file, the OS file watcher sees the event and fires a notification. If Pane responds to its own actions, the result is:
- Deleted notes get resurrected (file watcher sees deletion, creates a new DB entry)
- User gets toast notifications for files the AI just created
- Infinite loops in edge cases

**Solution: Runtime suppression set**

The app maintains a `HashSet<PathBuf>` of paths for which events should be ignored:

```rust
struct FileWatcherState {
    suppression_set: Arc<Mutex<HashSet<PathBuf>>>,
}

impl FileWatcherState {
    /// Add path to suppression set BEFORE file operation
    fn suppress_path(&self, path: &Path) {
        self.suppression_set.lock().unwrap().insert(path.to_path_buf());
    }
    
    /// Remove path from suppression set AFTER acknowledging the event
    fn acknowledge_event(&self, path: &Path) {
        self.suppression_set.lock().unwrap().remove(path);
    }
    
    /// Check if an event should be ignored
    fn is_suppressed(&self, path: &Path) -> bool {
        self.suppression_set.lock().unwrap().contains(path)
    }
}
```

**File operation pattern:**

```rust
// BEFORE any file write, delete, or rename:
file_watcher.suppress_path(&path);

// Perform the file operation
fs::write(&path, content)?;

// File watcher callback:
fn on_file_event(event: FileEvent) {
    if file_watcher.is_suppressed(&event.path) {
        // This is our own action, acknowledge and skip
        file_watcher.acknowledge_event(&event.path);
        return;
    }
    
    // External change detected — sync to DB
    handle_external_file_change(&event);
}
```

**Clearing mechanism: Event-based, NOT time-based**

- The path stays in the suppression set until the file watcher acknowledges the corresponding event.
- NO TTL (time-to-live) or expiration. Event-driven clearing is reliable across slow filesystems and high-load scenarios.
- Edge case: If the file watcher never fires (filesystem unmounted, watcher crashed), the suppression entry persists. This is safe—it just means that specific path won't trigger auto-sync until the app restarts. The alternative (TTL) risks removing suppression too early and causing resurrection bugs.

**What this prevents:**

- ✅ Deleted notes stay deleted (file watcher sees deletion, path is suppressed, no DB resurrection)
- ✅ No toast spam for AI-created files (create_note suppresses the path, watcher ignores the event)
- ✅ Saves don't trigger spurious "file modified externally" warnings
- ✅ Resilient to filesystem delays (event acknowledgment, not timing, controls suppression)

### 2A.3a Directory Exclusions: Watcher Skip Rules

> **Status:** BUILT (v0.67.67, PR #1823; depth-aware Assets fix v0.68.28, PR #2080)

The file watcher's `should_skip` function excludes entire directory subtrees from import processing. Paths matching any of these rules are never treated as importable notes:

- **Hidden files/directories** — any component starting with `.` (e.g. `.pane/`, `.git/`)
- **`Assets/` directories** — the per-project attachment store (`Projects/{Project}/Assets/`). Files written here by `save_attachment` or `copy_attachment_from_path` are attachments, not notes. Without this exclusion, dropping a `.md` or `.txt` file as an attachment triggers double ingestion: the file is saved as an attachment AND imported as a new note by the watcher after its 2-second debounce. (#1814). The exclusion is **depth-aware**: it skips the per-project `Assets/` subdirectory but allows a top-level project whose own name is "Assets" — a project at `Projects/Assets/` is a valid project, not an attachment store. (#1824)

Hidden-file exclusions are checked at the path-component level and apply regardless of nesting depth. The `Assets/` exclusion is position-sensitive: it applies only to the attachment subdirectory within a project, not to a project name.

### 2A.4 Missing Files: Self-Healing for User Notes, Flag for System Notes

> **Status:** PARTIALLY BUILT (#486 v0.59.0) — user note self-healing implemented, system note flagging spec only

**Problem:** During startup integrity check, if a DB entry exists but the corresponding `.md` file is not found on disk, behavior depends on note type.

**User notes (DB-primary, post-#486):**

- **NOT a critical error** — DB body column has full content
- File is backup — can be regenerated from body
- Integrity check recreates missing file from body automatically
- No `file_missing` flag needed (self-healing)
- Log: "Regenerated backup file from DB: {file_path}"

**System notes (file-only):**

- **CRITICAL** — file is sole source, cannot recreate (body is NULL)
- Set `file_missing = 1` flag
- Surface to user (may be disconnected storage, unmounted drive, sync conflict)
- Log: "WARNING: System note file missing: {file_path}"

**Why the distinction:** User notes survive file loss (DB has content). System notes don't (file-only).

**Why auto-deletion is still wrong for system notes:**

- The file may be on disconnected network storage (temporarily offline)
- The file may be on an unmounted external drive
- The user may have moved the system folder manually
- Sync conflict resolution may have temporarily removed the file

**Solution: Flag as `file_missing`**

```sql
ALTER TABLE notes ADD COLUMN file_missing INTEGER NOT NULL DEFAULT 0;
CREATE INDEX idx_notes_file_missing ON notes(file_missing) WHERE file_missing = 1;
```

**Integrity check behavior:**

```rust
async fn integrity_check_notes() -> Result<()> {
    for note in db.get_all_notes()? {
        if !Path::new(&note.file_path).exists() {
            // File is missing. Do NOT delete the DB entry.
            db.execute(
                "UPDATE notes SET file_missing = 1 WHERE id = ?",
                params![note.id]
            )?;
            
            log::warn!("Note {} file missing: {}", note.id, note.file_path);
        } else if note.file_missing == 1 {
            // File was missing but is now back. Clear the flag.
            db.execute(
                "UPDATE notes SET file_missing = 0 WHERE id = ?",
                params![note.id]
            )?;
        }
    }
    Ok(())
}
```

**UI treatment:**

- Notes with `file_missing = 1` show a subtle indicator in the UI (yellow caution icon, or strikethrough title)
- Hover tooltip: "File not found — may be on disconnected storage"
- User can:
  - Reconnect the storage (external drive, network share) → integrity check clears the flag
  - Manually delete the note → goes through normal deletion flow (tombstone created)
  - Ignore it → note stays flagged indefinitely, doesn't interfere with anything

**Rationale:**

Pane cannot distinguish between "file legitimately deleted externally" and "file temporarily unavailable." The conservative choice is to preserve the DB entry and let the user decide. This is the same philosophy as Git's handling of missing files in the working tree—flag the status, don't auto-delete tracked content.

### 2A.5 Startup Reconciler (Integrity Check): Bidirectional Sync

> **Status:** NOT BUILT — spec only

**Existing behavior (from §2.1 and §5.3):**

- File exists, no DB entry → create DB entry (indexing external changes)

**Extended behavior (this section adds):**

```
Startup integrity check runs on every app launch:

FOR EACH file in ~/Pane/Projects/**/*.md:
    IF no DB entry exists:
        → CREATE DB entry (existing behavior)
        → Log: "Indexed new file: {path}"

FOR EACH DB entry in notes table WHERE deleted_at IS NULL:
    
    -- Check body column population (#486: user notes only)
    IF note_type NOT IN ('soul', 'memory', 'rules', 'daily', 'identity', 'user', 'tools', 'agents', 'heartbeat'):
        -- This is a user note
        IF body IS NULL:
            IF file EXISTS:
                → READ file content
                → UPDATE notes SET body = content WHERE id = note_id
                → Log: "Integrity check: populated body from file: {path}"
            ELSE:
                → SET file_missing = 1
                → Log: "WARNING: User note missing both body and file: {path}"
        ELSE IF file does NOT exist:
            -- Body exists but file missing → regenerate file from body
            → WRITE file from body column
            → Log: "Regenerated backup file from DB: {path}"
    ELSE:
        -- This is a system note (file-only)
        IF file does NOT exist:
            → SET file_missing = 1 (CRITICAL: file is sole source)
            → Log: "WARNING: System note file missing: {path}"
        ELSE IF file_missing = 1:
            -- File was missing but is now back
            → CLEAR file_missing flag
            → Log: "System note file restored: {path}"
    
    IF file EXISTS AND deleted_at IS NOT NULL:
        → File was recreated externally after deletion
        → CLEAR deleted_at (resurrect the note)
        → Log: "Note resurrected: {title}"
        → This handles the edge case where sync deleted a note but 
          the user manually restored it from backup

FOR EACH tombstone WHERE deleted_at < (now - 30 days):
    → HARD DELETE from DB
    → Log: "Purged old tombstone: {id}"
```

**Rationale for bidirectional sync:**

- **File → DB:** External tools (text editors, sync clients, scripts) can add files. Pane must index them.
- **DB → File regeneration (#486):** User notes can self-heal from body column if file goes missing.
- **Body integrity check (#486):** Migration safety net — ensures all user notes have body populated from file if NULL.
- **System note file flag:** Missing system note files flagged (critical — file is sole source).
- **Resurrection logic:** If a deleted note's file reappears (user restored from Time Machine, sync reconverged), Pane should recognize it as intentional and clear the tombstone.
- **Tombstone cleanup:** Prevents the DB from accumulating deleted entries forever. 30 days is generous for sync propagation and user undo.

**Performance considerations:**

- This check runs on startup, not continuously.
- For large workspaces (1000+ notes), this may take 1-2 seconds. Acceptable for startup.
- Future optimization: Track last integrity check timestamp, only scan files modified since then.

### 2A.6 Folder Lifecycle: Same Pattern, Non-Empty Deletion Moves Content

> **Status:** BUILT (v1.0.60, #2874 item 65)

**Folders (sub-projects) follow the same lifecycle as notes:**

1. **Created in DB → directory created on disk** (§2.3 file operations)
2. **Deleted in DB → directory removed on disk** — non-recursively, and
   only once it's actually empty (every tracked item moved out, every
   untracked leftover trashed — see below)
3. **Integrity check creates missing directories** for DB folders (DB → disk)

**Special case: Non-empty folder deletion (superseded — see history below)**

This section originally specified refusing to delete a non-empty folder
(`PaneError::FolderNotEmpty`, "delete or move the notes first"). **Matt's
ruling (#2874 item 65, 2026-10-12) replaces that with: deleting a folder
moves its content, it never refuses and never silently orphans it.**

**What actually happens on delete (`delete_folder_internal`, `projects.rs`):**

1. Every note AND sheet row currently filed under the folder — **including
   items already sitting in Pane's own trash** (`deleted_at IS NOT NULL`,
   so restoring one afterward still lands correctly) — is moved into the
   parent project's **default folder** (`default_folder_id`, §1.7.1, found
   by ID, never by matching a name) through the exact same
   `move_note_internal` path every other note move in the app uses. There
   is no separate/parallel move implementation for this case.
2. **Filename clashes are renamed, never refused, never overwritten.** If
   the default folder already has a file with the same name, the moved
   file lands at `name (2).ext`, `name (3).ext`, etc. — the first free
   variant. The pre-existing file at the original name is never touched,
   inspected, or truncated. (Sheets have no on-disk file at all —
   `file_path = ''` — so this only applies to real note files.)
3. **All-or-nothing.** Every DB write (each note's move, an `ai_settings`
   collapse-state cleanup, the folder row delete) happens inside ONE SQL
   transaction, committed only once everything above succeeds. If ANY
   note's move fails — case: Pane's dual-storage model means the file and
   the DB row can independently desync, so a move can fail on the
   filesystem half even though the DB row looked fine — every file already
   relocated earlier in the same delete is renamed straight back to its
   original path, and the transaction is dropped uncommitted (automatic
   rollback). The folder is left exactly as it was: not deleted, contents
   not moved, nothing lost.
4. **Nothing Pane didn't move is ever permanently deleted (Reed WARN
   closure, #2874 item 65, 2026-10-12).** By the time the directory is
   touched, every tracked note/sheet has already been durably relocated —
   but the on-disk directory can still hold something Pane never put a
   `notes` row for (a file dropped in through Finder: a PDF, an image,
   anything untracked). So, in order:
   - Every remaining entry in the directory is handed to the OS trash via
     the existing `PaneFs::trash` path (the same call
     `delete_note_file` already uses — no parallel trash implementation).
     Recoverable, never a raw `remove_file`/`remove_dir_all`.
   - Only once the directory is actually empty is it removed, and only
     **non-recursively** (`remove_dir`, never `remove_dir_all`) — if it
     still isn't empty (a leftover's trash call failed), removal is
     skipped rather than forced.
   - Best-effort throughout: a failure trashing a leftover, or removing
     the directory, is logged and leaves the leftover/directory in place.
     It never fails the overall delete and never rolls back the moves —
     those are already committed by this point.
5. **The default folder itself can never be deleted** (§1.7.1 ruling 4,
   unchanged by this item) — refused before any of the above runs.
6. **Folders never nest.** `create_folder_internal` refuses any parent that
   is not a top-level project, so "the folder a deleted folder's contents
   move into" is always itself a direct child of the project, never
   another folder.
7. **The delete confirmation UI reads a live preview** (`get_folder_delete_preview`,
   note/sheet counts + the default folder's current name, by ID) at display
   time — e.g. "3 notes and 2 sheets will move to Overview" — never a
   hardcoded "General", never cached from an earlier render.
8. **§34 events (data-refresh.md):** one `moved` (`entity_type: "note"` or
   `"sheet"`) per item actually relocated, plus exactly one `deleted`
   (`entity_type: "folder"`) for the folder itself — using §34's existing
   constants, no new entity/reason invented.

**Integrity check for folders (unchanged):**

```
FOR EACH DB folder (project with parent_id NOT NULL):
    IF directory does NOT exist:
        → CREATE directory (DB → disk sync)
        → This handles the case where user deleted a folder directory 
          externally but DB entry remains

FOR EACH directory in ~/Pane/Projects/*/*/ (nested dirs):
    IF no DB folder entry exists:
        → CREATE DB entry (index external folder creation)
```

**Rationale:**

- Folders are structural containers, not content. The content inside them
  (notes, sheets) is what must never be lost — moving it to the default
  folder preserves it in full (DB row AND disk file, including trashed
  items) while still letting the folder itself go away, which is what the
  user actually asked for. Refusing the delete (the original design)
  forced the user to manually empty a folder first, for no safety benefit
  the move doesn't already provide outright.
- This is deliberately UNLIKE `rmdir` in Unix (which refuses on non-empty)
  — the content has somewhere well-defined and discoverable to go, so
  refusal would just be friction, not protection.

### 2A.6a Project (Top-Level) Deletion: A Trash Failure Stops the Delete

> **Status:** BUILT (#3027)

**Top-level projects (`parent_id IS NULL`) are not folders — deleting one
removes the entire directory, not just an empty shell once its contents
have been relocated.** That directory can hold everything the user has
ever put in the project: every note's flat file, sheets' assets, anything
dropped in through Finder. §2A.9's "Disk full during delete" row already
specified the right behavior for this in general terms (`trash::delete`
fails → error surfaced, DB entry not touched); `delete_project_internal`
(`projects.rs`) did not follow it — this section makes the two match.

**What used to happen (the bug, fixed by #3027):**

```rust
if let Err(e) = fs_policy.trash(PaneRoot::Projects, &project_relative, ExistingKind::Directory) {
    eprintln!("[PROJECTS] trash failed ({}), falling back to remove_dir_all", e);
    fs_policy.remove_dir_all(PaneRoot::Projects, &project_relative)
        .map_err(|e2| format!("Failed to delete project directory: {e2}"))?;
}
```

Any reason `trash()` could fail — OS trash unavailable, a permissions
error, anything — fell through to `remove_dir_all`: a permanent, recursive
delete with no OS trash involvement at all. Every note the user ever wrote
in that project was gone for good, with no error distinguishing "trashed"
from "destroyed" to the person who clicked delete.

**Matt's ruling, implemented exactly:**

1. **A trash failure stops the delete.** Nothing is removed from disk or
   from the database. The user sees an error naming Trash specifically
   (`"Could not move project '{name}' to Trash (...). The project was not
   deleted."`) — never a generic "failed to delete" that doesn't say what
   actually went wrong. **No `remove_dir_all` fallback exists anywhere on
   this path.**
2. **Disk and DB never diverge.** Every DB write for the delete (the
   `attachments` cleanup, the `projects` row delete — CASCADE handles
   notes/folders) happens inside one SQL transaction that stays
   **uncommitted** while the directory trash is attempted:
   - **Trash fails:** the function returns `Err` before `commit()` is ever
     reached. The transaction drops uncommitted, and rusqlite rolls back
     every DB write automatically (the same "drop = rollback" mechanism
     §2A.6's `delete_folder_internal` already relies on). Disk is
     untouched — `trash()` itself never completed. Nothing happened on
     either side.
   - **Trash succeeds:** the transaction commits. Normal case.
   - **Trash succeeds, but `commit()` itself then fails** (SQLite I/O
     error — no trash-crate "undo" exists to call instead): the one
     residual risk this ordering cannot remove, logged as CRITICAL. This
     ordering was chosen specifically because it is the only one under
     which a trash failure can satisfy point 1 above — committing the DB
     row's deletion before attempting trash would mean an `Err` from a
     failed trash still left the row gone.
3. **Legacy attachment cleanup** (`.pane/attachments/{project_id}/`,
   pre-v67) now also goes through `trash()` instead of a raw
   `remove_dir_all` — it used to permanently delete that directory with no
   OS trash step at all. Still best-effort/non-fatal (logged only) and
   runs after the main commit — it is a separate, already-authorized
   legacy location, not what ruling 1 above governs.

**Sweep (#3027 ruling 3):** the same destructive-fallback shape was found
and fixed in `database.rs::delete_note_file` (used by every note
delete/purge path) and the same no-trash-at-all permanent delete was found
and fixed in `attachments.rs`'s `delete_attachment` command. Internal/
system paths Pane owns itself (sync rollback cleanup of directories it had
just created, DB backup rotation, legacy `.pane/tasks/` migration cleanup,
confirmed-empty orphan directory removal) were reviewed and are not the
same bug — see the #3027 PR body for the full table.

### 2A.6b System Filenames Are Fixed By Role, Never Derived From Title

> **Status:** BUILT (#3029, extended #2874)

**System-managed notes** (`note_type` ∈ `soul`, `memory`, `rules`,
`identity`, `user`, `tools`, `daily` — the canonical list is
`SYSTEM_NOTE_TYPES`, `notes.rs`) have a filename determined entirely by
their **role**: `SOUL.md`, `MEMORY.md`, a given day's
`logs/{date}.md`, etc. That filename is never computed from the note's
`title`, and no creating path may ever introduce a `-N` suffixed variant
of one — a system note's canonical file is a fixed slot, not a
slug-collision-resolved name like a user note under `Projects/`.

**The bug this closes (#3029):** `update_note_internal`'s title-rename
branch (`notes.rs`) had no `note_type` guard, even though the frontend
already renders the title field readonly for these types
(`Editor.svelte`'s `SYSTEM_NOTE_TYPES`/`isReadOnly`). A title-changing
`update_note` call that reached a daily-log row renamed its file via a
slug derived from the new title — and because `generate_unique_slug`
(`notes.rs`) did not exclude the note's own current filename from its
collision check, a title edit that slugified to the same string as
today's filename self-collided, bumped to a `-1` suffix, and orphaned the
day's real content. The next "find or create today's log" lookup, keyed
on the now-stale `title`, missed the renamed row entirely and created a
fresh, empty file at the canonical path — two files on disk for one day.

**Matt's ruling, implemented exactly:**

1. **Title edits on system-managed note types are rejected server-side,
   with a clear error — not a silent no-op.** `update_note_internal`
   checks `current.note_type` against `SYSTEM_NOTE_TYPES` before any
   rename logic runs; a match returns `Err` immediately, before the file
   or DB row is touched.
2. **`generate_unique_slug` (`notes.rs`) excludes the note's own current
   filename from its collision check.** A candidate slug that lands on
   the note's existing file is never treated as a clash against itself.
3. **The daily-log "today" lookup (`ai::chat`) resolves by the
   immutable per-date filename encoded in `file_path`, not by the
   editable `title` column** — the lookup no longer trusts a field
   nothing was stopping anyone from changing.
4. **`cleanup_pane_system_duplicate_files` and all three callers were
   removed** (`sync/commands.rs`'s post-sync-pull call, the
   `cleanup_duplicate_files` Tauri command, `Onboarding.svelte`'s
   `invoke('cleanup_duplicate_files')`) — it had no DB correlation at all
   (pure filesystem `-N` scan) and could delete whichever file the
   separate DB-level dedup (`dedup_pane_system`/`dedup_folders_and_notes`,
   unaffected by this PR — tracked separately as #3030) had just decided
   to keep. Existing `-N` files from before this fix are not migrated or
   cleaned up by this change — they persist, by Matt's explicit ruling.

**Canonical note-type list:** `SYSTEM_NOTE_TYPES` (`notes.rs`) is the one
Rust-side source of truth. The frontend mirror lives in
`src/lib/constants/systemNoteTypes.ts`, imported by `Editor.svelte` and
its tests rather than hand-copied — `systemNoteTypes.parity.test.ts`
extracts both arrays via regex over their literal source text and fails
if they ever drift, the same mechanism #3026/item-64 established for
`VALID_COLUMN_TYPES`/`ShHeaderMenu.svelte`'s `TYPES`.

**System-managed notes are also never MOVED — rejected server-side
(#2874).** A fixed, role-derived filename only matters if the note also
stays in a fixed *location*; this extends the title guard above to a
note's `project_id`/`folder_id` too. Matt's ruling: hard reject, the same
shape as the title guard — a clear error, checked before any mutation, so
the file and the DB row are both left completely untouched.

- **Checked at every user- or AI-initiated move entry point:** the
  `move_note` command (via `move_note_internal`'s `enforce_system_guard`
  flag), `update_note_internal`'s project/folder-move branch, and the AI
  move tool (`execute_move_note`, `ai::tools`). All three call the
  same single check, `reject_system_note_move` (`notes.rs`), against
  `SYSTEM_NOTE_TYPES` — one piece of logic, multiple call sites, not four
  hand-typed copies of the same condition.
- **Deliberately NOT a blanket, unconditional guard on
  `move_note_internal` itself**, even though that function is the one
  piece of code every one of those paths funnels through (directly, or
  via `relocate_note_file`) — `delete_folder_internal` (§2A.6, folder
  deletion's forced relocation to the default folder) also calls
  `move_note_internal`, and that relocation-to-safety must keep working
  even for a system note a pre-this-guard move already left sitting in a
  folder that's now being deleted. `move_note_internal` therefore takes
  an explicit `enforce_system_guard: bool` — `true` from the user-facing
  `move_note` command, `false` only from `delete_folder_internal`'s
  forced relocation.

### 2A.7 Write-Back Integrity: DB and File Must Stay in Sync

> **Status:** NOT BUILT — spec only (enforcement layer)

**Existing principle (§2.1, §2.3):** Operations are atomic—file operation and DB update must both succeed or both fail.

**This section adds explicit checks:**

```rust
async fn save_note(note_id: &str, new_content: &str) -> Result<()> {
    let note = db.get_note(note_id)?;
    
    // Determine note type (#486: system vs user notes)
    let note_type: Option<String> = conn.query_row(
        "SELECT note_type FROM notes WHERE id = ?", params![note_id], |row| row.get(0)
    ).optional()?;
    
    let is_system_note = note_type.as_deref()
        .map_or(false, |nt| matches!(nt, "soul" | "memory" | "rules" | "daily" | "identity" | "user" | "tools" | "agents" | "heartbeat"));
    
    // Suppress file watcher before write
    file_watcher.suppress_path(&note.file_path);
    
    if is_system_note {
        // System note: file-only write
        fs::write(&note.file_path, new_content)
            .map_err(|e| PaneError::FileWriteFailed { path: note.file_path.clone(), source: e })?;
        
        // Update metadata only (body stays NULL)
        db.execute(
            "UPDATE notes SET updated_at = datetime('now'), word_count = ?, content_preview = ? WHERE id = ?",
            params![count_words(new_content), preview(new_content), note_id]
        )?;
        
        // Verify file write
        let written_content = fs::read_to_string(&note.file_path)?;
        if written_content != new_content {
            log::error!("File write verification failed for system note {}", note_id);
        }
    } else {
        // User note: dual-write (DB body + file)
        
        // 1. Write to DB body (primary)
        db.execute(
            "UPDATE notes SET body = ?, updated_at = datetime('now'), word_count = ?, content_preview = ? WHERE id = ?",
            params![new_content, count_words(new_content), preview(new_content), note_id]
        )?;
        
        // 2. Write to file (backup)
        fs::write(&note.file_path, new_content)
            .map_err(|e| PaneError::FileWriteFailed { path: note.file_path.clone(), source: e })?;
        
        // 3. Verify BOTH writes
        let db_body: String = conn.query_row(
            "SELECT COALESCE(body, '') FROM notes WHERE id = ?",
            params![note_id], |row| row.get(0)
        )?;
        
        if db_body != new_content {
            log::error!("DB body write verification failed for note {}", note_id);
        }
        
        let written_file = fs::read_to_string(&note.file_path)?;
        if written_file != new_content {
            log::error!("File backup write verification failed for note {}", note_id);
        }
    }
    
    Ok(())
}
```

**Verification on critical operations:**

- After every file write, read the file back and verify content matches what was written
- If verification fails, retry once. If still fails, surface error to user and log for debugging.
- This catches:
  - Filesystem corruption
  - Partial writes (disk full, process killed mid-write)
  - Sync conflicts that overwrite files immediately after write

**Rationale:**

File I/O can fail in subtle ways. Verification catches issues before they propagate and corrupt the user's mental model of their data.

### 2A.8 Implementation Checklist

> **Status:** NOT BUILT — spec only

This section requires changes across multiple layers:

**Backend (Rust):**
- [ ] Add `file_missing` column to `notes` table (migration)
- [x] Implement suppression set in file watcher module — `SUPPRESSED_PATHS` (`watcher.rs:20-50`)
- [x] Update `delete_note` to suppress path before file deletion — `delete_note_file` (`database.rs:952`)
- [x] Update `create_note`, `save_note`, `rename_note` to suppress paths — `create_note`/`save_note` route
      through `atomic_write` (`file_ops.rs:135`); `rename_note_file` wired in #2381 (`database.rs:965-1000`)
- [ ] Update startup integrity check to:
  - [ ] Flag missing files (set `file_missing = 1`)
  - [ ] Detect resurrected files (clear `deleted_at`)
  - [ ] Hard-delete tombstones older than 30 days
- [ ] Implement folder non-empty check in `delete_folder`
- [ ] Add write-back verification to critical file operations

**Database:**
- [ ] Migration: `ALTER TABLE notes ADD COLUMN file_missing INTEGER NOT NULL DEFAULT 0`
- [ ] Index: `CREATE INDEX idx_notes_file_missing ON notes(file_missing) WHERE file_missing = 1`

**UI (Svelte):**
- [ ] Visual indicator for notes with `file_missing = 1` (yellow caution icon)
- [ ] Hover tooltip explaining the missing file status
- [ ] User action: "Locate File" or "Delete Note" options for missing files
- [ ] Folder deletion: error dialog when folder is non-empty, with list of contained notes

**Testing:**
- [ ] Unit test: Delete note → verify file in trash, DB tombstone created
- [ ] Unit test: File watcher suppression → delete note, verify no resurrection
- [ ] Integration test: Unmount external drive → verify notes flagged as missing, not deleted
- [ ] Integration test: Restore file from trash → verify note resurrected
- [ ] Integration test: Integrity check with mixed state (missing files, external files, tombstones)
- [ ] Regression test: High-frequency file operations don't trigger watcher loops

### 2A.9 Edge Cases and Failure Modes

> **Status:** NOT BUILT — spec only

| Scenario | Behavior | Rationale |
|----------|----------|-----------|
| **User deletes file in Finder, Pane is running** | File watcher detects deletion → sets `deleted_at` on DB entry (normal deletion flow) | External deletion is treated as intentional user action |
| **User deletes file in Finder, Pane is closed** | On next startup, integrity check sets `deleted_at` | Same as above, deferred until app launch |
| **Sync-propagated delete received** | Silent `fs::remove_file`, respect tombstone | User decided on Device A; trash on Device B is noise |
| **File watcher crashes or is disabled** | Suppression set persists until restart. File changes not auto-synced until restart. User can manually trigger "Refresh Notes" | Graceful degradation. App remains functional, just no live sync |
| **Disk full during delete** | `trash::delete` fails → error surfaced to user, DB entry NOT marked deleted | Atomic failure. Note remains intact. For top-level project deletion specifically, see §2A.6a (#3027) — this row's principle is now actually implemented there, not just specified. |
| **Tombstone retention exceeded, file restored after 30 days** | Integrity check treats it as a new file → creates new DB entry with new ID | Old note ID lost. Acceptable—30 days is generous. |
| **Sync conflict: Note deleted on Device A, edited on Device B** | CRR handles this. Tombstone from A propagates. Device B sees `deleted_at` but file exists locally → resurrection logic clears tombstone, edit wins | CRDT conflict resolution: edit beats delete |
| **File moved externally (not via Pane)** | Integrity check sees DB entry for old path (file missing) and new file (no DB entry). Two notes in DB. | User must manually consolidate. Alternative: hash-based deduplication (future) |

### 2A.10 Future Enhancements (Post-Launch)

> **Status:** NOT BUILT — spec only (future work)

This section establishes the foundation. These refinements can come later:

1. **Hash-based deduplication** — detect when a file is moved (not copied) by comparing content hashes
2. **Undo buffer for deletions** — keep last N deleted notes in an in-memory cache for instant undo
3. **Smart resurrection** — if a file is restored from backup with same content hash, link it to original DB entry (preserve ID, tags, metadata)
4. **Filesystem monitoring health check** — detect if file watcher is falling behind, surface warning
5. **Integrity check on-demand** — Settings button to manually trigger integrity check without restart
6. **Reconciliation log** — show user what the integrity check found and fixed (UI panel in Settings)

---

## 2B. Database Safety & Migration Architecture

> **Status:** NOT BUILT — spec only (permanent architectural section added Mar 25, 2026)

This section codifies mandatory safeguards for database migrations and schema changes to prevent data loss. Every rule here exists because it was violated once.

### 2B.1 Pre-Migration Backup (MANDATORY)

> **Status:** NOT BUILT

**Before ANY migration runs, the app MUST:**

1. **Check if DB exists and has data** — file size > 4096 bytes (empty DB is ~4KB)
2. **WAL checkpoint** — `PRAGMA wal_checkpoint(TRUNCATE)` to flush all pending writes from WAL to main DB file
3. **Copy database** — `pane.db` → `pane.db.backup-{YYYYMMDD_HHMMSS}`
4. **Verify backup** — confirm backup file size matches original within 1KB tolerance

**This runs automatically on every app launch, before migrations.**

**Backup rotation:**
- Keep last 5 backups
- Delete older backups automatically
- Backup location: same directory as `pane.db` (`~/.pane/`)
- **Sidecar cleanup:** Each backup produces sidecar files (`_system.tar.gz`, `_install-uuid.json`, `_keychain.json`, `_metadata.json`). Rotation MUST delete all sidecars alongside the `.db` file. Sidecar suffixes are defined in a single `SIDECAR_SUFFIXES` constant to prevent creation/deletion drift.

> Status: BUILT (v0.67.26, PR #1583) — sidecar cleanup via shared SIDECAR_SUFFIXES constant

**Migration backup-read authorization invariant (#2187 / #2278, hardened by #2717).** Any migration that enumerates or opens backup files MUST authorize the backup directory and each candidate through `PaneFs`. Candidate filenames are reconstructed as relative components under `PaneRoot::Internal` and authorized with `ExistingKind::File` before SQLite or another parser opens them. Matching names are not sufficient authorization: symlinks, non-regular files, traversal, and entries resolving outside the Pane backup root MUST be rejected. Production migration code must never directly open a raw `DirEntry::path()`.

**Why:**
- Migrations can fail mid-execution
- Schema bugs can corrupt data
- CRR migrations have known failure modes (see §2B.3)
- A backup taken AFTER flushing WAL is a complete snapshot

**Implementation:**

```rust
async fn pre_migration_backup(db_path: &Path) -> Result<PathBuf> {
    // Check file exists and has data
    let metadata = fs::metadata(db_path)?;
    if metadata.len() <= 4096 {
        return Err(PaneError::EmptyDatabase);
    }
    
    // Open DB and checkpoint WAL
    let conn = Connection::open(db_path)?;
    conn.execute("PRAGMA wal_checkpoint(TRUNCATE)", [])?;
    drop(conn); // Close connection before copy
    
    // Create timestamped backup
    let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
    let backup_path = db_path.with_file_name(format!("pane.db.backup-{}", timestamp));
    
    fs::copy(db_path, &backup_path)?;
    
    // Verify backup size matches original
    let backup_size = fs::metadata(&backup_path)?.len();
    let original_size = fs::metadata(db_path)?.len();
    
    if (backup_size as i64 - original_size as i64).abs() > 1024 {
        fs::remove_file(&backup_path)?;
        return Err(PaneError::BackupVerificationFailed);
    }
    
    // Rotate old backups (keep last 5)
    cleanup_old_backups(db_path.parent().unwrap())?;
    
    Ok(backup_path)
}
```

### 2B.2 Graceful Migration Failure

> **Status:** BUILT v0.15.0 (graceful fallthrough), clarified ADR-0017 (2026-04-30)

**A failed migration MUST NEVER cause data loss.**

**When a version mismatch is detected, an explicit per-version-pair migration is applied.** If no migration exists for the pair, the `SCHEMA_MISMATCH` error is logged and the app continues with the existing schema (the new feature, in that case, is unavailable until a migration ships). The `[CRITICAL]` log surfaces the gap loudly per ADR-0018's CI smoke check.

**When `run_migrations()` returns an error:**

1. **Log the error** — `eprintln!("[CRITICAL] Migration failed: {}", error)` with full details
2. **Continue with existing database schema** — the app runs with the old schema
3. **The app runs with missing features, not missing data** — newer features disabled, existing data intact
4. **The DB is NEVER recreated if it contains data** — no `DROP DATABASE`, no fresh init

**A missing feature (e.g., compaction table doesn't exist) is infinitely preferable to lost data.**

**This is the production behavior.** Per ADR-0017, this graceful-fallthrough model has been the implemented behavior since v0.15.0 and is now the documented standard. Never destroys data per the v0.15.0 incident response (§2B.5).

**Implementation:**

```rust
async fn init_db(db_path: &Path) -> Result<Connection> {
    // Pre-migration backup (§2B.1)
    if db_path.exists() {
        match pre_migration_backup(db_path).await {
            Ok(backup_path) => log::info!("Backup created: {:?}", backup_path),
            Err(e) => {
                eprintln!("[CRITICAL] Backup failed: {}", e);
                return Err(e); // Don't proceed without backup
            }
        }
    }
    
    let conn = Connection::open(db_path)?;
    
    // Run migrations
    match run_migrations(&conn) {
        Ok(_) => log::info!("Migrations completed successfully"),
        Err(e) => {
            eprintln!("[CRITICAL] Migration failed: {}", e);
            eprintln!("[CRITICAL] Continuing with existing schema");
            eprintln!("[CRITICAL] Some features may not work");
            // DO NOT RETURN ERROR — continue with old schema
        }
    }
    
    Ok(conn)
}
```

**What this prevents:**
- v0.15.0 incident (see §2B.5) where migration failure → `init_db()` error → app recreated fresh DB → all data lost
- Any future migration bugs causing total data loss
- User losing work because of a developer mistake

### 2B.3 Schema Management: Pre-Launch vs. Post-Launch

> **Status:** BUILT in v0.61.0 — CRR UNIQUE and FOREIGN KEY constraints removed (#486, #493), FTS5 columns fixed.
> **Updated:** 2026-04-18 — cr-sqlite rejects checked FK constraints on CRR tables; enforce referential integrity in application code.

#### Pre-Launch (now through v1.0 — zero customers)

> **Updated:** 2026-04-30 (ADR-0017, ADR-0019) — per-version-pair migrations required on schema bumps

**Pre-launch schema management uses a single canonical schema definition (`init_schema()`) as the post-migration target.** Schema bumps require explicit per-version-pair migration code in `run_migrations()` to bridge older DBs forward to the canonical target.

**Rules:**
1. **One canonical schema** — `init_schema()` is the single source of truth for what the schema *is* at the current `SCHEMA_VERSION`
2. **No permanent migration chain accumulating through history** — no migration files numbered `001_initial.sql`, `002_add_column.sql`, etc., piling up forever
3. **Per-version-pair migrations required when bumping `SCHEMA_VERSION`** — bumping from N to N+1 requires adding a match arm in `run_migrations()` that brings v(N)-stamped DBs to the v(N+1) state. Idempotent (`IF NOT EXISTS` for table creation, no-op on already-applied state). The arm lives in the same PR as the schema bump.
4. **Graceful fallthrough on missing migration** — if `run_migrations()` encounters a mismatch with no matching arm, it logs the SCHEMA_MISMATCH error and `init_database()` continues with the existing schema per §2B.2. The new feature is unavailable; data is preserved. ADR-0018's CI smoke check fails the build before merge if this path produces a `[CRITICAL]` log.
5. **CRR enabled after creation** — tables are created with all columns baked in, then `crsql_as_crr()` is called. No altering CRR tables post-CRR.
6. **No companion tables for core columns** — columns belong on their parent table. `body` on `notes`, `is_hidden` on `projects`, settings on `projects`. Companion tables are a workaround for CRR ALTER limitations that don't apply pre-launch.

**Why this model:**
- Pre-launch user base is small (single-digit beta testers) — per-pair migrations are tractable
- User notes are stored as markdown files — DB is metadata, not primary content
- Eliminates the failure class of multi-step migration chains accumulating through history
- Produces the cleanest possible schema for launch day
- Previous companion tables (`folder_preferences`, `project_settings`) didn't sync via CRR — a bug caused by the workaround itself
- Per ADR-0017's postmortem, the "delete and recreate" path was never built and contradicted §2B.2. Per-pair migrations are the production behavior.

**Enforcement:**
- Any schema bump = edit the canonical definition + add per-pair migration arm + add integration test
- **Version coverage test** (per ADR-0017) — asserts every version-pair branch exists for every step from lowest supported to SCHEMA_VERSION. Catches PR #690-shape failures at PR time.
- **No UNIQUE or FOREIGN KEY constraints on CRR tables** — cr-sqlite rejects both. Enforce in application code (#493)
- **Every NOT NULL column on CRR tables must have a DEFAULT value** — required for sync compatibility
- **Projects/folders are never deleted, only archived** — eliminates cascade scenarios
- Reed audit: "Does the canonical schema include every column the app needs? Does the migration arm exist for this version pair?"

#### Post-Launch (v1.0+ — real customers with real data)

> **Status:** Pending research — tracked in #492

Post-launch migration strategy requires Reed research into cr-sqlite's actual ALTER TABLE capabilities, version-specific behavior, and what other production cr-sqlite apps do. The pre-launch companion table approach had a sync bug (settings were silently device-local), so the post-launch guidance must be evidence-based, not assumed.

Key questions for #492:
- Has cr-sqlite improved ALTER TABLE support in newer versions?
- Is drop-rebuild safe post-launch within a transaction with proper sync state handling?
- What do other production apps using cr-sqlite actually do?
- Are there better local-first sync solutions that handle schema evolution?

**Until #492 is resolved, the only known-safe post-launch approach is:**
1. Non-CRR tables: normal `ALTER TABLE ADD COLUMN`
2. CRR tables: companion tables (with CRR enabled on the companion if sync is needed)
3. Never use `SELECT *` in migrations — explicit column lists only

### 2B.4 Migration Versioning

> **Status:** NOT BUILT

**Migrations are numbered sequentially:** `001_initial_schema.sql`, `002_add_tags.sql`, ...

**Tracked in `schema_migrations` table:**

```sql
CREATE TABLE IF NOT EXISTS schema_migrations (
    version INTEGER PRIMARY KEY,
    applied_at TEXT NOT NULL DEFAULT (datetime('now')),
    description TEXT
);
```

**Migration rules:**

1. **Migration record inserted AFTER successful execution** — pessimistic tracking. If migration fails, no record is created.
2. **Migration numbers cannot be reused** — once 007 is attempted (even if failed), 007 cannot be used again.
3. **Each migration must be idempotent where possible** — `CREATE TABLE IF NOT EXISTS`, `ALTER TABLE IF NOT EXISTS COLUMN` (SQLite 3.35+)
4. **Failed migrations are logged** — `INSERT INTO migration_failures (version, error, attempted_at)`

**Implementation pattern:**

```rust
fn run_migrations(conn: &Connection) -> Result<()> {
    let current_version: i32 = conn
        .query_row("SELECT COALESCE(MAX(version), 0) FROM schema_migrations", [], |row| row.get(0))
        .unwrap_or(0);
    
    for migration in MIGRATIONS.iter() {
        if migration.version <= current_version {
            continue; // Already applied
        }
        
        match conn.execute_batch(&migration.sql) {
            Ok(_) => {
                conn.execute(
                    "INSERT INTO schema_migrations (version, description) VALUES (?, ?)",
                    params![migration.version, migration.description]
                )?;
                log::info!("Applied migration {}: {}", migration.version, migration.description);
            }
            Err(e) => {
                eprintln!("[CRITICAL] Migration {} failed: {}", migration.version, e);
                
                // Log the failure
                let _ = conn.execute(
                    "INSERT INTO migration_failures (version, error, attempted_at) VALUES (?, ?, datetime('now'))",
                    params![migration.version, e.to_string()]
                );
                
                // Return error — caller decides whether to proceed or abort
                return Err(e.into());
            }
        }
    }
    
    Ok(())
}
```

### 2B.5 Incident Record

> **Status:** HISTORICAL

**v0.15.0 (Mar 25, 2026):**

Migration 007 attempted `ALTER TABLE` on CRR table `chat_messages_local` to add a `summary` column for compaction feature.

**Failure chain:**
1. `ALTER TABLE chat_messages_local ADD COLUMN summary TEXT NOT NULL` executed
2. cr-sqlite rejected the alteration (CRR tables cannot use `NOT NULL` on ALTER)
3. Migration failed → `run_migrations()` returned `Err`
4. `init_db()` propagated the error instead of continuing with old schema
5. App initialization failed
6. App recreated a fresh database (because `init_db()` returned error suggested missing DB)
7. **All DB-only data lost:** projects, chat history, extracted facts
8. **Flat-file data survived:** agent notes (SOUL.md, MEMORY.md), daily logs

**Root cause:**
- Known CRR constraint (§2B.3) not enforced in code review
- No pre-migration backup (§2B.1) to restore from
- Migration failure caused `init_db()` to fail, which triggered fresh DB creation

**Resolution:**
- §2B.1 (pre-migration backup) added — MANDATORY on every launch
- §2B.2 (graceful failure) added — failed migration → log + continue, never recreate
- §2B.3 (CRR rules) codified — separate tables preferred, `crsql_begin/commit_alter` required if unavoidable
- Migration 007 rewritten to use separate `compaction_summaries` table (non-CRR)
- Code review checklist updated

**This incident validates the local-first + flat-file architecture:** Agent identity, memory, and daily logs survived because they live in `.pane/system/` as markdown files. Only DB-indexed metadata was lost. The recovery path was: restore agent identity from flat files, rebuild project structure manually, chat history unrecoverable but memory intact.

### 2B.6 Database Initialization Pattern

> **Status:** BUILT v0.29.3 (fixed #454)

**The database initialization function was being called twice on startup, causing migrations to run twice and roll back all schema changes.**

**Issue:**
- `init_database()` was called at line 273 (main startup) — correct
- `init_database()` was called again at line 628 (startup sync task, 10s later) — incorrect
- Second call re-ran migrations, hit "database is locked" error, rolled back ALL migrations including the first run's work

**Root cause:**
`init_database()` combined two operations that should be separate:
1. Opening a database connection (with pragmas, CRR extension)
2. Running migrations (should only happen ONCE at app startup)

**Fix:**

Two separate functions:

```rust
/// Opens a database connection without running migrations.
/// Use this when you need a connection but migrations have already run.
/// This is safe to call multiple times (e.g., in background tasks).
pub fn open_connection(crsqlite_path: Option<PathBuf>) -> Result<Connection, String> {
    let db_path = get_db_path();
    
    if let Some(parent) = db_path.parent() {
        fs::create_dir_all(parent)?;
    }
    
    let conn = Connection::open(&db_path)?;
    conn.execute_batch("PRAGMA journal_mode=WAL;")?;
    
    if let Some(ref path) = crsqlite_path {
        load_crsqlite_extension(&conn, path)?;
    }
    
    Ok(conn)
}
```

> Status: crsqlite load_extension path + entry point + code signing fix (#1131, #1132, #1134)

```rust
/// Full database initialization — runs migrations on a separate short-lived
/// connection, opens the app connection after migrations complete, seeds defaults.
/// Call this ONCE at app startup. Use open_connection() for subsequent connections.
///
/// The two-connection structure is load-bearing (see ADR-0051):
/// SQLite caches prepared statements (including FTS5 trigger VDBE programs)
/// per-connection. If migrations bump schema_cookie on a connection that
/// already has cached programs, the next invocation of those programs fails
/// with "unable to reset cached trigger stmt." Running migrations on a
/// short-lived connection that drops before the app connection opens
/// eliminates this class of failure by construction — the app connection
/// starts with the final post-migration schema_cookie and caches nothing stale.
pub fn init_database(crsqlite_path: Option<PathBuf>) -> Result<Connection, String> {
    init_file_system()?;

    // Migration connection: short-lived, dropped before the app connection opens.
    // See ADR-0051. Do not extend this scope or hold the connection past migration.
    {
        let migration_conn = open_connection(crsqlite_path.clone())?;
        match run_migrations(&migration_conn) {
            Ok(_) => {},
            Err(e) => {
                eprintln!("[CRITICAL] Migration failed: {}", e);
                // Continue with existing schema per §2B.2
            }
        }
        // migration_conn dropped here at end of scope.
    }

    // App connection: opens after migrations complete, sees the final schema.
    let app_conn = open_connection(crsqlite_path)?;

    seed_defaults(&app_conn)?;
    restore_ai_settings_from_config(&app_conn)?;

    Ok(app_conn)
}
```

**Load-bearing scope structure.** The migration connection MUST be dropped before the app connection opens. This is enforced by Rust block scope; a future refactor that combines the two connections, holds `migration_conn` in a wider scope, or moves `app_conn`'s open inside the block would reintroduce the schema-cookie mismatch class of failure. See ADR-0051 for the full mechanism and rationale.

**`seed_defaults` and `restore_ai_settings_from_config` run on the app connection.** Both are idempotent post-migration setup operations. Running them on `app_conn` (not `migration_conn`) keeps `migration_conn`'s responsibility scoped to migrations only. Migration writes persist to the WAL file and are visible to `app_conn` when it opens; no explicit checkpoint is required between the two connections.

**Rule:** Never call `init_database()` from background tasks, sync operations, or any code that runs after startup. Use `open_connection()` instead.

**Implementation locations:**
- `src-tauri/src/database.rs` — `open_connection()` and `init_database()` functions
- `src-tauri/src/lib.rs` line 628 — changed from `init_database(None)` to `open_connection(None)` in startup sync task (v0.29.3, #454)
- Tests added to verify multiple `open_connection()` calls work correctly and migrations only run once
- **v0.67.94 → per ADR-0051:** `init_database()` restructured to run migrations on a separate short-lived connection, dropped before the app connection opens. Eliminates the "unable to reset cached trigger stmt" class of failure on upgrade paths. Regression test filed as follow-up.

---

## 2C. File Write Safety

> **Status:** NOT BUILT — spec only (permanent architectural section added Mar 25, 2026)

This section codifies mandatory safeguards for file write operations to `.pane/system/` to prevent data loss. Every rule here exists because it was violated once.

### 2C.1 Write Guard Principle (MANDATORY)

> **Status:** NOT BUILT

**All writes to `.pane/system/` MUST go through a write guard function.** No direct `std::fs::write()` to system files.

The guard function `safe_write_system_file()` exists but currently has zero callers (pane-file-write-audit.md documents 9 out of 13 write paths using blind `std::fs::write()`).

**Write guard behavior:**
1. **Check if file exists** — `Path::exists()`
2. **If file has content** — read existing content, check size > 0
3. **Preserve existing content** — never overwrite without reading first
4. **Log the write** — timestamp, file path, operation type (create|update|append), content size
5. **Atomic write** — temp file + rename pattern (§2C.7)

**Implementation:**

```rust
async fn safe_write_system_file(
    path: &Path,
    new_content: &str,
    operation: WriteOperation  // Create | Update | Append
) -> Result<()> {
    // 1. Check existence
    let file_exists = path.exists();
    
    // 2. If exists, read current content
    let existing_content = if file_exists {
        fs::read_to_string(path)?
    } else {
        String::new()
    };
    
    // 3. Guard against destructive overwrites
    if file_exists && !existing_content.is_empty() {
        match operation {
            WriteOperation::Create => {
                return Err(PaneError::FileAlreadyExists {
                    path: path.to_path_buf(),
                    size: existing_content.len()
                });
            }
            WriteOperation::Update => {
                // Content loss guard: reject if new content < 50% of old
                if new_content.len() < (existing_content.len() / 2) {
                    log::error!(
                        "Write guard blocked: new content ({} bytes) < 50% of existing ({} bytes) for {}",
                        new_content.len(),
                        existing_content.len(),
                        path.display()
                    );
                    return Err(PaneError::ContentLossGuard {
                        path: path.to_path_buf(),
                        existing_size: existing_content.len(),
                        new_size: new_content.len()
                    });
                }
            }
            WriteOperation::Append => {
                // For append, merge new content with existing
                let merged_content = format!("{}\n{}", existing_content.trim_end(), new_content);
                return atomic_write(path, &merged_content);
            }
        }
    }
    
    // 4. Log the write
    log::info!(
        "System file write: {:?} operation on {} ({} → {} bytes)",
        operation,
        path.display(),
        existing_content.len(),
        new_content.len()
    );
    
    // 5. Atomic write (§2C.7)
    atomic_write(path, new_content)?;
    
    Ok(())
}

enum WriteOperation {
    Create,   // File must not exist or be empty
    Update,   // Replace content with safety check
    Append,   // Add to existing content
}
```

**Enforcement:**
- Code review checklist: "Does this code write to `.pane/system/`? If yes, does it use `safe_write_system_file()`?"
- Linter rule: flag any `std::fs::write()` with a path containing `.pane/system/`
- Unit tests: verify guard blocks destructive writes

### 2C.2 Synthesis Must Read-Before-Write

> **Status:** NOT BUILT

**Synthesis (§5.5) must NEVER blindly overwrite Memory notes.** The synthesis pipeline rewrites Memory notes to consolidate facts — this is the highest-risk write operation in Pane.

**Mandatory pattern:**

```rust
async fn synthesize_memory(memory_note_id: &str) -> Result<()> {
    // 1. Read existing Memory note
    let existing_memory = read_note_content(memory_note_id).await?;
    
    // 2. Send to AI with explicit instruction
    let synthesis_prompt = format!(
        "Rewrite this memory document. Consolidate duplicate facts, \
         organize by category, remove outdated information. \
         PRESERVE all important facts. Write in clean, readable paragraphs.\n\n\
         EXISTING MEMORY:\n{}",
        existing_memory
    );
    
    let synthesized_content = call_ai_synthesis(synthesis_prompt).await?;
    
    // 3. Safety net: reject if new content is drastically shorter
    if synthesized_content.len() < (existing_memory.len() * 70 / 100) {
        log::warn!(
            "Synthesis produced content <70% of original ({} → {} bytes). \
             Showing review dialog instead of auto-applying.",
            existing_memory.len(),
            synthesized_content.len()
        );
        
        // Show review dialog to user
        show_synthesis_review_dialog(
            memory_note_id,
            &existing_memory,
            &synthesized_content
        ).await?;
        
        return Ok(()); // User must approve manually
    }
    
    // 4. Write via guard
    safe_write_system_file(
        &get_note_file_path(memory_note_id)?,
        &synthesized_content,
        WriteOperation::Update
    ).await?;
    
    Ok(())
}
```

**The rule:** Synthesis reads → merges/appends → writes back. Never blind overwrite.

### 2C.3 Case-Insensitive Filesystem Awareness

> **Status:** NOT BUILT

**macOS HFS+ and APFS are case-insensitive by default.** Filenames in `.pane/system/` must be unique regardless of casing to prevent collisions.

**The problem:**
```
.pane/system/Brit Freed/MEMORY.md   (user's Memory note)
.pane/system/Brit Freed/Memory.md   (synthesis output)
```

On macOS, these are THE SAME FILE. Writing `Memory.md` overwrites `MEMORY.md`.

**Solution: Canonical naming convention + collision detection**

**Canonical names for system files (case-sensitive list):**
```
MEMORY.md
SOUL.md
IDENTITY.md
AGENTS.md
USER.md
TOOLS.md
HEARTBEAT.md
RULES.md
```

**Enforcement:**

```rust
fn validate_system_filename(filename: &str) -> Result<()> {
    const CANONICAL_NAMES: &[&str] = &[
        "MEMORY.md",
        "SOUL.md",
        "IDENTITY.md",
        "AGENTS.md",
        "USER.md",
        "TOOLS.md",
        "HEARTBEAT.md",
        "RULES.md",
    ];
    
    // Check if filename matches canonical name (case-insensitive)
    let normalized = filename.to_uppercase();
    
    for canonical in CANONICAL_NAMES {
        if normalized == canonical.to_uppercase() {
            // Filename must match exactly (case-sensitive)
            if filename != *canonical {
                return Err(PaneError::CaseCollision {
                    filename: filename.to_string(),
                    canonical: canonical.to_string(),
                    message: format!(
                        "System file must be named '{}', not '{}' \
                         (macOS filesystems are case-insensitive)",
                        canonical, filename
                    )
                });
            }
            return Ok(());
        }
    }
    
    Err(PaneError::UnknownSystemFile {
        filename: filename.to_string()
    })
}
```

**During file operations:**
- Before any write to `.pane/system/`, validate filename with `validate_system_filename()`
- On startup integrity check (§2A.5), scan for case-collision duplicates and log warnings
- User-facing error: "Memory note must be named 'MEMORY.md' (all caps) on macOS to avoid file collisions"

### 2C.4 Write Categories & Required Guards

> **Status:** NOT BUILT

Different operations require different guard behaviors. This table defines what guard each operation needs:

| Operation | Files Affected | Guard Required | Notes |
|-----------|---------------|----------------|-------|
| **Onboarding** | All system files | `Create` | Files must not exist or be empty. Onboarding never overwrites. |
| **Synthesis** | MEMORY.md | `Update` | Read existing → synthesize → write with <70% guard. |
| **Memory extraction** | MEMORY.md, project Memory.md | `Append` | AI appends facts, never replaces. |
| **Daily log** | `logs/YYYY-MM-DD.md` | `Append` | AI appends timestamped entries throughout the day. |
| **Manual save** | MEMORY.md, project Memory.md, daily log | `Append` | User saves from chat → appends to destination. |
| **User edit** | Any system file | `Update` | User edits in Pane editor. Content loss guard active. |
| **OpenClaw write** | Any system file (via symlink) | No guard (external) | OpenClaw writes directly to flat files. Pane detects changes on startup (§2A.5). |
| **Integrity check** | Missing system files | `Create` | Creates missing files from DB or defaults. |

**Create-only operations:**
- Onboarding: System files created once, never overwritten by the onboarding flow
- Integrity check: Missing files recreated from DB state or default templates

**Update operations:**
- Synthesis: Rewrites Memory notes with consolidation
- User edits: Full-content updates via Pane editor

**Append operations:**
- Memory extraction: AI adds facts to Memory notes
- Daily log writes: Timestamped entries appended throughout the day
- Manual save: User-triggered appends from chat UI

### 2C.5 Atomic Writes (Temp File + Rename Pattern)

> **Status:** NOT BUILT

**All writes to `.pane/system/` use atomic writes** to prevent partial writes from corrupting files.

**Pattern: write to temp file → rename to target**

```rust
async fn atomic_write(path: &Path, content: &str) -> Result<()> {
    let temp_path = path.with_extension("tmp");
    
    // 1. Write to temp file
    fs::write(&temp_path, content).map_err(|e| {
        PaneError::FileWriteFailed {
            path: temp_path.clone(),
            source: e
        }
    })?;
    
    // 2. Verify write succeeded
    let written_content = fs::read_to_string(&temp_path)?;
    if written_content != content {
        fs::remove_file(&temp_path)?;
        return Err(PaneError::WriteVerificationFailed {
            path: temp_path
        });
    }
    
    // 3. Atomic rename
    fs::rename(&temp_path, path).map_err(|e| {
        // Clean up temp file on failure
        let _ = fs::remove_file(&temp_path);
        PaneError::FileRenameFailed {
            from: temp_path.clone(),
            to: path.to_path_buf(),
            source: e
        }
    })?;
    
    // 4. Verify final file
    let final_content = fs::read_to_string(path)?;
    if final_content != content {
        log::error!(
            "Atomic write verification failed for {}. \
             Content mismatch after rename.",
            path.display()
        );
        return Err(PaneError::AtomicWriteFailed {
            path: path.to_path_buf()
        });
    }
    
    Ok(())
}
```

**Why atomic writes:**
- **Process crash during write** → incomplete temp file, target unchanged
- **Disk full during write** → write fails, target unchanged
- **Power loss** → at worst, temp file is incomplete, target unchanged
- **Filesystem rename is atomic** → the file is either old or new, never half-written

**What this prevents:**
- Corrupted Memory notes from incomplete writes
- Loss of existing content due to write failures
- Race conditions from concurrent reads/writes

### 2C.6 Audit Log for System File Writes

> **Status:** NOT BUILT

**Every write to `.pane/system/` is logged** for debugging and audit.

**Log format:**

```rust
log::info!(
    target: "pane::system_file_write",
    "operation={:?} path={} size_before={} size_after={} caller={}",
    operation,
    path.display(),
    existing_size,
    new_size,
    caller_function_name
);
```

**Example log output:**

```
[2026-03-25T19:42:17Z INFO  pane::system_file_write] operation=Append path=.pane/system/Brit Freed/logs/2026-03-25.md size_before=3241 size_after=3387 caller=write_daily_log
[2026-03-25T20:15:33Z INFO  pane::system_file_write] operation=Update path=.pane/system/Brit Freed/MEMORY.md size_before=12459 size_after=12891 caller=synthesize_memory
[2026-03-25T20:15:33Z WARN  pane::system_file_write] Content loss guard BLOCKED: path=.pane/system/Brit Freed/MEMORY.md existing=12459 bytes, new=6 bytes (synthesis)
```

**Log storage:**
- Console output (visible during dev)
- Log file: `.pane/logs/system-writes.log` (rotated daily, kept for 30 days)
- Searchable via Settings → Logs → System File Writes

**What this enables:**
- **Debugging:** "When did MEMORY.md last get written to?"
- **Incident investigation:** "What wrote to this file on Mar 25?"
- **User transparency:** User can see what the AI has written to their system files

### 2C.7 The Rule

> **Status:** N/A — design principle

**A write to `.pane/system/` that destroys existing user content is a data loss bug, full stop.**

No exceptions. No "usually safe" patterns. No "shouldn't happen in practice." If the write guard didn't catch it, the guard is wrong.

**System files are the user's knowledge base.** Losing MEMORY.md is like losing a journal. It's not "just data" — it's context, decisions, and identity that cannot be recreated.

**Every line of code that writes to `.pane/system/` is high-risk code.** Treat it accordingly:
- Mandatory code review
- Unit tests that verify guard behavior
- Integration tests that simulate filesystem failures
- Manual QA on every build

**When in doubt: read first, preserve existing, log everything, make it atomic.**

### 2C.8 Incident Record

> **Status:** HISTORICAL

**Mar 25, 2026 — MEMORY.md Overwrite via Synthesis**

**What happened:**
Synthesis pipeline overwrote a 245-line MEMORY.md with 6 lines of generated content, destroying accumulated agent memory.

**Failure chain:**
1. Synthesis ran on `MEMORY.md` (legitimate operation)
2. AI synthesis call produced 6 lines of generic content instead of consolidating existing 245 lines
3. Code used blind `std::fs::write()` — no existence check, no size comparison
4. File overwritten → 245 lines → 6 lines
5. User (Matt) discovered data loss hours later
6. No automated alert or guard triggered

**Root causes:**
1. **No write guard** — Direct `std::fs::write()` used instead of `safe_write_system_file()`
2. **No read-before-write** — Synthesis didn't read existing content to merge/append
3. **No content loss detection** — 6 lines replacing 245 lines went unnoticed by the system
4. **Case collision on macOS** — Synthesis may have written `Memory.md` (lowercase "m") which overwrote `MEMORY.md` on case-insensitive filesystem
5. **Audit found 9 out of 13 write paths** to `.pane/system/` use blind `std::fs::write()` with no guards (pane-file-write-audit.md)

**What was lost:**
- 245 lines of accumulated memory (identity, preferences, decisions)
- ~3 weeks of memory extraction since onboarding
- Context about ongoing projects and Matt's working style

**What survived:**
- Daily logs (separate files, not affected)
- Project-specific memory (different files)
- Database entries for extracted facts (but synthesis output lost the prose)

**Immediate fixes:**
1. Rollback: Restored MEMORY.md from git history (file was committed 2 days prior)
2. Loss: ~2 days of memory extraction gone, but majority recovered
3. Spec update: §2C added to architecture spec (this section)

**Permanent mitigations:**
1. **Write guard (§2C.1)** — All writes to `.pane/system/` go through `safe_write_system_file()`
2. **Read-before-write (§2C.2)** — Synthesis reads existing → merges → writes with guard
3. **Content loss guard** — Reject writes where new content < 70% of existing (with review dialog)
4. **Case-sensitive filenames (§2C.3)** — Enforce canonical `MEMORY.md` (all caps) to prevent collisions
5. **Atomic writes (§2C.5)** — Temp file + rename to prevent partial writes
6. **Audit logging (§2C.6)** — Every system file write logged with before/after sizes
7. **Code review checklist** — "Does this write to `.pane/system/`? Guard present?"
8. **Unit tests** — Verify guard blocks destructive writes (test cases from this incident)

**Lessons:**
- **File writes are high-risk operations.** Assume they will fail or corrupt. Guard everything.
- **Flat files are source of truth, but fragile.** A single bad write destroys knowledge that took weeks to build.
- **Content loss is silent.** The system happily overwrites 245 lines with 6 lines unless explicitly guarded.
- **Case-insensitive filesystems are a footgun.** Enforce canonical naming to prevent collision bugs.
- **Audits find gaps.** The pane-file-write-audit.md found 9 unguarded write paths — this incident proves the audit was necessary.

**This incident validates the principle:** A write to `.pane/system/` that destroys existing user content is a data loss bug, full stop. The guard exists for a reason. Use it.

### 2C.9 Implementation Checklist

> **Status:** NOT BUILT — spec only

**Backend (Rust):**
- [ ] Implement `safe_write_system_file()` with all guard logic (§2C.1)
- [ ] Implement `atomic_write()` helper (§2C.5)
- [ ] Implement `validate_system_filename()` for case-collision detection (§2C.3)
- [ ] Update synthesis pipeline to use read-before-write pattern (§2C.2)
- [ ] Update all 13 write paths identified in audit to use write guard
- [ ] Add audit logging for all system file writes (§2C.6)
- [ ] Add unit tests: guard blocks destructive writes, append works, create works, case collision detected
- [ ] Add integration tests: synthesis safety net, atomic write rollback on failure

**Error types:**
- [ ] `PaneError::FileAlreadyExists` — guard blocked Create operation on existing file
- [ ] `PaneError::ContentLossGuard` — new content < 50% of existing (for Update)
- [ ] `PaneError::CaseCollision` — filename doesn't match canonical casing
- [ ] `PaneError::WriteVerificationFailed` — content mismatch after write
- [ ] `PaneError::AtomicWriteFailed` — rename succeeded but verification failed

**UI (Svelte):**
- [ ] Synthesis review dialog (§2C.2) — shows old vs new content when guard triggers
- [ ] User action: Accept synthesis | Reject | Edit manually
- [ ] Health warning for case-collision detected files (Settings → Health)

**Testing:**
- [ ] Unit test: Create operation blocked on existing file
- [ ] Unit test: Update operation blocked when new < 50% of existing
- [ ] Unit test: Append operation merges content correctly
- [ ] Unit test: Case-collision detection catches `Memory.md` vs `MEMORY.md`
- [ ] Unit test: Atomic write rollback on partial failure
- [ ] Integration test: Synthesis triggers review dialog on <70% content
- [ ] Integration test: Simulate disk full during write → atomic rollback works
- [ ] Regression test: Reproduce Mar 25 incident scenario → guard blocks it

**Documentation:**
- [ ] Update developer onboarding docs with write guard rules
- [ ] Add code review checklist item for `.pane/system/` writes
- [ ] Add linter rule to flag unguarded `std::fs::write()` in system paths

### 2C.10 Future Enhancements (Post-Launch)

> **Status:** NOT BUILT — spec only (future work)

1. **Version history for all system files** — silent snapshots on every write (§5.8 covers Memory notes, extend to all system files)
2. **Diff view in synthesis review dialog** — side-by-side comparison of old vs new content
3. **Smart synthesis detection** — flag suspiciously short synthesis output before sending to guard
4. **Checksum verification** — BLAKE3 hash stored with each write, verified on read
5. **Filesystem monitoring** — detect external writes (OpenClaw, user edits outside Pane) and surface warnings
6. **Backup rotation** — keep last N versions of each system file in `.pane/backups/` (separate from version history)
7. **Recovery UI** — "Your MEMORY.md looks corrupted — restore from backup?" with one-click recovery

---

## 2D. Pre-Ship Audit Requirements

> **Status:** NOT BUILT — spec only (permanent architectural section added Mar 25, 2026)

This section codifies mandatory audit gates for milestone launches and data-path builds to prevent cascading failures. Every rule here exists because it was violated once.

### 2D.1 The Rule (MANDATORY)

> **Status:** NOT BUILT

**Before any milestone ships OR any build touching data paths merges, specific audits must pass. No exceptions.**

A milestone or data-path build that fails its audit checklist does not ship. Period. The cost of shipping a broken migration or overwrite bug is catastrophic — we learned this on Mar 25, 2026.

### 2D.2 Two Tiers of Audit

> **Status:** NOT BUILT

#### Tier 1: Before Every Milestone

> **Status:** NOT BUILT

**Milestones:** move-in (first real user data), public launch, new pricing tier, new platform (iOS, Windows).

**Mandatory audits:**

**1. Destructive testing** — Deliberately break things to verify recovery:
- Wipe the database mid-operation → verify nothing gets destroyed that shouldn't
- Kill the app mid-write → verify partial writes roll back cleanly
- Disconnect network during sync → verify data integrity maintained
- Corrupt a system file → verify app recovers or surfaces actionable error
- **Document what was tested and what survived.** "Killed during migration → rollback worked, backup restored successfully."

**2. Write path audit** — Inventory every code path that writes to user files:
- Run: `grep -rn "std::fs::write\|write_note_content\|safe_write_system_file\|append_to_note" src-tauri/src/ --include="*.rs"`
- Verify all writes to `.pane/system/` go through the write guard (§2C.1)
- Verify no new direct `std::fs::write()` to `.pane/system/` bypasses guards
- **Document any exceptions and why they're safe.**

**3. Failure cascade analysis** — For each major component (DB, files, sync, AI), map: "if X fails, what else breaks?"
- Document the chains: "DB wipe → what survives? (Flat files.) What's lost? (Extracted facts, chat history.)"
- Migration failure → does it trigger fresh DB creation? (It MUST NOT per §2B.2.)
- File write failure → does existing content survive? (It MUST per §2C.7.)
- **Produce a failure cascade diagram** — visual map of dependencies and failure modes.

**4. Backup verification** — Not just "backup code exists" but actual verification:
- Trigger pre-migration backup (§2B.1) → verify file exists
- Verify backup is readable (open it, query a table, read a flat file)
- **Verify it can restore** — actually restore from backup and confirm data integrity
- Document: "Restored from backup taken {timestamp} → {what was recovered}."

**5. File integrity check** — After destructive test, verify all `.pane/system/` files match their pre-test content or were safely preserved:
- Checksum all system files before test
- Run destructive test
- Checksum all system files after
- Any differences → explain why or fail the audit

**Deliverable:** A written audit report covering all five checks, filed as `docs/audits/milestone-{name}-{date}.md` and linked from the milestone's GitHub issue.

#### Tier 2: Before Every Build That Touches Data Paths

> **Status:** NOT BUILT

**Triggers:** Any PR/commit that modifies:
- `std::fs::write` or `fs::write` calls
- `write_note_content` or any write-to-disk function
- Migrations (`migrations.rs`)
- Integrity check (`verify_system_notes_integrity`)
- Synthesis pipeline
- Flat-sync / write-back system
- Bridge import/export

**Detection command (run in CI):**
```bash
grep -rn "std::fs::write\|write_note_content\|migrations\|integrity\|synthesis\|flat.sync" src-tauri/src/ --include="*.rs"
```

If the diff touches any of these, Tier 2 audit is MANDATORY before merge.

**Required for triggered builds:**

**1. "What could this overwrite?" checklist** — Developer must answer in PR description:
```markdown
### Data Path Audit Checklist
- [ ] What files does this code write to?
- [ ] Does it check existence before writing?
- [ ] What happens if the file already has content?
- [ ] Does it use the write guard (§2C.1) or explain why not?
- [ ] If it's a migration, does it use `crsql_begin/commit_alter` for CRR tables?
```

**2. Migration tested on copy of real DB** — Never test migrations on fresh DB only:
- Copy `pane.db` from a dev environment with real data
- Run migration against the copy
- Verify no data loss, schema matches expected
- Document in PR: "Tested on DB copy from {source} with {N} notes, {M} projects."

**3. Backup verified post-build** — Confirm pre-migration backup ran and is readable:
- Build the branch
- Trigger a migration (if applicable)
- Verify backup file was created per §2B.1
- Verify backup is valid (can be opened and queried)
- Document in PR: "Pre-migration backup verified at {path}, {size}."

**4. Write guard compliance** — Verify no new direct `std::fs::write()` to `.pane/system/`:
- Run write path audit (same command as Tier 1 #2)
- Any new direct writes → must use `safe_write_system_file()` or explain exception
- Reviewer verifies in code review

**Enforcement:** CI checks for trigger patterns. If detected, PR requires:
- Checklist filled in PR description
- Reviewer approval from someone who didn't write the code
- "Audit: PASSED" label before merge

### 2D.3 What Does NOT Need Audit

> **Status:** NOT BUILT

Not every change needs the full audit gate. These are explicitly excluded:

- **Pure UI changes** — CSS, layout, labels, component styling (no file or DB writes)
- **Read-only operations** — Queries, searches, rendering logic
- **Frontend-only logic** that doesn't invoke Tauri commands touching files/DB
- **Documentation updates** — README, spec changes, comments
- **Test-only changes** — Adding tests without touching production code

**Guideline:** If the code path cannot write to disk or DB, it doesn't need Tier 2 audit. If you're unsure, run the trigger detection command — if it doesn't match, you're clear.

### 2D.4 Incident Record

> **Status:** HISTORICAL

**Mar 25, 2026 — Migration Failure → Data Loss Cascade**

**What happened:**
Migration 007 (compaction feature) shipped without destructive testing. The migration attempted `ALTER TABLE` on a CRR table with `NOT NULL`, which cr-sqlite rejected. Migration failure cascaded into DB wipe + file overwrites, destroying projects, chat history, and portions of accumulated memory.

**Failure chain:**
1. Migration 007 added `summary` column to CRR table `chat_messages_local` with `NOT NULL`
2. cr-sqlite rejected the alteration (§2B.3 constraint violated)
3. Migration failed → `run_migrations()` returned `Err`
4. `init_db()` propagated error instead of continuing with old schema (§2B.2 violated)
5. App initialization failed
6. App recreated fresh database (because `init_db()` error suggested missing DB)
7. **All DB-only data lost:** projects, chat history, extracted facts
8. **Flat-file data survived:** agent notes (SOUL.md, MEMORY.md), daily logs
9. Hours later, synthesis ran and overwrote MEMORY.md with 6 lines instead of 245 (§2C violated)

**What would have caught it:**

| Audit | Would Have Caught | How |
|-------|------------------|-----|
| **Destructive testing (Tier 1 #1)** | ✅ Migration failure | Test: wipe DB mid-migration → app recovers or fails gracefully |
| **Write path audit (Tier 1 #2)** | ✅ MEMORY.md overwrite | Grep would have found unguarded `std::fs::write()` to system files |
| **Failure cascade analysis (Tier 1 #3)** | ✅ DB wipe cascade | Map: migration fails → what happens? Answer: fresh DB creation (WRONG) |
| **Backup verification (Tier 1 #4)** | ✅ Recovery path | Verify: backup exists, is readable, can restore → would have enabled immediate recovery |
| **Migration tested on real DB (Tier 2 #2)** | ✅ CRR constraint violation | Real DB has CRR tables → migration would fail in testing |
| **What could this overwrite? (Tier 2 #1)** | ✅ Synthesis overwrite | Dev would answer: "Synthesis writes to MEMORY.md" → triggers guard review |
| **Write guard compliance (Tier 2 #4)** | ✅ Direct write detected | Grep would flag `std::fs::write()` to MEMORY.md without guard |

**ALL SEVEN AUDITS would have caught this incident.** That's why they're mandatory now.

**Outcome:**
- §2B (Database Safety) added — pre-migration backup, graceful failure, CRR rules
- §2C (File Write Safety) added — write guards, read-before-write, atomic writes
- §2D (this section) added — audit gates to enforce §2B and §2C before shipping
- Migration 007 rewritten to use separate `compaction_summaries` table (non-CRR)
- Architecture spec updated with incident record in each relevant section

**Lesson:** A single uncaught bug in a data path can cascade into catastrophic data loss. Audits are not optional. They are the permanent scar tissue from this incident — the mechanism that ensures we never make this mistake again.

### 2D.5 PR Checklist Template (Copy-Paste)

> **Status:** NOT BUILT

For PRs that trigger Tier 2 audit (detected by CI or manually identified):

```markdown
## Data Path Audit Checklist (§2D Tier 2)

**Trigger detection:**
- [ ] Ran: `grep -rn "std::fs::write\|write_note_content\|migrations\|integrity\|synthesis\|flat.sync" src-tauri/src/ --include="*.rs"`
- [ ] This PR modifies: [list files from grep output]

**Write impact analysis:**
- [ ] Files written by this code: [list paths]
- [ ] Existence check before write: [yes/no + where in code]
- [ ] Behavior if file already has content: [describe]
- [ ] Write guard used (§2C.1): [yes/no + function name OR exception reason]
- [ ] If migration: uses `crsql_begin/commit_alter` for CRR tables: [yes/no/not-applicable]

**Testing:**
- [ ] Migration tested on copy of real DB: [yes + source/size OR not-applicable]
- [ ] Pre-migration backup verified: [yes + path/size OR not-applicable]
- [ ] Destructive test performed: [describe test OR not-needed-because]
- [ ] Write path audit passed: [all writes guarded OR exceptions documented]

**Reviewer verification:**
- [ ] Code reviewed by: [@reviewer-github-handle]
- [ ] Audit checklist verified complete
- [ ] Label: `audit:passed` applied before merge

**Notes:**
[Any additional context, exceptions, or justifications]
```

Paste this into the PR description when CI detects a data-path change. Fill it out before requesting review.

### 2D.6 CI Integration (Future)

> **Status:** NOT BUILT — spec only (future work)

**Automated detection:**
- GitHub Actions workflow runs trigger detection command on every PR
- If matches found, bot comments on PR with checklist template
- PR cannot merge until:
  - Checklist is filled in description
  - `audit:passed` label is applied by a reviewer
  - At least one approval from a maintainer

**Milestone gate:**
- GitHub milestone cannot be closed until audit report exists in `docs/audits/`
- Release workflow checks for audit file before building DMG
- No DMG ships without green audit

**Future enhancement:** Pre-commit hook that warns locally when a commit touches data paths — gives developer immediate feedback before pushing.

### 2D.7 Implementation Checklist

> **Status:** NOT BUILT — spec only

**Documentation:**
- [ ] Add audit checklist template to `.github/PULL_REQUEST_TEMPLATE.md` (conditional section)
- [ ] Create `docs/audits/` directory with README explaining the process
- [ ] Add audit requirement to `CONTRIBUTING.md`
- [ ] Update developer onboarding docs with audit expectations

**CI/CD:**
- [ ] GitHub Actions workflow: detect data-path changes, comment with checklist
- [ ] Branch protection rule: require `audit:passed` label for PRs with data-path changes
- [ ] Milestone workflow: check for audit report before allowing milestone close
- [ ] Release workflow: verify audit report exists before building DMG

**Process:**
- [ ] Train team on audit requirements (what, when, why)
- [ ] Conduct first Tier 1 audit on current `main` branch as baseline
- [ ] File baseline audit report in `docs/audits/baseline-2026-03-25.md`
- [ ] Enforce Tier 2 on all open PRs touching data paths (retroactive)

**Testing:**
- [ ] Integration test suite: destructive testing scenarios (kill mid-write, corrupt DB, network disconnect)
- [ ] Migration test suite: test all migrations on copy of real DB
- [ ] Write path test: verify all `.pane/system/` writes go through guard

### 2D.8 The Philosophy

> **Status:** N/A — design principle

**Data loss is worse than shipping late.** A broken feature can be fixed in the next build. Lost user data cannot be recovered. The user's trust, once broken, may never return.

**Audits slow us down on purpose.** The friction is the point. It forces us to think through failure modes before they happen, not after. If an audit feels tedious, that's a sign it's working — you're being thorough.

**This spec section is scar tissue.** It exists because we learned these lessons the hard way. Every checklist item maps to a real failure that actually happened. We're not being paranoid — we're being informed by experience.

**Shipping is a privilege, not a right.** A build earns the right to ship by passing its audits. No shortcuts. No "we'll fix it in the next one." If it doesn't pass, it doesn't ship.

**When in doubt, audit.** If you're unsure whether a change needs Tier 2 audit, run the trigger detection. If you're on the fence, do the checklist anyway. Over-auditing costs time. Under-auditing costs data.

---


---

## Appendix B: Migration Checklist (Current → New Architecture)
> **Status:** N/A (reference doc)

1. **Schema migration:** Unify `notes` + `tasks` → single `notes` table
2. **File system:** Create `~/Pane/` structure, migrate existing notes to `Projects/` directories
3. **Add new tables:** `ai_memory`, `ai_actions`, `user_profiles`, `user_roles`, `sync_state`, `ai_message_queue`, `extensions`
4. **Update Tauri commands:** All CRUD commands updated to new unified schema
5. **Update TypeScript types:** Replace `Task` + `NoteMeta` with unified `NoteMeta`
6. **Context assembler:** New Rust module that builds AI prompts from current view state
7. **Tool layer:** Register all AI tools with the LLM client
8. **Memory extraction:** Async pipeline after each AI response
9. **Audit logging:** Middleware on all AI tool calls
10. **Notification engine:** Background task in Tauri async runtime

---

