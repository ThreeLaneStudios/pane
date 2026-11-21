# Data Change Refresh Point

**Last Updated:** 2026-10-12
**Sections:** §34
**Issue:** #2972 (tracked on #2874)

> **Cross-references:** `sheets.md` §33.11 (AI tools — sheet-tool emit parity), `toolbar.md` "5. Move to Trash" (editor's own `pane-data-changed` dispatch), `ai-integration.md` §4.1 (tool dispatch loop), `core-data-model.md` §1 (entity model).

> **Background:** `~/Shared/ExampleApp/docs/research/2026-09-24-scope-refresh-point.md` (the scope doc, "the scope doc" below). That doc dates from v1.0.57 and is **stale on two points** — see §34.0 below before trusting any of its code citations. This spec supersedes it as the governing document for #2972; the scope doc remains useful for its sender/listener inventory (§34.1/§34.2 here re-verify that inventory against current `main`).

---

## 34. Data Change Refresh Point

> **Status:** BUILT (#2972, tracked on #2874)

**Matt's ruling (authoritative):** one event for every data change across notes, sheets, folders, and projects. It carries the entity ID, the entity type, and a reason (`created`, `edited`, `moved`, `deleted`). It replaces `pane-data-changed`, `pane-note-created`, `pane-note-changed`, and `pane-project-changed`. Two senders — backend and app — emit the identical payload shape. Echo suppression is by per-save origin tag, precise enough that a note still refreshes when changed by someone/something else. `pane-task-meta-changed` folds in. Every screen listens and updates only what changed; the board patches/inserts/removes one card, never reloads. `NoteList.svelte` and `pane-refresh-notes` are deleted once confirmed dead. The `mutating_tools` allowlist is deleted — the tool loop's single emit collects touched IDs itself.

This section specifies that event's payload, its two senders, every listener's post-migration behavior, the migration/deletion of the six events and two dead-code items it replaces, and the four-stage build-out. **No code changes in this PR** — every behavior below is specified ahead of implementation, per §34.9.

---

### 34.0 Stale scope-doc references — corrected

The scope doc (2026-09-24, written against v1.0.57/merge-base `169917be6225c32ba6277742c4c2082f25b82bed`) is cited throughout this spec for its sender/listener inventory and design reasoning, but **two things changed under it** between then and this PR's base (`2153cc5331ce6b108535e94f28b0c23e2339de73`, #3011). Every scope-doc code reference was re-verified against that base commit for this spec; the ones that had gone stale:

| Scope-doc reference | Scope-doc claim (v1.0.57) | Current value at `main` (#3011) |
|---|---|---|
| `resolve_folder_id_or_create` (§3, "silent auto-create inside `resolve_folder_id_or_create`" (deleted outright, #2874 item 50b)) | A silent folder-auto-create path invisible to any allowlist, called from three sites including `execute_create_sheet` | **Deleted outright.** #2874 item 50b (#3011, merged) replaced it with `resolve_folder_branch_a` (explicit `folder_id`) and `resolve_folder_branch_b` (context-derived folder name), both `ai::tools`, — both are **refusal-on-miss**, never create-on-miss (ai-integration.md §4.1's "Folder resolution for `create_note` and `create_sheet`" contract, now BUILT). The scope doc's §3 "folder creation, silent path" gap **no longer exists as a code path at all** — there is nothing left to wire into the refresh point on that branch. Only two folder-creation paths remain live: the explicit `create_folder` tool (`execute_create_folder`, `ai::tools`) and the human Sidebar "New Folder" path (§34.1.4 below). |
| Sheet tool list (§3, "five, confirmed exhaustive") — `create_sheet`, `set_sheet_cells`, `add_sheet_row`, `add_sheet_column`, `delete_sheet_column` | Five mutating sheet tools + `get_sheet_data` read-only = six total | **Eight tools now, seven mutating.** #2970 (merged, PR referenced in `CHANGELOG.md`) added `add_sheet_rows` and `add_sheet_columns` (both `ai::tools`) as batch variants. Current full list, re-enumerated in §34.7 below from `src-tauri/src/ai/tools.rs` (schema and dispatch both live there post-#3097) — do not reuse the scope doc's five/six-tool count. |
| `mutating_tools` allowlist (§3, "nine entries, zero sheet tools") | `create_note, edit_note, append_to_note, set_status, set_priority, create_subtask, delete_note, create_folder, create_project` — no sheet tools at all | **Fifteen entries at `main`** (`ai::chat::run_agent_loop`'s spawned task): the original nine, plus **all six mutating sheet tools** (`set_sheet_cells`, `add_sheet_row`, `add_sheet_column`, `delete_sheet_column`, `add_sheet_rows`, `add_sheet_columns`) — added as a stopgap by the #2970 PR itself and by the item-62 hardening pass, per the `mutating_tools` block's own comment (same function), which **names #2972 (this issue) as the real fix**. `create_sheet` is conspicuously **absent** from the list even after that stopgap (verified by reading the 15-entry array directly) — a pre-existing gap this spec's unconditional emit (§34.7) closes as a side effect, not something #2970 introduced. The payload is still `{tools: [...], results: [...]}` — no entity ID, exactly as the scope doc described; only the gate list grew. |
| the AI-stream emitter, "no entity ID at all" | Fires `pane-note-changed` with a tool-name array, no entity ID | Still true at `main` — now in `ai::chat::run_agent_loop`, unconditional shape unchanged except the allowlist above it is longer. This is the exact code this spec's §34.5 replaces. |
| `sheets.md:361-372` §33.11, "six sheet tools" sentence | Lists `create_sheet, get_sheet_data, set_sheet_cells, add_sheet_row, add_sheet_column, delete_sheet_column` as "the agent has six sheet tools" | **Stale at `main`** (now eight — see row 2 above). §33.11's sentence needs updating as part of this issue's eventual code PR; not changed here since this PR is docs-only for #2972's own scope, but flagged so the sheets.md fix isn't lost — see §34.8. |

Everything else in the scope doc's §2 (sender/listener inventory) and §6 (per-screen table) was re-verified line-by-line against `main` for this spec and found either unchanged or shifted only by harmless line-number drift (both cited directly, by current line, in §34.1/§34.2/§34.6 below — the scope doc's own line numbers are not reused uncorrected anywhere in this document).

---

### 34.1 The event — name, payload, senders

**Name:** `pane-data-changed` (reused — the most-referenced of the six names it replaces, per §34.3's count; renaming it a seventh time would just be more churn for no behavior gain).

**Payload (both senders, byte-identical shape):**

```json
{
  "entity_id": "string, required — the note/sheet/folder/project/category ID (or name, for category) that changed",
  "entity_type": "note | sheet | folder | project | category",
  "reason": "created | edited | moved | deleted",
  "origin_tag": "string, required — see §34.4 echo suppression"
}
```

- `entity_id` is non-optional. There is no variant of this event that doesn't know what changed — that was the AI-stream emitter's whole defect (§34.0 row 4) and this event does not reproduce it.
- `entity_type` discriminates note vs. sheet vs. folder vs. project vs. category. This resolves the scope doc's §1 open question ("is `pane-project-changed` folded in?") — ruling 1 says explicitly "every data change... notes, sheets, folders and projects," so yes: one event, originally four entity types, not a separate project-scoped channel. **#3018 adds `category` as a fifth `entity_type`** (Matt's ruling, implemented in this issue): renaming, recoloring, or deleting a category in Settings previously refreshed nothing anywhere (rename/delete regressed in Stage 2 when their payload-less dispatch stopped matching Stage 3's `subscribeDataChanged()`, which no-ops on a missing `detail`; recolor never refreshed at all). `entity_id` for `category` is the category name itself — there is no other stable identifier (`category_colors` has no surrogate id column). The same four `reason` values apply unchanged, no fifth reason: add = `created`, rename/recolor = `edited`, delete = `deleted`. Both backend (`refresh_event.rs`) and app (`refreshEvent.ts`) mirrors gain the `ENTITY_CATEGORY` constant together (§34.1's "two senders, one shape" requirement extends to the enum itself — a drift between the two mirrors means events silently never match).
- `reason` is exactly the four values ruling 1 names. No fifth value. A tool/command that doesn't cleanly map to one of the four (there are none identified in this audit — every mutation found in §34.2/§34.7 is a create, an edit, a move, or a delete) does not get a special case; if the implementation PR finds one, it is a sign the operation is mis-modeled, not a sign the enum needs a fifth value.
- `origin_tag` is required on every emit, including backend-initiated ones (§34.4).

**Two senders (ruling 2), one shape:**

1. **Backend (Rust, Tauri `emit()`).** Fires for agent/tool-loop changes and background jobs (daily-log auto-append, memory synthesis/consolidation) — anything the frontend didn't directly initiate via its own successful `invoke()`. Same-process frontend mutations are **not** re-notified by a second backend round-trip; see the next point.
2. **App (TypeScript, DOM `CustomEvent`).** Fires after the frontend's own successful `invoke()` calls that mutate data — i.e., every site enumerated in §34.2 as a `pane-data-changed`/`pane-note-created` DOM-CustomEvent sender today, migrated to the new shape with a real `entity_id`/`entity_type`/`reason`/`origin_tag` instead of no payload at all.

This resolves the scope doc's §1 architecture question (one channel vs. two) explicitly: **two channels, identical payload contract**, exactly the scope doc's own recommendation — least code churn, and genuinely backend-initiated changes (AI tool loop, background jobs) have no choice but to cross the Tauri IPC boundary regardless.

**34.1.1 Backend emitters (what the event replaces, §34.7 covers the full set):**
- `notes.rs:3264` (`move_note`) — today emits `pane-note-changed` with `{note_id, reason: "note_moved"}`. Migrates to `{entity_id: note_id, entity_type: "note", reason: "moved", origin_tag}`.
- `notes.rs:3289` (`move_folder`) — today emits `pane-note-changed` with `{folder_id, reason: "folder_moved"}`. Migrates to `{entity_id: folder_id, entity_type: "folder", reason: "moved", origin_tag}`.
- `ai::chat::run_agent_loop`'s tool-loop emitter (`{tools, results}`, no entity ID) — **replaced entirely** by §34.5/§34.7's unconditional, entity-identified, allowlist-free emit.
- the daily-log auto-append path (`ai::chat`) and the memory-synthesis path (`ai::scheduler`) — today emit `pane-note-changed` with `{note_id, reason: "..."}`. Migrate to the new shape, `reason: "edited"`.
- `projects.rs:217` (`create_project`) and `update_project_settings` (`ai::connections`) — today emit `pane-project-changed`. Migrate to `{entity_id: project_id, entity_type: "project", reason: "created"|"edited", origin_tag}`.
- **Correction (#2874 item 4, lane 1 PR 4):** `create_project_internal` (`projects.rs`) creates the project row AND its auto-created default folder row in the SAME transaction — both exist in the DB immediately. The `create_project` bullet above, as originally written, only accounted for the project's own event; it did not call for a second emit for the folder that same call just created. Without it, the sidebar's single-entity patch handler (`handleDataChangedEvent`, §34.5) never learns the folder's id, and it stays invisible until something unrelated (e.g. creating a SECOND folder) happens to trigger a full reload. **Both** callers — the `create_project` Tauri command and the AI agent's `create_project` tool — now emit TWO events from one successful call, via one shared helper (`projects::emit_project_created_events`): `{entity_id: project.id, entity_type: "project", reason: "created"}` immediately followed by `{entity_id: project.default_folder_id, entity_type: "folder", reason: "created"}`. No new `entity_type`/`reason` value — both existing values, from §34.1's table, used twice.

**34.1.2 App emitters — `src/lib/api.ts` (DOM CustomEvent today, no payload):**
- `saveNote()` (`api.ts:136`) → `entity_type: "note", reason: "edited"`.
- `saveSheetTitle()` (`api.ts:160`) → `entity_type: "sheet", reason: "edited"`.
- `moveToTrash()` (`api.ts:450`) → `entity_type: "note", reason: "deleted"`.
- `setProjectCategory()` (`api.ts:1493`) → `entity_type: "project", reason: "edited"` (one project's own `category` field changed — unchanged by #3018).
- **#3018 (category as a fifth `entity_type`):** `setCategoryColor()` (`api.ts:1542`, backs both Settings.svelte's "add category" and "recolor" flows — `reason` passed by the caller, `"created"` for add / `"edited"` for recolor), `renameCategory()` (`api.ts:1562`), and `deleteCategory()` (`api.ts:1569`) → `entity_type: "category", entity_id: <category name>`. `renameCategory`'s and `deleteCategory`'s backend commands (`projects.rs:1145`/`:1199`) also change `projects.category` in bulk across every affected project — each returns the affected project IDs from the **same transaction** that changed them (not worked out from frontend state), and the app dispatches **one `project`/`edited` event per returned ID** (zero if none) immediately after the single `category` event, so every screen that groups/colors projects by category regroups without a restart. Supersedes this bullet's pre-#3018 text (`setProjectCategory`/`renameCategory`/`deleteCategory` all as one `project`/`edited` line) — that text described Stage 3's intended-but-unimplemented plan; Stage 3's actual code left `renameCategory`/`deleteCategory` on the old bare, payload-less dispatch (see that function's own pre-#3018 comment, since removed), which silently stopped refreshing anything once Stage 3's `subscribeDataChanged()` landed (it no-ops on a missing `detail`). #3018 is the fix.
- `createFolder()` (`api.ts:1067`) — **currently dispatches nothing at all** (confirmed: no `window.dispatchEvent` call in this function, unlike every sibling above). **Correction, Stage 3 (#2972 item 5):** this bullet originally called for `createFolder()` to gain an app-side dispatch here. By the time Stage 3 landed, Stage 1 had already made the backend `create_folder` command (`projects.rs`) emit the single event unconditionally, every call — including the one this wrapper triggers. Per ruling 1's "one event for every data change," adding a second, app-side dispatch for the identical mutation would violate that ruling, not satisfy it. `createFolder()` stays at zero dispatches, permanently — the backend is this mutation's sole sender.

**34.1.3 App emitters — components:**
- `Editor.svelte:480` (`handleMoveToTrash`) and `Editor.svelte:659` (`handleFolderMove`) — today both dispatch bare `pane-data-changed` with no detail. `:480` duplicates `api.ts:450`'s dispatch on the same action (confirmed double-dispatch today) — the duplication is removed as part of this migration, not preserved.
- `+page.svelte:550` (template applied) — migrates with `entity_type: "note", reason: "edited"`.
- `NotesBrowser.svelte:1075` (cabinet drag-drop status change) — migrates with `entity_type: "note", reason: "edited"`.
- `+page.svelte:597`, `:635` (re-dispatch of `pane-data-changed` from inside the old `pane-note-changed` listener, for `delete_note`/other tool calls) — **removed**, not migrated. Once the backend emit itself carries `entity_id`/`entity_type`/`reason` (§34.1.1), this hand-rolled bridge between the two old channels has nothing left to do.

**34.1.4 The third folder-creation gap — Sidebar "New Folder" (human path):**

`Sidebar.svelte:452` (`submitNewFolder`) → `api.createFolder()` → `api.ts:1067`. Confirmed at `main`: this path dispatches no event of any kind on the app side — it works today only because `Sidebar.svelte:465` manually re-fetches `$projects` for itself right after (its own, event-system-independent refresh, unchanged by this issue). No sibling component (KanbanBoard, NotesBrowser, another Sidebar instance) learned a folder was created through this path **before Stage 1** closed the gap from the backend side instead: ruling 7 ("the sidebar's New Folder" is explicitly named as a required emit source) is satisfied by `create_folder` (`projects.rs`, the Tauri command this Sidebar path's `invoke()` call reaches) emitting the single event unconditionally, every call, since #2972 stage 1 — not by an app-side dispatch here (§34.1.2's correction above). The Sidebar's own three sibling screens (KanbanBoard, NotesBrowser, another Sidebar instance) all now hear that backend emit via their own Stage 2/3 listeners.

---

### 34.2 Listener migration — every current sender/listener and its verdict

Full `file:line` inventory, re-verified against `main` (#3011) for this spec. Columns: **Verdict** — Replaced (removed, folded into §34.1's event) / Retained (stays distinct) / Deleted (dead code removed outright).

| Event (old) | Site | Role | `file:line` (current) | Verdict |
|---|---|---|---|---|
| `pane-data-changed` | sender | `saveNote()` | `api.ts:136` | Migrated (§34.1.2) |
| `pane-data-changed` | sender | `saveSheetTitle()` | `api.ts:160` | Migrated (§34.1.2) |
| `pane-data-changed` | sender | `moveToTrash()` | `api.ts:450` | Migrated (§34.1.2) |
| `pane-data-changed` | sender | `setProjectCategory` | `api.ts:1493` | Migrated (§34.1.2) |
| `pane-data-changed` | sender | `setCategoryColor` (add + recolor) | `api.ts:1542` | **#3018:** migrated to `entity_type: "category"`, `reason` by caller (`"created"`/`"edited"`) — was zero-dispatch (no payload, no refresh) before #3018 |
| `pane-data-changed` | sender | `renameCategory` (category event + per-affected-project `project`/`edited` events) | `api.ts:1562` | **#3018:** migrated — was a bare, payload-less dispatch (regressed to no-op by Stage 3's `subscribeDataChanged()`) before #3018 |
| `pane-data-changed` | sender | `deleteCategory` (category event + per-affected-project `project`/`edited` events) | `api.ts:1569` | **#3018:** migrated — same prior regression as `renameCategory` |
| `pane-data-changed` | listener | `Sidebar.svelte` category list reload | `Sidebar.svelte` (`handleDataChangedEvent`, `entity_type === "category"` branch) | **#3018:** new branch — re-reads the whole category list (§34.5: tiny, no per-item read, deliberate) on any category event |
| `pane-data-changed` | listener | `ProjectsView.svelte` | `ProjectsView.svelte` (`handleDataChangedEvent`) | **#3018:** new — this view shows categories (the per-project `category-indicator` dot) but had **zero** `pane-data-changed` listener of any kind before #3018, for any entity type. Gains a listener scoped to `category` (re-reads the category list) and `project` (patches the one affected project, so a rename/delete's bulk `projects.category` change repaints the dot) — not the broader note/sheet/folder Stage-2/3-equivalent migration this view still lacks for everything else, which is out of #3018's scope. |
| `pane-data-changed` | sender | `Editor.svelte` trash action | `Editor.svelte:480` | Replaced — duplicate dispatch removed (§34.1.3) |
| `pane-data-changed` | sender | `Editor.svelte` folder move | `Editor.svelte:659` | Migrated (§34.1.3) |
| `pane-data-changed` | sender | `NotesBrowser.svelte` cabinet drag-drop | `NotesBrowser.svelte:1075` | Migrated (§34.1.3) |
| `pane-data-changed` | sender | `+page.svelte` template applied | `+page.svelte:550` | Migrated (§34.1.3) |
| `pane-data-changed` | sender | `+page.svelte` bridge re-dispatch (×2) | `+page.svelte:597,635` | Replaced — removed (§34.1.3) |
| `pane-data-changed` | listener | `KanbanBoard.svelte` board reload | `KanbanBoard.svelte:224` (`loadTasks`) | Replaced — patch-in-place handler, §34.5 |
| `pane-data-changed` | listener | `Editor.svelte` metadata reload | `Editor.svelte:1895` (`handleDataChanged`) | Replaced — echo-suppressed targeted update, §34.4 |
| `pane-data-changed` | listener | `NoteList.svelte` | `NoteList.svelte:45` | Deleted (§34.6 — component is dead) |
| `pane-data-changed` | listener | `NotesBrowser.svelte` full reload | `NotesBrowser.svelte:668` (`dataHandler`) | Replaced — targeted patch, §34.5 |
| `pane-data-changed` | listener | `Sidebar.svelte` project tree reload | `Sidebar.svelte:97` (`handleProjectsChanged`) | Migrated — same handler now fed real `entity_type: "folder"/"project"` payloads instead of a bare event |
| `pane-data-changed` | listener | `syncStore.ts` auto-sync trigger | `syncStore.ts:111` | Migrated — fires on every mutation incl. sheet tools, closing the "sheet edits never trigger auto-sync" gap the scope doc found |
| `pane-data-changed` | listener | `AllTasksView.svelte` full reload | `AllTasksView.svelte:175` (`dataHandler`) | Replaced — targeted patch, §34.5 |
| `pane-note-created` | sender | `api.ts` various `create_*` | `api.ts:203,242,498,887,1889` | Migrated — `reason: "created"` variant of the same payload |
| `pane-note-created` | sender | `+page.svelte` | `+page.svelte:633` | Migrated |
| `pane-note-created` | listener | `KanbanBoard.svelte`, `NoteList.svelte`, `NotesBrowser.svelte`, `AllTasksView.svelte`, `Sidebar.svelte`, `syncStore.ts` | (6 sites) | Same disposition as each component's `pane-data-changed` listener above — one handler, both reasons |
| `pane-task-meta-changed` | sender | `Editor.svelte` field pills/title/preview | `Editor.svelte:610,1591,1784`(approx., field-level dispatch sites) | Replaced — folds into the single event, `reason: "edited"` (ruling 4) |
| `pane-task-meta-changed` | listener | `KanbanBoard.svelte`, `AllTasksView.svelte`, `NotesBrowser.svelte` | `KanbanBoard.svelte:149` (`handleTaskMetaChanged`), others | Replaced — folds into each screen's single new-event handler; the **existing field-patch logic in `handleTaskMetaChanged` is the donor pattern** the new handler generalizes (§34.5) |
| `pane-reload-editor` | sender | `+page.svelte:552,639`; `ChatSidebar.svelte` (memory-synthesis-review accept) | `+page.svelte:552,639`, `ChatSidebar.svelte:1782` | **Retained** (ruling 8) — re-wired to fire off the new event instead of the old `pane-note-changed` bridge |
| `pane-reload-editor` | listener | `Editor.svelte` diff-highlight reload | `Editor.svelte:1835` (`handleReload`) | **Retained**, unchanged logic — still the only content-diff-aware reload path |
| `pane-reload-editor` | backend emit | consolidator | `ai/consolidator.rs:203` | **Retained** — unaffected; still emits the Tauri-side signal `+page.svelte`'s listener turns into the DOM dispatch |
| `pane-note-changed` (Tauri) | emitter | `move_note`, `move_folder` | `notes.rs:3264,3289` | Migrated (§34.1.1) |
| `pane-note-changed` (Tauri) | emitter | tool-loop (allowlist-gated) | `ai::chat::run_agent_loop` | Replaced entirely (§34.5/§34.7) |
| `pane-note-changed` (Tauri) | emitter | daily-log, memory synthesis | `ai::chat`, `ai::scheduler` | Migrated (§34.1.1) |
| `pane-note-changed` (Tauri) | listener | `AllTasksView.svelte` | `AllTasksView.svelte:157` (`listen(...) => loadData(true)`) | Replaced — targeted patch, §34.5 |
| `pane-note-changed` (Tauri) | listener | `NotesBrowser.svelte` | `NotesBrowser.svelte:1206` (`listen(...) => loadProjectData()`) | Replaced — targeted patch, §34.5 |
| `pane-note-changed` (Tauri) | listener | `Sidebar.svelte` | `Sidebar.svelte:105` (`listen(...) => loadData()`) | Migrated — folded into the same single-event listener as `pane-project-changed` below |
| `pane-note-changed` (Tauri) | listener | `+page.svelte` dual-purpose handler | `+page.svelte:590-639` | Replaced — the delete-clears-editor check becomes a direct `entity_id === $activeNote.meta.id && reason === 'deleted'` check; the unrelated "auto-favorite new projects" logic (piggybacking on this listener today) is untangled onto its own project-`created` branch of the same new listener, not left riding a note-changed handler |
| `pane-note-changed` (Tauri) | listener | `KanbanBoard.svelte` | **none exists today** | New — this is the literal gap ruling 1/item 46 names; added as the patch-in-place handler, §34.5 |
| `pane-project-changed` (Tauri) | emitter | `create_project` | `projects.rs:217` | Migrated (§34.1.1) |
| `pane-project-changed` (Tauri) | emitter | `ProjectSettings.svelte` rename | `ProjectSettings.svelte:218` | **Correction, Stage 3 (#2972 item 5):** this row originally called for migrating this emit into an app-side DOM dispatch. By the time Stage 3 landed, the backend `update_project_settings` command (`ai::connections`) — invoked by this same save handler, immediately before this emit — already emitted the single event unconditionally itself (Stage 1, §34.1.1). Converting this call to an app-side dispatch would have produced two events for one save. **Deleted outright instead**, not migrated — the backend is this mutation's sole sender. |
| `pane-project-changed` (Tauri) | emitter | project settings update | `update_project_settings` (`ai::connections`) | Migrated (§34.1.1) |
| `pane-project-changed` (Tauri) | listener | `Sidebar.svelte` | `Sidebar.svelte:108` | Migrated — folded into the one listener above |
| `pane-refresh-notes` | listener | `NoteList.svelte` | `NoteList.svelte:44` | Deleted (§34.6) |
| `pane-refresh-notes` | sender | **none found** | — | N/A — confirms the scope doc's finding that this event has zero senders anywhere in `src/` |

**Note on `ProjectSettings.svelte:218`, corrected at Stage 3:** this section originally proposed converting this frontend-initiated Tauri `emit()` into a same-process DOM dispatch, per §34.1's two-channel design. That proposal did not account for the fact that this emit's save handler calls `invoke('update_project_settings', ...)` immediately before it — and that backend command, once migrated in Stage 1 (§34.1.1), already emits the single data-changed event unconditionally on every call. Converting the frontend emit to a dispatch rather than deleting it would therefore have produced two events per save, not one. **Stage 3's actual change: this emit call is deleted outright**, not converted — the backend emit is this mutation's sole sender, and §34.1's "one event per change" ruling is satisfied by zero app-side sends here, not by picking a cheaper app-side send.

---

### 34.3 Migration table — current usage counts (verified against `main`)

Counts are **files referencing the event name**, across both `src/` and `src-tauri/src/` (Rust emitters + tests count where they reference the literal event-name string). Re-counted directly for this spec via `grep -rl` against `main` (#3011) — all seven match Matt's own count exactly:

| Event | Files referencing | Verified count |
|---|---|---|
| `pane-data-changed` | 16 | ✅ matches |
| `pane-note-created` | 9 | ✅ matches |
| `pane-note-changed` | 6 | ✅ matches |
| `pane-project-changed` | 4 | ✅ matches |
| `pane-task-meta-changed` | 4 | ✅ matches |
| `pane-reload-editor` | 5 | ✅ matches |
| `pane-refresh-notes` | 1 | ✅ matches |

**Disposition after this issue lands:**
- **Deleted outright:** `pane-data-changed` the *name* survives (reused for the new event, §34.1) but every pre-migration sender/listener of it is rewritten, not left dual-running. `pane-note-created`, `pane-note-changed`, `pane-project-changed` are deleted as event names — every site in §34.2 tagged "Migrated" moves onto the single `pane-data-changed` name with the new payload. `pane-task-meta-changed` is deleted (ruling 4). `pane-refresh-notes` is deleted (§34.6).
- **Retained as distinct names:** `pane-reload-editor` only (ruling 8).
- **Net result:** seven event names in play today → two (`pane-data-changed`, `pane-reload-editor`) after all four stages land.

---

### 34.4 Echo suppression — precise, not broad

**Mechanism:** every save carries an **origin tag** — a value unique to the save call that produced it (e.g., a per-Editor-instance or per-save-call UUID/monotonic token; the exact token scheme is an implementation detail for the code PR, not specified here, since nothing in Matt's ruling constrains the token's shape, only its behavior). The origin tag travels in the event payload (`origin_tag`, §34.1) on every emit, both senders.

**The rule (ruling 3, stated precisely):**
- A listener that **is the thing that just saved** (the same editor instance, the same component call) and sees an event **carrying its own origin tag** → **skips** the update. It already has the authoritative post-save state locally; re-applying the broadcast would be a redundant round-trip, not new information.
- A listener that **is not** the origin of the save — including the same note open in a *different* window/tab/editor instance, a collaborative edit, or an agent/background edit to the currently-open note — **always updates**, regardless of whose tag it is, as long as the `entity_id` matches what it's displaying.

**Why this must not be broader:** a tag check scoped to "this note ID" (rather than "this exact save call") would suppress a second, independent edit to the *same* note arriving moments later — exactly the failure mode Matt's ruling calls out explicitly: *"suppression that's too broad means collaborative/agent edits silently don't appear, which is worse than a redundant reload."* The origin tag must be scoped to the **save call**, not the **entity** — two different saves to the same note, even from the same editor instance moments apart, are two different origin tags, and an incoming event for the *first* save's tag arriving after the *second* save has already started editing must not suppress the second save's own eventual echo-check incorrectly. (This is a sequencing detail for the implementation — e.g. "latest outstanding tag per note, not a static per-instance constant" — flagged here as a requirement the code PR must satisfy, not resolved further in this spec.)

**Required test shape (§34.10, from the scope doc §7, carried forward unchanged):** echo suppression and cross-origin refresh must be tested **together**, in the same test — a test proving only "doesn't refresh on its own save" without also proving "still refreshes on an independent save to the same note" would pass a broken implementation that simply stopped listening.

**Editor's existing partial guard, for context (not a replacement):** `Editor.svelte:1895`'s `handleDataChanged` already checks `get(activeNote)?.meta?.id !== noteId` and no-ops on mismatch (guards #2237/#2240) — this guards against *resurrecting a closed note*, not against *echoing your own save*. It is unrelated to origin-tag suppression and stays in place unchanged; origin-tag suppression is a new, additional check layered on top of it, not a replacement for it.

---

### 34.5 Listener behavior per screen — patch, insert, or remove; never reload

**The board (ruling 5's literal example):** `KanbanBoard.svelte`'s new handler, replacing `loadTasks` at `KanbanBoard.svelte:224`, branches on the incoming `entity_id`/`entity_type`/`reason`:
- Note already present in the local `tasks` array → **patch** only that entry's changed fields, reusing exactly the field-projection shape `handleTaskMetaChanged` (`KanbanBoard.svelte:149`) already builds (title/status/priority/due_date/preview) — refetched via a single-entity read (`api.readNote`), not a full `loadTasks()`.
- Note not yet present (newly created, or moved into this board's project/status from elsewhere) → **insert**.
- Note no longer belongs on this board (moved out, deleted) → **remove**.

This is a strict superset of what `handleTaskMetaChanged` does today (patch-only, never insert/remove) — the donor pattern is reused for the patch branch; insert/remove are new.

**Every other screen, same discipline — one handler, branch on `reason`, never a full reload:**
- **Editor (note)** — `Editor.svelte:1895`'s `handleDataChanged`: on a matching `entity_id` with a foreign `origin_tag` (§34.4), refetch and patch metadata only (title/status fields) — content reload stays `pane-reload-editor`'s job (ruling 8), unchanged division of responsibility.
- **Editor (sheet)** — same `Editor.svelte` instance when the open note is a sheet; metadata patch identical to the note case. Cell/grid content is `SheetGrid.svelte`'s concern, next bullet.
- **Sheet grid** — `SheetGrid.svelte` has **zero** refresh listeners today (confirmed, §34.0/§34.6 scope doc finding re-verified: no `pane-data-changed`/`pane-note-changed`/`listen(` hits in the file at `main`). Gains a listener for the single event, filtered to its own `note_id`; on a match, re-reads the sheet's grid data and patches in place (cell/row/column diff, not a full component remount) — this closes the "open sheet never updates while an agent edits it" gap named in both the scope doc and the #2970 `mutating_tools` comment (§34.0 row 3) as the reason the stopgap "is NOT a live-UI-refresh fix."
- **Sidebar** — folds the `pane-data-changed`, `pane-note-created`, old `pane-note-changed`, and old `pane-project-changed` listeners (`Sidebar.svelte:97,102,105,108`) into one handler on the new event; on `entity_type: "folder"` with `reason: "created"`, inserts the new folder into the tree in place (closing item 50b's literal repro — see §34.1.4) rather than calling `loadData()` wholesale. A rename/edit patches the existing node's name/color in place.
- **Calendar view** — `CalendarView.svelte` has **zero** listeners today (confirmed, re-verified at `main`: `onMount` registers no `pane-*` listener). Gains a listener scoped to notes with a due date shown on the calendar; on a match, patches that note's calendar card in place (same discipline as the board).
- **All-tasks view** — `AllTasksView.svelte:175`'s `dataHandler` (`loadData(true)`) and `:157`'s Tauri `pane-note-changed` listener both collapse into the one new-event handler, reusing the existing `handleTaskMetaChanged` (`:149`-equivalent in this file) patch shape already proven for field-level updates, extended to insert/remove exactly as the board is.
- **Notes browser** — `NotesBrowser.svelte:668`'s `dataHandler` (`loadProjectData()`) and `:1206`'s Tauri listener both collapse into one handler; patches the affected row in the cabinet/folder list in place.
- **Chat/AI sidebar (`ChatSidebar.svelte`)** — confirmed (re-verified at `main`) to have **no dependency today** on `pane-data-changed`/`pane-note-changed` of any kind; its only involvement with this event family is as a `pane-reload-editor` **sender** (`ChatSidebar.svelte:1782`, memory-synthesis-review accept), which is retained unchanged (§34.2). No new listener is added here — there is nothing for it to patch; it is included in the nine-screen verification list (§34.6) because it is a sender touched by this migration, not because it gains new listening behavior. Its own, unrelated `ChatSidebar.echoSuppression.test.ts` (transcript-ingest gating, #2410) is a naming collision with this section's "echo suppression" and must not be conflated with §34.4's origin-tag mechanism in any test file this issue adds.

---

### 34.6 Dead code — confirmed dead, then deleted

Ruling 6's condition — "once confirmed dead" — is a process, not an assumption. Confirmation method and result, both re-run against `main` for this spec:

**`NoteList.svelte`:**
- **Confirmation method:** search every `.svelte` file for an actual instantiation (`<NoteList`), as distinct from an import.
- **Result at `main`:** exactly one file imports it (`CanvasRenderer.svelte:19`, `import NoteList from './NoteList.svelte';`), and that file contains **zero** `<NoteList` instantiations (`grep -n "<NoteList" src/lib/components/CanvasRenderer.svelte` → no matches). No other file references it at all. **Confirmed dead** — imported, never rendered, per this workspace's UI-reachability-gate convention (`SCOPE-TEMPLATE.md`): a stubbed/never-invoked surface is dead code, not a live defect.
- **Action (done — Stage 2, #3015):** `NoteList.svelte` was deleted outright, along with its one dead import in `CanvasRenderer.svelte:19`. Its `pane-data-changed`/`pane-note-created`/`pane-refresh-notes` listeners (`NoteList.svelte:44,45,46`) were deleted with the file — not migrated to the new event, because there was no live surface for them to update. **Re-verified at Stage 4 (#2972): `NoteList.svelte` does not exist anywhere in `src/` (confirmed via `find`/`grep`, zero hits) — this is no longer a pending action.** (§34.6/§34.11 wording fix, #2972 stage 4: this bullet previously read as an imperative/future instruction, as if the deletion were still outstanding; it was completed in Stage 2 and is corrected here to say so.)

**`pane-refresh-notes`:**
- **Confirmation method:** search all of `src/` for any sender (`dispatchEvent` or equivalent) of this exact event name.
- **Result at `main`:** zero senders found anywhere in `src/` (re-verified for this spec; matches the scope doc's finding). The only reference anywhere was `NoteList.svelte`'s own `addEventListener`/`removeEventListener` pair (`:44,49`), which no longer exists.
- **Action (done — Stage 2, #3015):** deleted as a consequence of `NoteList.svelte`'s deletion — no separate migration was needed, since nothing else listened for or sent it. **Re-verified at Stage 4: zero `pane-refresh-notes` references anywhere in `src/`.**

---

### 34.7 The `mutating_tools` allowlist — deletion, and the current sheet-tool list

**Ruling 7 (authoritative):** the allowlist is deleted. The tool loop's single emit collects the IDs each tool touched, including every sheet tool, the agent's `create_folder`, and the sidebar's New Folder.

**What exists today at `main` (`ai::chat::run_agent_loop`'s spawned task — a local `let` binding, not a module-level const):**

```rust
let mutating_tools = [
    "create_note", "edit_note", "append_to_note", "set_status", "set_priority",
    "create_subtask", "delete_note", "create_folder", "create_project",
    "set_sheet_cells", "add_sheet_row", "add_sheet_column", "delete_sheet_column",
    "add_sheet_rows", "add_sheet_columns",
];
let has_mutations = tool_calls.iter().any(|tc| mutating_tools.contains(&tc.name.as_str()));
if has_mutations {
    let _ = app_handle.emit("pane-note-changed", serde_json::json!({
        "tools": tool_calls.iter().map(|tc| tc.name.clone()).collect::<Vec<_>>(),
        "results": tool_results_log,
    }));
}
```

Fifteen entries, gated on tool *name*, payload still carries no entity ID (§34.0 row 3/4). `create_sheet` is absent even from this list.

**Current full sheet-tool enumeration** (do not reuse the scope doc's five/six-tool list — re-enumerated here from `src-tauri/src/ai/tools.rs`'s `ToolDef` entries and dispatch (both live in `tools.rs` post-#3097), read directly for this spec):

| Tool | `ToolDef` at (`tools.rs`) | Dispatch fn at (`tools.rs`) | `note_id` available at execution? | Mutating? |
|---|---|---|---|---|
| `create_sheet` | `:524` | `execute_create_sheet`, `:4248` | Returns the newly-created note's ID (`sheet_data.note_meta.id`) | Yes |
| `get_sheet_data` | `:553` | `execute_get_sheet_data`, `:4423` | `note_id` param | **No — read-only, excluded from the emit requirement** |
| `set_sheet_cells` | `:565` | `execute_set_sheet_cells`, `:4496` | `note_id` param | Yes |
| `add_sheet_row` | `:592` | `execute_add_sheet_row`, `:4597` | `note_id` param | Yes |
| `add_sheet_column` | `:605` | `execute_add_sheet_column`, `:4650` | `note_id` param | Yes |
| `add_sheet_rows` | `:623` | `execute_add_sheet_rows`, `:4742` | `note_id` param | Yes |
| `add_sheet_columns` | `:648` | `execute_add_sheet_columns`, `:4878` | `note_id` param | Yes |
| `delete_sheet_column` | `:676` | `execute_delete_sheet_column`, `:4936` | `note_id` param | Yes |

Eight tools total, seven mutating, one read-only — matching the backend's own `test_all_eight_sheet_tool_names_present_exactly_once` and `test_get_pane_tools_returns_33_entries` tests (`tools.rs:865,856`). **Confirmed at `main`: none of the seven mutating sheet-tool functions contain an `emit` call today** (checked directly, function body by function body) — the *only* path by which a sheet mutation reaches the frontend at all is the allowlist-gated loop-level emit above, and even that carries no entity ID.

**What replaces it (ruling 7, mechanism):** the allowlist array and the `has_mutations`/`.contains()` gate are deleted outright — not extended with `create_sheet`, not kept as a secondary filter. The loop-level emit becomes **unconditional**: it fires once per turn whenever the collected tool-results set is non-empty, with the payload built from the **IDs each executed tool actually returned/touched**, not from matching tool names against a list. Each `execute_*` function's own return value (already containing `note_id`/`column_id`/created-folder-name per the table above and §3's return-value survey) is the source of the entity ID placed in the emitted event — this is why no sheet tool needs its own direct `emit()` call (§3's option (a), rejected here exactly as the scope doc recommended): one change at the existing loop-level chokepoint, not eight new call sites.

This also closes, as a side effect, the `create_sheet`-missing-from-allowlist gap (§34.0 row 3) and the agent's `create_folder` tool's existing gap (today gated correctly but carrying no folder ID in its payload, per the scope doc's §3 finding, re-confirmed: `execute_create_folder` returns `Ok(format!("Created folder '{}' in project", name))` — a human-readable string, not a structured ID the emit could read today; the implementation PR must change this return shape, or thread the `id` variable already computed in `execute_create_folder` (`ai::tools`) through a side channel, for the unconditional emit to have an entity ID to report. Noted here as a concrete requirement on the Stage 1 implementation, not resolved in this spec).

**The test (no longer present at `main` — see note below):**

`test_mutating_tools_contains_all_six_sheet_tools` — referenced by name in a `#2972`-era comment but not found as a live test at the #3097 base commit (already removed, independent of this split; flagged for Matt, not re-added here) — source-scanned `commands.rs` as text when it existed for the literal `let mutating_tools = [...]` binding (built via `format!` specifically so the test's own source text doesn't self-match, per its comment) and asserts six named sheet-tool strings appear inside that block. **This test is deleted, not updated, when the allowlist is deleted** — once `mutating_tools` no longer exists as a binding, the anchor string `content.find(&anchor_needle)` searches for (`"let mutating_tools = ["`) will not exist in the file, and `.expect("mutating_tools binding must exist")` will panic at the `.expect()`, not fail an assertion. This is `cargo test` failing to compile-then-run cleanly, not a red assertion — the implementation PR must remove this test in the same commit that removes the binding, not leave it to fail at runtime. No replacement source-scan test is needed for the new unconditional-emit code, because (per ruling 7) there is no longer a name-based allowlist for a source-scan test to assert membership in — the new design's correctness is tested by asserting the event actually fires with the right entity ID per tool (§34.10, backend emit contract tests), not by scanning for a string in the array literal.

---

### 34.7A Agent note-tool emit correctness — resolved IDs, and three tools with no emit at all (#2874 item 5 / lane 1 PR 4)

Two gaps found by the 2026-10-16 live-test audit (`docs/audits/2026-10-16-scope-1060-test-failures.md` item 5), after Stage 1 (§34.7) had already landed — Stage 1's own correctness tests proved each tool's emit fires with SOME id; they did not prove that id was the right one, and did not cover these three tools at all.

**Defect 1 — the emitted id must be the RESOLVED id, not the raw param.** `edit_note`, `append_to_note`, `set_status`, `set_priority`, and `delete_note` all accept a `note_id` tool parameter that may be a real UUID **or a title** — `resolve_note_id` (the one function every one of these tools' executors calls before acting) explicitly supports both, and resolving by title is a normal, expected agent calling convention, not an edge case. §34.7's "built from the IDs each executed tool actually returned/touched" principle was implemented, for these five tools, by reading the call's raw `params["note_id"]` straight through — unresolved. A title-based call therefore emitted an `entity_id` that was the title STRING, which no UUID-keyed listener (every current listener, §34.5) could ever match: the event fired, with no error, and was silently dropped. **Fix:** resolve `note_id` via the exact same `resolve_note_id` call the executor itself makes, before building the emitted tuple — one resolution, reused, not a second copy that could drift from the executor's own. The resolved row's own `note_type` column also decides `entity_type` (`sheet` if `note_type = 'sheet'`, `note` otherwise) — not a per-tool guess, since all five of these tools can legally target either a plain note or a sheet (both are rows in `notes`, and share the `status`/`priority` columns these tools write).

**Defect 2 — three tools emitted nothing at all.** `move_note`, `move_folder`, and `star_project` (the AI agent's own tool-dispatch versions, `ai::tools` — distinct from the human Sidebar's `move_note`/`move_folder` Tauri commands in `notes.rs`, which §34.1.1 already covers) were never members of the old `mutating_tools` allowlist (§34.7) and were never added to §34.7's unconditional-emit replacement either — a gap in this spec's own enumeration, not (only) an implementation gap. **Fix:** each now resolves its own target the same way its own executor does (`resolve_note_id` for `move_note`; a new shared `resolve_folder_id_by_id_or_name` for `move_folder`, now also used by `execute_move_folder` itself; the existing shared `resolve_project_id` for `star_project`, which replaces that executor's own ad hoc, unvalidated UUID-shape heuristic) and emits: `move_note` → `{entity_type: note|sheet (from the row), reason: "moved"}`; `move_folder` → `{entity_type: "folder", reason: "moved"}`; `star_project` → `{entity_type: "project", reason: "edited"}` (starring/unstarring edits the project's own `is_favorite` flag — §34.1's four reasons have no "starred" value, and this is squarely an edit of an existing row, not a create/move/delete — no fifth `reason` value added, per §34.1's own ruling).

**Required test (Matt's ruling, the actual deliverable of this item):** a test that walks every tool marked `ToolAccess::Write` or `ToolAccess::HomeOnlyWrite` in the tool registry itself (`tools::write_tool_names()` ∪ `tools::home_only_tool_names()`, #3038/#3036's existing marks — not a hand-typed list of tool names, which #2995 already ruled proves nothing) and fails if a successful call for any of them emits no event. This is a general regression guard for §34's whole contract, not specific to the two defects above — it is what would have caught `move_note`/`move_folder`/`star_project`'s total absence automatically, without needing a human audit to notice the gap in this spec's own table.

---

### 34.8 Cross-references — verified at `main`

- **`sheets.md` §33.11 "AI tools"** (`sheets.md:585-596`) — resolves. Current text (`sheets.md:589`): *"AI-driven mutations flow through the same write chokepoint as user edits: body regeneration, FTS, snapshot throttling all apply identically."* This describes write parity, not notify parity — true for the write path, silently not extended to the refresh path (the exact gap this issue closes). §33.11's six-tool sentence (`sheets.md:591`, *"The agent has six sheet tools..."*) is **stale** (§34.0 row 2 — it's eight now) and needs correcting in the eventual implementation PR alongside the new refresh-emit language; not changed here since this PR is scoped to #2972's own spec, and §33.11 already carries its own pending-merge status line (`sheets.md:596`, `Status: PENDING — items 35, 40 / #2874. I merge, then Stia.`) for unrelated items 35/40 — the tool-count fix should land with whichever PR touches that status line next, named here so it isn't lost.
- **`toolbar.md:143`** — resolves exactly, unchanged. Current text: *"4. Dispatches `pane-data-changed` — refreshes note lists"* (under "5. Move to Trash", `Editor.svelte`'s `handleMoveToTrash()`). This is the `Editor.svelte:480` dispatch site named as a duplicate-removal candidate in §34.1.3/§34.2 — `toolbar.md:143`'s step 4 needs a one-line update in the implementation PR once that duplicate dispatch is actually removed, from "Dispatches `pane-data-changed`" to something reflecting that `api.moveToTrash()` (which `handleMoveToTrash()` already calls, step 2) is now the sole source of that dispatch. Flagged here, not changed here (code-following-spec, not this PR).

---

### 34.9 Status marking

This section (§34) **started** as **TARGET — NOT BUILT (#2972, tracked on #2874)** — spec ahead of code, matching the convention used by `ai-teams.md` (§25) and `sheets.md` (§33) for pending sections: a `> **Status:**` line directly under the heading, naming the tracking issue. The event name `pane-data-changed` and the `pane-reload-editor` event both already existed in their **pre-migration** form before this issue (§34.3) — that pre-migration form was never what this spec documents; §34.1's payload shape was, and remains, the target.

**Partial-stage convention (added at Stage 2, adopted from `ai-integration.md` §4.5.9's precedent):** §4.5.9 keeps its status line terse and single-line — `BUILT - #2874 item 60` — naming only the tracking issue, not restating the whole feature's build history in the status line itself (the body below it carries the detail). §34 had no convention yet for a section that is partway through a multi-stage build-out; this adopts the same shape, scaled to "partway," rather than inventing a new one:

- While **no** stage has landed: `TARGET — NOT BUILT (#2972, tracked on #2874)` (§34.11's original form, unchanged).
- While **some but not all** stages have landed: `IN PROGRESS — N of 4 stages (#latest-landed-PR, this PR)` — `N` counts stages merged-or-in-this-PR against §34.11's four-stage plan; the parenthetical names the most recently landed stage's PR number and, when the status line is itself being updated by the PR landing the next stage, "this PR" for that one. This PR (Stage 2) is the first to use the convention: `IN PROGRESS — 2 of 4 stages (#3014, this PR)` — #3014 is Stage 1's PR; this PR is Stage 2.
- Once **all four** stages have landed: `BUILT (#2972, tracked on #2874)` — the next PR to touch this section updates the line one final time, same as any other section's TARGET→BUILT flip elsewhere in this spec file.

No subsection below carries a different status than this top-level line — every behavior described in §34.1–§34.8/§34.11/§34.12 is the same target the remaining stages build toward; the top-level line tracks build *progress*, not a per-subsection split.

---

### 34.10 Tests — acceptance criteria per stage (scope doc §7, carried forward)

These are the scope doc's own §7 test list, unchanged in substance, mapped onto this spec's four-stage plan (§34.11) as each stage's acceptance criteria:

**Stage 1 (backend emit) acceptance criteria:**
- For each of the seven mutating sheet tools (§34.7's table), the explicit `create_folder` tool, and the human Sidebar `create_folder` Tauri command: assert the single event fires exactly once per mutating call, with the correct `entity_id`/`entity_type`/`reason`.
- Negative case: `get_sheet_data` (read-only) does **not** fire the event.
- The deleted `resolve_folder_id_or_create` silent-auto-create path (§34.0 row 1) needs no test here — it no longer exists to test.

**Stage 2 (listeners, all nine screens in one PR per §34.11) acceptance criteria:**
- **Patch-in-place (KanbanBoard):** dispatching the event for a note already in `tasks` patches only that card's DOM node (same node identity, no full-list re-render) — mirrors `KanbanBoard.editor-survives-reload.test.ts`'s existing node-identity assertion, but via patch, not "survives despite reload." Insert and remove branches tested separately.
- **Sheet-tool-to-grid (SheetGrid):** an open `SheetGrid` updates (at minimum, does not silently show stale data) when the event fires for its own `note_id`. No prior coverage exists for this file (confirmed zero `pane-data-changed`/`pane-note-changed` hits in `SheetGrid.svelte` or any `SheetGrid.*.test.ts` at `main`).
- **Folder-creation-visibility (Sidebar):** create a folder via each of the two remaining live paths (explicit `create_folder` tool, human Sidebar path — the third, silent-auto-create path no longer exists per §34.0 row 1) and assert it appears in `$projects`-derived tree without manual reload/restart.
- **Calendar:** opening a sheet from the calendar after an agent creates it in a new folder shows the correct folder, matching the sidebar (no duplicate "General"-named folder).
- **Payload-shape migration:** every existing test constructing the *old* payload shape must be updated, not silently left red or silently deleted — enumerate explicitly in the implementation PR: `KanbanBoard.sheet-delete-refresh.test.ts`, `KanbanBoard.editor-survives-reload.test.ts`, `Editor.attachment-onload-dirty.test.ts`, `Editor.mount-no-dirty.test.ts`, `api.saveSheetTitle.test.ts`.
- **`test_mutating_tools_contains_all_six_sheet_tools`** is deleted, not updated (§34.7) — already gone at the #3097 base commit, per the flag above.
- **Sibling-parity anchor:** the new KanbanBoard patch-in-place handler ships with a regression test that references **#2972** and this spec (**§34**), so a future AGENTS.md §8a sibling-parity check on this code has something to find.

**Stage 3 (senders + echo suppression) acceptance criteria:**
- **Echo suppression, both halves in one test (§34.4):** open a note, save from that editor instance, assert no re-fetch/re-render from its own resulting event — **and**, in the same test, a second independent event for the same note ID (simulating an agent/external edit) **does** still trigger a refresh.
- Re-run every Stage 2 per-screen test against the new sender shape to confirm no regression.
- **Identical shape, both senders (§34.1):** a test fires an equivalent mutation through **both** senders — the backend emit and the app `CustomEvent` — and asserts the two payloads are **structurally identical: same keys, same types**.

**Stage 4 (verification across 9 screens) acceptance criteria:**
- Item 46's original repro: agent creates/edits a sheet, board updates with no navigate-away-and-back.
- Item 50b's original repro: agent-created sheet's folder appears in the sidebar without restart.
- `NoteList.svelte`/`pane-refresh-notes` deletion (§34.6) does not regress anything — no test currently exercises either (confirmed: no test file references `NoteList` or `pane-refresh-notes`), so this is a compile/build-clean check, not a new test.
- Full pass across all nine screens enumerated in §34.6/§34.12.

---

### 34.11 Stage plan — order and Matt's one-PR-per-stage ruling

**Order (ruling 10):**

1. **Backend emit.** §34.1.1 + §34.7 — the unconditional, entity-identified, allowlist-free emit; all backend senders migrated to the new payload shape.
2. **All listeners, in one PR.** Every screen in §34.12 converted together (§34.6/§34.11 wording fix, #2972 stage 4: this previously cross-referenced §34.6, the dead-code-deletion section, as if it were a second screens enumeration — §34.12 alone is the screens list; §34.6 covers `NoteList.svelte`/`pane-refresh-notes`, both deleted in Stage 2, not a listener target). **Matt's standing ruling on the refresh point (from the scope doc's own §5, restated as binding here): "no separate point fixes" — Stage 2 does not ship KanbanBoard's patch-in-place while NotesBrowser/AllTasksView/Sidebar/SheetGrid/CalendarView still full-reload on the same event; that reintroduces the exact inconsistency this issue exists to fix.** One PR, all nine screens, even though it is the largest diff of the four stages.
3. **Senders and echo suppression.** §34.1.2/34.1.3 (app senders) + §34.4 (origin-tag echo suppression). Depends on Stage 2 already existing — senders emitting the new shape into listeners that don't understand it yet (if Stage 3 shipped before Stage 2) would be worse than today, per the scope doc's own sequencing note.
4. **Verification across the nine screens.** §34.10's Stage 4 criteria — primarily verification plus any sheet-specific special-casing Stage 2/3 didn't already cover, and confirms items 46/50b's original repros are closed end-to-end.

**The 7-vs-9 discrepancy, accounted for:**

Ruling 5 names seven screens ("the board, editor, sheet grid, sidebar, calendar, all-tasks view and notes browser"). Ruling 10 requires verification "across the 9 screens." The two extra, enumerated:

1. **The editor is one named screen in ruling 5 but splits into two for verification** — "Editor (note)" and "Editor (sheet)" are the same component (`Editor.svelte`) but materially different post-change behaviors (§34.5): note metadata patch vs. sheet metadata patch, with the sheet's *content* patch delegated to the separate SheetGrid listener. Counting them as one screen in the ruling's prose and two in the verification table is consistent, not contradictory — ruling 5 is naming *components*, ruling 10's verification is checking *distinct behaviors*.
2. **`ChatSidebar.svelte` is the second addition**, not named in ruling 5 at all. It earns a verification row not because it gains new listening behavior (§34.5's last bullet — it has none) but because it is a **sender** this migration touches (`pane-reload-editor` dispatch on memory-synthesis-review accept, `ChatSidebar.svelte:1782`) and because the scope doc explicitly flagged its superficially-similar-named `ChatSidebar.echoSuppression.test.ts` as a naming-collision risk against this issue's real echo-suppression mechanism (§34.4) — a screen worth a verification pass specifically to confirm that collision doesn't produce a false-positive "it's covered" assumption.

Both accounted for; no unexplained gap between 7 and 9.

---

### 34.12 The screens, enumerated

> **Updated at Stage 4 (#2972):** this was originally nine (the set §34.11 ruling 10 named for verification). Stage 4's own full inventory sweep (PR body has the complete table, including every "loads on open"/dead finding) found four more live screens that showed note/sheet/folder/project/category data without a listener, not named by ruling 5/10 and not closed by Stages 1–3 — each closed in this stage. **It is no longer nine; it is thirteen.** The count is screens with an active `pane-data-changed` listener (or, for #9, a sender with no listener needed) — "loads on open" surfaces (modals/pickers: `TaskEditModal`, `QuickSwitcher`, `FolderPicker`, context menus) and confirmed-dead/unreachable surfaces (`TaskList.svelte`'s `task_list` view, `CanvasRenderer`'s `search_results`/`dashboard` views, `TaskFullPage.svelte`) are deliberately excluded — see the PR body's inventory table for why each of those is excluded, not omitted by oversight.

1. **Kanban board** — `KanbanBoard.svelte`
2. **Editor panel (note)** — `Editor.svelte`, note-type behavior
3. **Editor panel (sheet)** — `Editor.svelte`, sheet-type behavior (metadata only; content via #4)
4. **Sheet grid** — `SheetGrid.svelte`
5. **Sidebar (folder/project tree)** — `Sidebar.svelte`
6. **Calendar view** — `CalendarView.svelte`
7. **All-tasks view** — `AllTasksView.svelte`
8. **Notes browser (cabinet/folder view)** — `NotesBrowser.svelte`
9. **Chat/AI sidebar** — `ChatSidebar.svelte` (sender-only; see §34.11 point 2)
10. **Projects view** — `ProjectsView.svelte` (`category`+`project` listener added #3018/#3019; `note`/`sheet`/`folder` note-count-refresh listener added #2972 stage 4 — #3019 explicitly left this gap "for a future issue" with none opened; this stage is that issue)
11. **Trash** — `Trash.svelte` (new at stage 4: `note`/`deleted` reloads the trash list)
12. **Settings (Categories tab)** — `Settings.svelte` (new at stage 4: `category` reloads the category list)
13. **Project settings (Category picker)** — `ProjectSettings.svelte` (new at stage 4: `category` reloads the category dropdown; deliberately scoped to the dropdown only, not the form's own dirty-editable fields — see the fix-site comment)

---

*This section (§34) is the canonical spec for the data-change refresh point. It supersedes the ad hoc multi-event mechanism described piecemeal across `sheets.md` §33.11, `toolbar.md`, and the scattered `pane-*` event sites enumerated in §34.2, for everything postdating #2972's implementation. The scope doc (`~/Shared/ExampleApp/docs/research/2026-09-24-scope-refresh-point.md`) remains the design-rationale record for *why* each decision in this spec was made; this file is the contract for *what* ships.*
