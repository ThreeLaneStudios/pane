# UI Layout & Context System

**Last Updated:** 2026-04-19  
**Source:** Extracted from pane-architecture-spec.md  
**Sections:** §3

---

## 3. AI Context System
> **Status:** BUILT (v11.28.24) — spatial context, project/folder/note scoping

The AI receives different context depending on where the user is in the app. Context is assembled on every message send.

### 3.1 Context by View
> **Status:** BUILT

| View | Context Loaded | Token Budget |
|------|---------------|--------------|
| **Home (workspace)** | Workspace memory + all project summaries + dashboard state (habits, streaks, tasks) | ~2,500 |
| **Project view** | Project memory + workspace memory + project note/task list + folder tree | ~2,000 |
| **Folder view** | Same as project + current folder highlighted + folder's notes listed | ~2,500 |
| **Note view** | All of the above + full note content (or truncated if huge) | ~4,000 |

**Total context budget per request: ~8,000 tokens** (configurable in config.json).

### 3.2 Context Assembly Order
> **Status:** BUILT

The Context Assembler builds the prompt in this priority order. When budget is exceeded, lower-priority layers are trimmed first:

1. **System prompt** (~500 tokens) — personality, capabilities, formatting rules. NEVER trimmed.
2. **AI memory — personal + work** (~1,000 tokens) — extracted decisions, preferences. NEVER trimmed.
3. **Active note content** (~2,000 tokens) — full content or heading-structure + first/last 500 tokens if huge.
4. **Conversation history** (~3,000 tokens) — recent messages. When conversation exceeds budget, backend compaction (§16.4) generates summaries and flags old messages as excluded. Context includes: compaction summary + recent messages.
5. **Project structure** (~500 tokens) — folder tree + note titles. Collapse to top-level if over budget.
6. **Sibling context** (~500 tokens) — summaries of other notes in same folder. Most aggressively trimmed.
7. **Dashboard state** (~800 tokens) — only in Home view. Habits, streaks, tasks, reflection scores.

### 3.3 Context Anchor Bar
> **Status:** BUILT

A persistent UI bar at the top of the chat panel showing what the AI currently sees:

```
┌─────────────────────────────────────────┐
│ 🧠 Context: Backend / Auth / auth-flow  │
│ 📁 12 notes · 3 tasks · Memory: 8 facts │
└─────────────────────────────────────────┘
```

- Updates automatically on navigation (200ms fade transition)
- Clicking navigates back to the referenced note/folder
- Shows breadcrumb when nested
- Expandable to show full context breakdown (which memory entries, which notes loaded)

### 3.4 Context Permeability
> **Status:** BUILT

Even in project-scoped mode, the AI maintains a lightweight **project index** (~200 tokens): all project names + one-line descriptions. This allows:

- Cross-project references when the user asks ("How does this relate to my Music project?")
- The AI can request deep context from another project via tool use (`get_project_summary`)
- Workspace memory is always loaded regardless of view

### 3.5 Context Shift Markers
> **Status:** BUILT

When the user navigates, a visual divider is inserted into the chat stream:

```
── now viewing: Design Spec ──
```

Stored in `ai_context_markers` table. Clickable to navigate. Preserves the conversation's spatial context for replay.

### 3.6 Navigation Context
> **Status:** BUILT (v0.67.59, PR #1771)

`navigateTo()` resolves `projectName` and `folderName` from the projects store before setting canvas state. All views that read `$canvasState.context.projectName` (Kanban, CanvasRenderer breadcrumb) receive the project name atomically with the navigation update. Card breadcrumbs render `project › folder` consistently across kanban, folder columns, and all-tasks views.

---

## 3.2 Folder View Layout
> **Status:** BUILT — PR A (scaffold), PR B (note overlay), and PR C (collapse rails) all merged (#2180, #2252)

Supersedes the 2026-04-16 CSS grid decision. The folder view uses a **fixed-column flex layout** where Cabinet and Tasks never resize or reflow, and notes open as a resizable `position: absolute` overlay panel instead of squeezing the columns.

### CSS Custom Properties

| Variable | Value | Description |
|----------|-------|-------------|
| `--fv-col` | `300px` | Fixed width of each expanded column (Cabinet, Tasks) |
| `--fv-rail` | `46px` | Width of a collapsed column's slim rail |

### Layout Structure

```
fv-work (flex column)
  ├─ grid-folder-header         │ project title + New Note btn │ flex-shrink: 0, h=48px
  └─ fv-stage (flex row)         │ position: relative, flex: 1, overflow: hidden
       ├─ grid-folder-content     │ flex-shrink: 0; display: flex flex-col; width from children
       │    ├─ grid-folder-toolbar │ Cabinet | Tasks header row  │ sticky, h=48px
       │    └─ folder-columns      │ display: flex row
       │         ├─ Cabinet column  │ width: var(--fv-col) = 300px, flex-shrink: 0
       │         └─ Tasks column    │ width: var(--fv-col) = 300px, flex-shrink: 0
       ├─ fv-void                  │ flex: 1 — empty state or note overlay anchor
       └─ fv-note (overlay, PR B)  │ position: absolute; top:0; bottom:0; left:{n}px; right:0
```

**Note on `grid-folder-content` wrapper:** The two columns are wrapped in `.grid-folder-content` which holds the shared toolbar row and the scrollable column pair. The wrapper has **no explicit width** — it sizes from its children. When PR C collapses a column from `var(--fv-col)` to `var(--fv-rail)`, the wrapper shrinks automatically. No wrapper rework is required for PR C.

### Column Widths

- **Cabinet column**: `width: var(--fv-col); flex-shrink: 0` — never resizes, never reflows.
- **Tasks column**: `width: var(--fv-col); flex-shrink: 0` — same discipline.
- `--fv-col` and `--fv-rail` are declared on `.fv-stage` — single source of truth for all column-width references.
- Combined fixed footprint: 600px (2 × `--fv-col`). The right remainder is the note overlay or empty void.
- Narrowing the window does not auto-collapse columns; it triggers horizontal scroll. Auto-collapse is never triggered — collapse is an explicit, per-column user action.

### Note Panel (Overlay)
> **Status:** BUILT (#2182 + #2180 PR B, `feat/2180-folder-view-note-overlay-pr-b`)

- `position: absolute; top: 0; bottom: 0; left: {noteLeft}px; right: 0` inside `fv-stage`.
- Columns underneath stay **fully lit** — no scrim, no dimming.
- Resize: left-edge drag handle (`fv-note__resize`). Same mouse logic as the kanban editor panel (`startKanbanEditorResize` at `NotesBrowser.svelte` ~line 296–306), with delta sign matching left-edge drag.
- **Width constraint (clamp pattern adopted from kanban editor panel):** `noteLeft` must satisfy `stageWidth - noteLeft ≥ 350px` and `stageWidth - noteLeft ≤ 800px`. Equivalent to `Math.max(350, Math.min(800, panelWidth))`. No hardcoded panel width anywhere — all sizing is clamped at runtime.
- Panel width persists to `localStorage` key `pane-fv-note-width` (global, not per-folder).
- Dismiss: **✕** button in note header, **Esc** key, or click on any empty space outside the panel (column background or stage void). Clicking a different card closes the current note and opens the new one.
- **Delete-close invariant:** When a note is deleted while the panel is open, `goBackToCards()` must cancel any in-flight `selectNote()` call (via `cancelPendingSelection()`) before clearing panel state. Without this, an async selection can resolve after close and reopen the panel. (Bug fix: #2228, regression from #2224 `selectSeq` counter.)
- No backdrop/scrim — columns remain interactive and visible behind the panel.

### Column Collapse (Rails)
> **Status:** BUILT (#2252, `feat/2252-collapse-rails`)

- Each column collapses independently to a `46px` rail (`.fv-railcol`) showing: expand chevron, icon, vertical label (`writing-mode: vertical-rl; text-orientation: mixed`, uppercase, 12px/600), and a count badge pill.
- Collapse toggled by the **collapse button** (`.fv-collapse`) in the column header (`.column-header-left`/`.column-header-right`). The header itself also narrows to `var(--fv-rail)` when its column is collapsed — required explicitly via a `.collapsed` class override, since the header row's `flex: 1` 50/50 split does not auto-narrow on its own.
- Rail click (anywhere on `.fv-railcol`) or the expand chevron inside it expands the column back.
- Four valid combinations, all independently reachable and covered by tests: both open, Cabinet only collapsed, Tasks only collapsed, both collapsed.
- When both columns are collapsed and a note panel is open: `noteLeft` is recomputed to `92px` (2 × 46px) — not a reactive side-effect of note-open, but an explicit assignment fired only when a column's collapse state actually toggles while `fvNoteOpen` is true, so the user's manually-resized panel width (`pane-fv-note-width`) is never clobbered by an ordinary note-open with no collapse change. When both columns are collapsed with no note open, the note-open case simply doesn't apply until a note is selected.
- Width transition: `transition: width 0.15s ease` on `.folder-column` (and mirrored on the header cell) via class toggle.
- Implementation: `NotesBrowser.svelte` — no separate `FolderView.svelte` component exists; the rail markup, collapse state (`cabinetCollapsed`/`tasksCollapsed` `$state` booleans), and persistence effects live inline alongside the rest of the folder-view markup (~line 858 onward).
- **Not the same pattern as** the kanban board's `doneCollapsed` (`NotesBrowser.svelte`, unrelated "Done" column collapse): that pattern is ephemeral (`$state` only, resets every mount, never persisted) and renders a 32px icon-only stub, not the icon+label+count rail. The two are visually and behaviorally distinct; do not conflate them.

### Persistence (SQLite, per-folder)

Supersedes the prior stub in this section, which specified `localStorage` keys (`pane-fv-cabinet-collapsed`, `pane-fv-tasks-collapsed`) and explicitly global (not per-folder) scope. Per Matt's ruling: collapse state must be (1) stored in SQLite, not `localStorage`, and (2) scoped per-folder, not global. The stub predated implementation and was never built as specified — this section documents the actual built behavior.

- **Table:** the existing generic `ai_settings` KV table (`key TEXT PRIMARY KEY, value TEXT`) — no new table, no new migration. `ai_settings` is **not** in `CRR_TABLES` (`src-tauri/src/migrations.rs`), so it is per-device by construction and never syncs across devices.
- **Commands:** the existing generic `get_ai_setting`/`set_ai_setting` Tauri commands (`src-tauri/src/notes.rs`, backed by `src-tauri/src/database.rs`) — zero new Rust commands, zero new registrations.
- **Key format:** `folder:{scope_id}:cabinet:collapsed` / `folder:{scope_id}:tasks:collapsed`, where `scope_id` is `$activeFolder.id` when a folder is selected, or `$activeProject.id` when viewing a project's root-level notes with no folder selected (so root-level collapse state still persists rather than being silently dropped).
- **Values:** literal strings `'true'` / `'false'` (same convention as `onboarding_completed` and other existing `ai_settings` boolean flags — not JSON-wrapped).
- **Default:** a missing key (folder/scope never visited before) reads back as `null`/`None` from `get_ai_setting`, which the frontend treats as expanded (`false`). No insert-on-folder-create is needed.
- **Load:** both keys are read via `invoke('get_ai_setting', ...)` in a `$effect` keyed off `$activeFolder`/`$activeProject` changes, alongside the existing `loadTaskNotes()` reload effect.
- **Save:** written immediately on toggle click via `invoke('set_ai_setting', ...)` — a discrete click, not a drag, so no debounce is needed (contrast with `pane-fv-note-width`'s drag-resize, which persists on `mouseup`).
- **Cleanup on folder deletion:** `delete_folder`'s Rust handler (`src-tauri/src/projects.rs`) runs `DELETE FROM ai_settings WHERE key LIKE 'folder:' || ?1 || ':%:collapsed'` scoped to the deleted folder's exact id before removing the `projects` row, so collapse-state rows don't orphan permanently. Covered by `test_delete_folder_cleans_up_ai_settings_collapse_rows` (verifies a sibling folder's rows are untouched).

`pane-fv-note-width` (the note overlay panel width, §“Note Panel (Overlay)” above) is unaffected by this change — it remains a global `localStorage` key, since panel width is a layout preference independent of which folder is open, unlike column collapse which the user may reasonably want to differ per-folder.

### Sidebar Interaction

The app sidebar expands 40px → 220px on hover (CSS `transition: width`) which squeezes `main-content` (flex: 1, min-width: 0). The `fv-stage` lives inside `main-content` and shrinks accordingly:

- Fixed columns stay at their exact pixel width — they do not reflow.
- The note overlay's `right: 0` inset tracks the stage's right edge automatically — the overlay narrows as the stage narrows.
- `noteLeft` (the overlay's left anchor) is an absolute pixel value from the stage left edge — it does not change when the sidebar expands.

### Context Menu Safety

`overflow: hidden` on `fv-stage` does **not** clip `position: fixed` context menus (`.context-overlay`, `.context-menu`). `position: fixed` is viewport-relative and bypasses overflow clipping entirely. No containing-block transforms are applied to `fv-stage` or any ancestor, so the behavior is reliable.

---

## 3.3 Kanban Board Layout
> **Status:** BUILT (#2874 item F/Kanban collapse, PR "Calendar opens the board with the card, shared ConfirmDialog, every Kanban column collapsible")

This section did not exist before this PR — neither the Calendar→board navigation target nor the Kanban column-collapse behavior/persistence had any spec coverage (confirmed: no hit for `handleTaskClick`/`KanbanBoard` column-collapse in any `docs/spec/*.md` prior to this change). Written here per Matt's rulings below, not inferred.

### Calendar → Board Navigation

- Clicking a task on `CalendarView` navigates to that task's project **board** (`view: 'kanban'`) with the clicked task's **card open** — not `view: 'notes_browser'` (the full per-project note/file browser). Matt's ruling: "opening a task from the Calendar opens that project's board with the task's card open" — both halves required, not just landing on the right view.
- Implementation: `CanvasRenderer.svelte`'s `handleTaskClick` calls `navigateTo({ view: 'kanban', projectId: task.project_id, noteId: task.id })`, then (after `tick()` lets the `{#if currentCanvasState.view === 'kanban'}` branch mount) dispatches the existing `window` `pane-select-note` CustomEvent `{ detail: { noteId } }` — the same event `NotesBrowser.svelte` and `ChatSidebar.svelte` already dispatch/listen for elsewhere; no new dispatch path was added. `KanbanBoard.svelte` listens for it and opens the matching task's card inline (`handleNoteSelect`, no separate navigation), queuing the id if `tasks` hasn't finished loading yet (same completion-signal pattern as `NotesBrowser.svelte`'s `pendingSelectNoteId`).
- This is the same `view: 'kanban'` target `Sidebar.svelte`'s `selectProject` already uses for a project-root click — Calendar now lands on the identical "project view," not a separate one.

### Column Collapse

- **Every column is independently collapsible** — not gated to the Done column or to columns with more than 5 cards (the pre-#2874 behavior). Click anywhere on a column's header to toggle; the collapse indicator (`▶`/`▼`) always renders.
- **Done defaults collapsed** for a project with no persisted collapse state yet (first-ever visit); every other column defaults expanded.
- **Persistence (SQLite, per-project, per-column)** — reuses the exact mechanism §"Column Collapse (Rails)" → "Persistence (SQLite, per-folder)" above already documents and `NotesBrowser.svelte` already implements for its cabinet/tasks rails: the generic `ai_settings` KV table via the existing `get_ai_setting`/`set_ai_setting` Tauri commands (`src-tauri/src/notes.rs`) — zero new Rust commands, zero new migrations. `ai_settings` is not in `CRR_TABLES`, so this is deliberately per-device, never synced — correct for a local UI preference, consistent with the folder-view precedent. A prior, unrelated `localStorage`-based draft of this persistence was rejected in favor of this SQLite mechanism before landing, for the same reasons Matt's folder-view ruling above gives.
  - **Key format:** `kanban:{projectId}:{status}:collapsed`, one key per `TaskStatus` (`cabinet`, `todo`, `in_progress`, `review`, `done`). Values are the literal strings `'true'`/`'false'`.
  - **Default:** a missing key reads back `null` from `get_ai_setting`; the frontend maps that to `done` → collapsed, everything else → expanded (not a blanket "expanded" default like the folder-view rails, since Matt's ruling 3 specifically keeps Done collapsed by default).
  - **Load:** all five keys read via `invoke('get_ai_setting', ...)` in a `$effect` keyed off `$canvasState.context.projectId`, guarded by a monotonic sequence counter against a stale resolve from a fast double project-switch (same `collapseLoadSeq` pattern as `NotesBrowser.svelte`).
  - **Save:** written immediately on toggle click via `invoke('set_ai_setting', ...)` — a discrete click, no debounce needed.
  - **Cleanup on project deletion:** deliberately **not** added. Project deletion already hard-deletes the whole project tree; the handful of leftover `kanban:{projectId}:*:collapsed` rows are the same class of harmless, bounded, per-device orphan the folder-view spec section above already calls "pre-existing/acceptable" for its own project-root-scoped rows. Adding cleanup here would be new, untracked scope beyond Matt's ruling.

---

## 3.4 Wiki Links and Note Creation
> **Status:** BUILT (#3044 items 1–3) — supersedes the create-on-miss behavior #2419 originally shipped.

This section did not exist before this PR — the wiki-link create-on-miss behavior and the Cmd+N/New-Note-button unification had no prior spec coverage.

### Missing wiki links are inert

- A `[[Title]]` wiki link to a note that does NOT exist does nothing on click — no create, no confirm dialog. **"Does nothing" describes click-time BEHAVIOR, not rendering:** `Editor.svelte`'s render-pass regex (`[[Title]]` text → a `data-wiki-link` span) and the live `WikiLink.ts` ProseMirror node's `renderHTML` are both unconditional — every `[[Title]]` becomes the identical clickable-looking span regardless of whether a note with that title exists; there is no existence check anywhere in rendering, on either route, and neither was changed by this PR. Both routes instead funnel every click through the same `navigateWikiLink()` (`src/lib/utils/wikiLinkNavigation.ts`), which is the ONE place — and the ONLY place — that decides whether anything happens; a dangling link looks pixel-identical to a live one and gives no visual signal it's broken until clicked. (Corrected per Reed's 2026-10-17 delta1 audit, which found the prior wording's "stays plain" framing could be misread as a rendering change — it isn't one.)
- **Superseded:** the prior behavior (#2419) searched for the title, and on no exact match, asked via `confirmFn` (`window.confirm`) and created the note via `createNoteFn` on confirmation. That entire create/confirm path — and the `projectId`/`folderId`/`createNoteFn`/`confirmFn` parameters it needed — is deleted from `navigateWikiLink`'s signature, not merely gated. An exact-title match still navigates exactly as before (`selectNoteFn` with a `contextOverride` carrying the target note's own `project_id`/`folder_id` — unchanged from #2443's single-writer fix).

### A link opens the note in its OWN project, not whichever project was previously active

- **The bug:** `CanvasRenderer.svelte`'s `loadDataForView` has a dedicated case for every view that scopes the screen to a project (`notes_browser`/`project_detail`, `folder_detail`, `task_list` (pre-#3044 item 5 deletion), `project_config`) — each resolves `context.projectId` into the `$activeProject` store. The `note_editor` case — the view wiki-link navigation (and any other cross-project note_editor navigation) actually lands on — never did. `NotesBrowser.svelte` (the SAME component rendered for `notes_browser`/`note_editor`) reads `$activeProject` directly for its folder header, folder list, and task-note filtering, so a note's CONTENT loaded correctly (`get_note` is id-keyed, not project-scoped) while every surrounding chrome kept showing whichever project was active before the click.
- **The fix:** the `note_editor` case now also resolves `context.projectId`/`context.folderId` into `$activeProject`/`$activeFolder`/`$activeNotebook`, mirroring every other case in the same switch, before loading the note's content.

### Cmd+N creates a note; Cmd+T does not exist
> **Status update (2026-10-18, #3044 revisited):** the `window.__paneNewNote` mechanism this section used to describe is GONE — Reed's 2026-10-17 delta1 audit found it relocated the original race into a silent, untested no-op gap covering most of the app's screens (see below). Superseded by a plain shared function with no global and no "is a component mounted" precondition at all.

- **Only Cmd+N creates a note.** Cmd+T is removed entirely — no fallback, no secondary path.
- **One shared function, no window global:** `noteService.createNewNote()` (`src/lib/services/noteService.ts`) is the ONE place note-creation code lives. Both the New Note button (`NotesBrowser.svelte`'s `handleNewNote`, a thin reentrancy-guarded wrapper) and `+page.svelte`'s global Cmd+N keydown handler call it directly — no `window.__paneNewNote` or any other global exists anymore. The function reads/writes stores only (`$activeProject`, `$activeNote`, `canvasState`), never a component's local state, so it works identically whether or not any particular component happens to be mounted — this is what makes the race structurally impossible rather than relocated a third time.
- **Inside a project** (`$activeProject` set): creates the note in that project's own `default_folder_id` — never a "first non-system folder" scan, never a hardcoded `'AI'`/`'Pane System'` name check (both were the leftover island #3044 already removed once from this exact code path; superseded again here) — then opens it by setting `$activeNote` and navigating `canvasState` to `note_editor`, the same activeNote-then-canvasState sequencing `noteService.selectNote()` already uses. Works from every project-scoped screen, not only `notes_browser`/`note_editor`.
- **Outside a project** (`$activeProject` is null — Home, All Tasks, Calendar, Trash, the project list): creates nothing and dispatches a `pane-new-note-blocked` window event instead; `+page.svelte` shows **"Open a project to make a note."** — a visible result instead of the prior mechanism's silent no-op.
- **Kanban is a separate, pre-existing, unrelated flow, unaffected by any of this:** `KanbanBoard.svelte` has registered its own `window` keydown listener for Cmd+N since #424 (`handleCreateNewTask`, always status `cabinet`, opens inline in the board's own editor panel) — `+page.svelte`'s handler explicitly returns before reaching the shared function when `$canvasState.view === 'kanban'`, both before and after this revision.
- **Root cause of "Cmd+N/Cmd+T don't save title or content" (#3044, original):** `NotesBrowser.svelte`'s `onMount` runs `$activeNote = null;` as its first statement (#2874 item 15 fix #3, guarding against stale carry-over from other views). `+page.svelte`'s own duplicate creation code, when `NotesBrowser` wasn't already mounted, created the note and set `$activeNote` to it, THEN navigated to bring `NotesBrowser` on screen for the first time — which ran the very `onMount` that nulls `$activeNote` right back out, before the user could type anything. **The intermediate fix's own gap (#3044 revisited, Reed's WARN):** routing creation through `window.__paneNewNote` (set by `NotesBrowser`'s `onMount`, cleared by its `onDestroy`) meant Cmd+N became a silent no-op — zero note, zero error — from every screen where `NotesBrowser` isn't currently mounted, which `CanvasRenderer.svelte`'s plain `{:else if}` render chain makes true for every non-Notes top-level view. The current fix removes the global outright rather than relocating the race a third time.

## 3.5 Job-session back navigation (bell/card → one-deep back)
> **Status:** BUILT (#3151 PR1 Commit F). "Nav is the complaint that started this" (#3151's own framing) — a job session (hidden from the sessions drawer by design, §36.11) has exactly two entry points: the Jobs card's "Open session" link and the notification bell. Before this PR, following either stranded the user with no way back to wherever they were.

- **Both entry points set the return target**, not just one — `scheduledPanel.ts`'s `openScheduledJobSession` (the Jobs card) and `NotificationPanel.svelte`'s `handleRowClick` `target_kind === 'session'` case (the bell) both read the chat view's current session BEFORE switching and hand it to `recordJobSessionEntry` (`$lib/services/jobSessionBack.ts`), which sets the one `jobSessionBackTarget` store (`$lib/stores/app.ts`) — skipping the write when there's nothing to go back to, or the "previous" session is the one just entered.
- **One-deep, by construction, not by convention.** The store holds at most one id — there is no stack. `goToJobSessionBackTarget` clears it as its OWN first step, before any navigation, so a SECOND back press (or a second `Cmd+[`) reads `null` and is a plain, safe no-op: nothing happens. If the user then navigates elsewhere and wants to get back to the job session, the card/bell is the way in again, same as before this PR — matching "one-deep is sufficient," the issue's own framing, read literally rather than inventing a navigation stack nobody asked for.
- **Two ways to go back, one shared action.** `Cmd+[` (`+page.svelte`'s global keydown handler) calls `goToJobSessionBackTarget()` directly. The `←` glyph (below) calls the same function.
- **The return trip uses the SAME mechanism as the trip in** — `switchSession` (PR 6's origin-guarded `demote_and_promote`) + `navigateTo`, never a second navigation path invented for the way back.

> **fix(runtime) (#3155, C49b) — the back button moved, and `showBackButton` gained a second, required condition.** Before this fix, `showBackButton` was derived straight from `$jobSessionBackTarget !== null` — "does a back target exist somewhere," never "is the session CURRENTLY on screen actually a job session." `handleSessionSwitch` (`ChatSidebar.svelte`, the ONE session-switch path that bypasses `navigateTo` entirely — the sidebar/drawer manual-switch path) never cleared the target either. Combined, these let the button follow the user onto a totally unrelated session after a manual switch (Matt's repro: it followed him onto Aurum) — the floating `.job-session-back-button` rendered as an absolutely-positioned header sibling, independent of which session was actually open.
>
> **Fixed, two parts:**
> - **Placement:** the floating element is deleted. The `←` glyph now lives at the left edge of the JOB BRIEF strip's own label (`ChatPanel.svelte`'s `.job-brief-label`, the §3.5-adjacent C32 dispatch-brief block) — it only has a strip to attach to inside an actual job-brief-shaped message (`job_id`/`synthetic`/`role === 'user'`), which a job's own dedicated session always has (its own dispatch brief, written before the job's first turn ever runs).
> - **Gate:** `showBackButton`'s derivation is now `shouldShowJobSessionBackButton(backTarget, activeSessionOrigin)` (`$lib/services/jobSessionBack.ts`, a pure function with its own Vitest coverage) — TWO conditions, both required: a non-null back target AND the ACTIVE session's own `origin` column being `'spawn'`/`'schedule'` (§36.11's existing "this is a job's own dedicated session" convention, reused, not reinvented). `activeSessionOrigin` (`$lib/stores/app.ts`) is mirrored alongside `activeChatThreadId` at every site that resolves a session: `navigateTo`'s STEP 3, and `ChatSidebar.svelte`'s `loadOrCreateThread`/`handleSessionSwitch`. `handleSessionSwitch` ALSO now clears `jobSessionBackTarget` as its own first step, unconditionally — both halves of the original bug closed, not just the one that happened to mask the symptom.
> - **Composition with the C32 dispatch-brief rendering:** a wake job's brief lives in its ORIGIN chat by construction (`session_id == origin_session_id`, §36.4.5) — that chat's own `origin` is never `'spawn'`/`'schedule'`, so the SAME gate that scopes the back button also keeps the glyph off of a wake brief sitting in an origin chat, for free. The `.job-brief` chip's own muted-chip rendering (C32, unchanged) still renders there; only the `←` glyph is absent — the two halves compose by sharing one predicate, never by the brief-rendering branch special-casing job `kind`.
> - **Tests:** `shouldShowJobSessionBackButton`'s own suite (`jobSessionBack.test.ts`) proves the origin gate in isolation (mutation: drop the origin check — reddens). `ChatSidebar.jobSessionBackOrigin.test.ts` proves `handleSessionSwitch`'s unconditional clear (mutation: remove the clear call — reddens). `ChatPanel.backButton.test.ts` proves the glyph's new DOM shape, including "absent when `showBackButton` is true but there is no job-brief message to attach it to."

> **fix(runtime) (#3166, C58) — the header's OWN session name was still stale after a bell/card entry, one level deeper than `showBackButton`.** Matt's repro: open a job session via the bell/card while already viewing a project (same project the job session belongs to) — the message body correctly loads the job session's content, but the header kept reading the PROJECT's ordinary interactive session's name ("Aurum · New One") instead of the job session's own name. Root cause: `SessionDropdownHeader.svelte`'s header text is NOT driven by `activeChatThreadId`/`activeSessionOrigin` (both already correct, mirrored by `navigateTo` at every resolution site per C49b above) — it is driven by `sessionStore.activeSession`, which is refreshed ONLY by `SessionDropdownHeader`'s own `$effect(() => loadSessions(projectId, true))`, gated on `projectId` CHANGING. A job session in the SAME project as the one already open never changes `projectId`, so that `$effect` never re-fires and `activeSession` stays pinned on whatever the project's flagged interactive session was — a gap one level deeper than C49b's `showBackButton` fix, which only ever touched `activeSessionOrigin`/`jobSessionBackTarget`, never `sessionStore.activeSession` itself.
>
> **Fixed, two parts:**
> - **The binding:** a new `sessionStore.setActiveSessionFromSwitch(session)` sets `activeSession` directly from a `Session` the caller already has — no second round trip, no re-promote/demote (`switch_session`'s own backend guard, §36.11's "open without promote," already decided that). Both job-session entry points call it right after their own `switch_session` resolves: the bell (`NotificationPanel.svelte`'s `target_kind === 'session'` case) and the Jobs card (`scheduledPanel.ts`'s `openScheduledJobSession`, via a new optional `setActiveSession` dep). **The return trip gets the same fix**, not just the two ways in: `goToJobSessionBackTarget` (`jobSessionBack.ts`) also calls it with whatever session the return-trip `switchSession` resolved — without this, going back would leave `activeSession` pinned on the job session just left, the identical staleness bug mirrored in the other direction.
> - **The persistent cue:** `SessionDropdownHeader.svelte` renders a "Job session · `<name>`" badge next to the session name whenever `isJobSessionOrigin($activeSessionOrigin)` is true (`isJobSessionOrigin`, `jobSessionBack.ts` — the bare origin half `shouldShowJobSessionBackButton` above already used, now extracted so both surfaces read the SAME check, never two independently-written `=== 'spawn' || === 'schedule'` comparisons). Deliberately NOT gated on a back target existing (unlike `showBackButton`) — the cue states what the session IS, true even with nothing to go back to. Lives in the header — always on screen — rather than only inside the JOB BRIEF strip (C32's `.job-brief` block), which scrolls out of view the moment the conversation continues past the dispatch brief itself; Matt's own repro continued the conversation ("Astro replied there") before noticing the stale header, which the strip-only cue would already have scrolled past by then.
> - **Composition with C49b's `←`:** both the cue and the back button derive from `isJobSessionOrigin`/the same `$activeSessionOrigin` read — the cue is a strict widening (drops the back-target condition), never a second, independently-derived origin check that could drift from the `←`'s own gate.
> - **Tests:** `sessionStore.test.ts`'s "setActiveSessionFromSwitch (#3166 C58)" suite (mutation: no-op the method — reddens). `NotificationPanel.jobSessionBack.test.ts` and `scheduledPanel.test.ts` each pin their own entry point calling it with the resolved session (mutation: drop the call at either site — reddens independently at that site only). `jobSessionBack.test.ts`'s new suites pin the return-trip fix and `isJobSessionOrigin` itself, plus a direct agreement check against `shouldShowJobSessionBackButton`'s origin half. `SessionDropdownHeader.test.ts`'s new suite pins the cue rendering for `spawn`/`schedule`, its absence for `user`/`null`, and the shared-predicate identity (mutation: hardcode the cue's condition to `false` — reddens both positive cases).

---

## App Shell Root Sizing (`.app`)
> **Status:** BUILT (#3144, second attempt — supersedes the `100vw/100vh` → `100%` change from #3139/`ad0c074a`, which shipped broken on 1.0.72)

**`.app` (`src/routes/+page.svelte`) is sized `position: fixed; inset: 0`, not `width/height: 100%` and not `100vw/100vh`.**

- **The mechanism is deliberately unconfirmed-proof, not mechanism-confirmed.** #3139's fix moved `.app` off viewport units onto `%` on the theory that `vw`/`vh` diverge from `%` under a zoomed WKWebView — a "strong lead, not a confirmed repro" per that PR's own commit message. The `%` value it shipped broke every session's scroll on 1.0.72 (Home, project, and job sessions alike — the message list grew with its content and the composer sat below the window edge). A second theory — that `app.html`'s `display: contents` wrapper (SvelteKit's own default scaffold, chosen to protect hydration from extension-injected DOM; see sveltejs/kit#6244, #7585, #14357) doesn't pass percentage height through in WKWebView — was tested and also **not confirmed**: a byte-faithful repro of the exact shipped CSS chain passed clean, 6/6 structural variants, in both Playwright's bundled WebKit and Chromium, including the version that is broken in production (`docs/audits/2026-11-02-scope-3144-c27-scroll.md`, §0/§3). No WebKit tracker entry corroborates that mechanism either.
- **`position: fixed`'s containing-block resolution is a third, independent path** — the viewport's own geometry (the initial containing block, since no ancestor of `.app` sets `transform`/`filter`/`perspective`/`contain`/`backdrop-filter`/`will-change`), which shares neither percentage-of-ancestor-box's resolution path nor viewport-units' resolution path. It does not need either discredited theory to be true or false to work.
- **Rejected: `100dvh`/`100dvw`.** Same unit family as `vw`/`vh` — dynamic-toolbar-aware, which is irrelevant on desktop Tauri — so recommending it would reintroduce the original, still-unconfirmed viewport-unit risk under a different name.
- **Rejected: giving `app.html`'s wrapper an explicit height.** That `display: contents` div is SvelteKit's own default, kept specifically to protect hydration from extension-injected DOM (see citations above); changing it fights the framework for a benefit this scope's own testing could not confirm exists.
- **Proven empirically, not argued:** forcing an ancestor box to collapse to zero height — a stand-in for whichever link in the chain actually drops `.app`'s height in production, whatever that turns out to be — reproduces the exact 1.0.72 break under `%` (`.app`'s `clientHeight` goes to 0) and recovers completely under `position: fixed; inset: 0` (`.app`'s `clientHeight` is the full viewport height either way).
- **Precedent already in this codebase:** `.app`'s own true siblings in `+page.svelte` (the sync-failure banner, `UpdateBar`, `UpdatingScreen`'s overlays, `.auth-loading`) are already `position: fixed; inset: 0` (or equivalent `top/right/bottom/left: 0`). This section's own "Context Menu Safety" note above states the same underlying property for a different element: `position: fixed` is viewport-relative and bypasses overflow/containing-block questions entirely, reliably, so long as no ancestor names a containing-block transform — confirmed true of `.app`'s ancestor chain (`html`, `body`, the `display:contents` wrapper). `.app` sets no `z-index` of its own, so this introduces no new stacking context; `.chat-panel`'s `z-index: 1001` (a normal-flow descendant) and the siblings at `z-index: 9999` (`QuickSwitcher`/`Templates`/`.auth-loading`, true siblings of `.app`, not descendants) are unaffected.
- **Verification:** no CI-run Playwright WebKit test can honestly catch a regression here — Playwright's bundled WebKit does not reproduce the symptom (above), and this repo's own `e2e.yml` ("E2E — Playwright (self-hosted)") is `disabled_manually` (Matt, 2026-10-30) regardless. `src/routes/page.c27-app-sizing.test.ts` is a source-text drift guard only — it proves the literal is present, nothing about real layout. The only things that confirm real layout are `e2e/tests/tier1/c27-app-sizing-layout.spec.ts` (local-only, run by hand against a real Tauri build through the real-WKWebView socket bridge) and Matt's own live re-walk of the built app.
- **C27 cleanup-list status:** back to Open, "live repro required before the next attempt" — the specific causal mechanism behind the 1.0.72 break is still unconfirmed; this fix closes the symptom by removing it from the one class of failure (an indefinite-height ancestor, for any reason) that explains it, not by identifying the cause.
- **Correction (#3147, live test #5, third C27 diagnosis):** this section's own fix was never the cause of the composer-scrollbar symptom Matt reported at Cmd+ zoom (the live-test-#5 repro). Both attempts documented in this section (`ad0c074a`/#3139's `%` change, and #3144/#3145's `position: fixed; inset: 0` change, immediately above) target `.app` — the root flex container. The real symptom lives in `.message-input` (`ChatPanel.svelte`'s composer textarea), a completely different element several layers below `.app`, fixed separately below ("Composer Textarea Horizontal Overflow"). Stated plainly so a future reader doesn't re-diagnose this section's own, separately-real scroll fix as a candidate for a bug it never caused.

---

## App Shell Responsive Breakpoints (Sidebar / Chat / Drawer)
> **Status:** BUILT (#3182, C64)

**Three width states, derived from `windowWidth` (`src/routes/+page.svelte`'s "Responsive breakpoints" `$effect`, branching logic extracted to `src/lib/utils/responsiveLayout.ts::deriveResponsiveState`):**

- **wide** (`>= 1200`) — sidebar + main + chat, all three visible side by side.
- **medium** (`< 1200`) — sidebar collapsed (`$sidebarCollapsed = true`); main + chat still shown. Chat is untouched in this band — it has no medium-specific behavior of its own.
- **narrow** (`< 900`) — sidebar collapsed AND chat becomes a **slide-over on top**, via the existing `@media (max-width: 899px)` rule (`+page.svelte`, ~line 921: `.chat-panel { position: fixed; ... }`). This is a CSS-level change, not a mount/unmount — the same `.chat-panel` element (and its children) that sits in flow at wider widths becomes a fixed-position overlay at narrow widths.

**User-chosen open/close state is tracked separately from width-derived state.** `autoCollapsed` (sidebar) and its exact mirror `autoChatCollapsed` (chat) each record ONLY "a breakpoint handler forced this closed," never "the user chose this" — `$chatSidebarOpen` alone used to conflate the two, which was C64's root cause (narrowing closed chat, nothing ever recorded that the close was forced rather than chosen, so widening back past 1200 never reopened it — the sidebar's own `autoCollapsed` had solved this same problem already, just not for chat). A manual open/close (the `Cmd+\` shortcut, the collapsed-sliver's expand button) resets its own `auto*Collapsed` flag at the call site; `deriveResponsiveState` itself only ever sets a flag true (forcing a close) or consumes-and-clears one (restoring on widen) — it never resets one on its own, so a manual choice is never second-guessed by a later resize event in the same width band.

**The sessions drawer always lives inside the chat panel, never in the main column.** `SessionsDrawer` is, structurally, always a DOM child of `.chat-panel` (a sibling of `ChatSidebar`, both inside `.chat-panel.home-view` on Home) — this has never changed and should not change. The empty-column symptom this issue fixed was never `SessionsDrawer` moving; it was `ChatSidebar`'s own render gate (`ChatSidebar.svelte`, `{#if alwaysOpen || $chatSidebarOpen}`) depending entirely on `$chatSidebarOpen`, which the narrow-breakpoint handler was clobbering — so `ChatSidebar` unmounted on Home while `SessionsDrawer` (unconditional on Home) stayed, reading as an abandoned drawer sitting alone in the main column (`.main-content` is hidden on Home, so `.chat-panel.home-view` **is** the visible main content region there). The fix is `ChatSidebar` receiving `alwaysOpen={$canvasState.view === 'home'}` from `+page.svelte` — Home's chat is structurally the main content and should never depend on the same store the breakpoint handler writes. A future PR must not "fix" a reappearance of this symptom by moving `SessionsDrawer`'s markup; the gate is the thing to check.

**The 899/900 JS/CSS boundary agreement is a fact to keep true on future edits to either side, not an enforced invariant.** JS's narrow threshold is `windowWidth < 900` (0–899 inclusive); the one `@media` rule in `+page.svelte` is `(max-width: 899px)`. The two agree exactly today. No shared constant exists tying them together — this is documentation of the current agreement, not a structural guard against future drift.

**Verification:** `src/lib/utils/responsiveLayout.test.ts` (jsdom, pure function — no DOM) proves the state-machine: narrow→medium→wide restores `chatSidebarOpen`, a manual close while wide is never force-reopened, medium never touches chat. **jsdom cannot cover the real on-screen restore at all** — no CSS layout engine, and a jsdom test here would pass against the broken build exactly as it did for C57 (#3178's own audit hit this failure mode once already) — so the actual resize-and-restore sequence, and `SessionsDrawer`'s real DOM parent at each step, is proven instead by `tests/e2e/c64-narrow-layout-restore.spec.ts` (WebKit, mocked IPC, real computed layout; Matt's own live re-walk at 1400→800→1400 is the acceptance test beyond that).

### Sidebar rail: two collapsed states, never three — hover-expand works in both (#3186, C65)
> **Status:** BUILT (#3186, C65)

**There are exactly two collapsed states — narrow and medium — and no third.** Matt's own correction on this scope, stated explicitly because an earlier draft of it over-specified a third: *"I never asked for manual collapse — that was me over-specifying. Cmd+B is gone, nothing sets it, so there are two collapsed states and only two: narrow and medium."* Confirmed in code, not assumed: `grep -rn "sidebarCollapsed = true"` outside `responsiveLayout.ts` returns zero hits; `Sidebar.svelte`'s rail dot-buttons only ever set `$sidebarCollapsed = false`; the Cmd+B shortcut that used to set it manually was removed in #2916. There is no UI path left that writes a manual collapse, so there is no third state to design for. **Do not restore a manual collapse control and do not add a "pending control" placeholder note here** — if manual collapse is wanted later, it is new, unscoped work for its own issue.

**The rail is always visible at every width, narrow included, and hover-expand (`.sidebar:hover { width: 220px }`, the same mechanism "Sidebar Interaction" above already documents) now works in both collapsed states.** Fixed by removing `Sidebar.svelte`'s `.sidebar.collapsed:hover, .sidebar.collapsed.expanded { width: 40px; }` rule, which unconditionally defeated hover-expand any time `collapsed` was set — i.e. at both narrow and medium, exactly when the rail is in its 40px state and hover-expand matters most. That rule dates to `8598e470`, when `collapsed` meant only "the user hit Cmd+B" — a real, deliberate case worth guarding against a stray mouse pass, at the time. The very next commit (`aa7df7c9`) reused the same `$sidebarCollapsed` flag for breakpoint-driven auto-collapse without updating this selector, and #2916 later deleted the only path that could ever set it manually — leaving the rule permanently guarding a state nothing can reach.

**Correction (#3190, C65 final):** the sentence that used to close this paragraph here claimed hover-expand "squeezes `.main-content`/the chat-panel sibling exactly as it already does at wide" — i.e. a push/reflow, the sidebar's box growing and shoving its siblings over. **That was never true and was never verified** — this subsection's own `width`-only assertion (and the deleted spec that proved it) could pass whether the box pushed or overlaid, because neither it nor the test distinguished the two. It does not push: hover-expand paints the wide-state DOM **over** `.main-content`/`.chat-panel` instead of reflowing them, same as it does at wide. See the next subsection for the mechanism and why reflow was never the design.

**Verification:** jsdom cannot cover any of this — no CSS layout engine, no `:hover` resolution. The `tests/e2e/c65-sidebar-rail-hover.spec.ts` width/position assertions that used to be cited here were deleted by #3190 (C65 final) — they asserted exactly the width and `elementFromPoint`-position claims that passed three times while the overlay was visibly broken (menu painting behind content at narrow, labels never rendering at medium). Proven instead by `tests/e2e/c65-sidebar-overlay-visual.spec.ts` (WebKit, mocked IPC, `toHaveScreenshot()` against committed baselines) — see the next subsection.

### Home view at narrow: `.chat-panel` overlay bound (#3186, C65)
> **Status:** BUILT (#3186, C65)

**A second, independent bug previously made the rail's narrow-width visibility unverifiable on Home**, and read, on a live screen, as "the sidebar disappeared" — it had not; `.sidebar` itself was always 40px and visible. `.chat-panel`'s narrow rule (`@media (max-width: 899px)`) sets `position: fixed; top: 0; right: 0; bottom: 0; z-index: 100` with no `left` and no `width` bound. On every non-Home route this is harmless — the template pins an explicit inline `width` (`chatWidth`, from `localStorage`) regardless of width band, so `right: 0` plus a concrete width resolves to a normal, bounded drawer. On Home specifically, the template sets `style:width=""` (empty) for `.chat-panel.home-view`, and with no `left`/`width` bound, a `flex: 1` child inside it (`ChatSidebar`, centered layout) is free to settle on a box wider than the viewport — measured in WebKit at an 800px viewport: `left: -10.6px; width: 810.6px`. That box paints over the entire screen, rail included, in `.chat-panel`'s own `#0a0a0f` background — identical to `.sidebar`'s own background — with no seam, which is why it reads as "vanished" rather than "something is covering it." `.sidebar` sets no `z-index`/stacking context of its own, so it cannot out-rank `.chat-panel`'s explicit `z-index: 100` once `.chat-panel`'s box physically overlaps it.

**Fix:** `.chat-panel.home-view`, inside the same `@media (max-width: 899px)` block, gets `left: 40px` — bounded to the rail's own width so it can never extend over it. Scoped to `.home-view` specifically (not the bare `.chat-panel` selector) so the non-Home drawer, already pinned by its own explicit inline `width`, is untouched. Independent of the hover-expand fix above — different file, different root cause (a missing positional bound vs. a selector unconditionally defeating `:hover`), different blast radius; not squashed into the same commit.

**Verification:** jsdom cannot cover this either — no CSS layout engine. The `document.elementFromPoint` companion assertion that used to be cited here was deleted along with the rest of `tests/e2e/c65-sidebar-rail-hover.spec.ts` by #3190 (C65 final) — `elementFromPoint`/position assertions are one of the two forbidden categories named below. Proven instead by `tests/e2e/c65-sidebar-overlay-visual.spec.ts`'s screenshots and Reed's visual audit of them. Matt's own live re-walk (rail visible at narrow on Home; hover-expand at both narrow and medium) remains the acceptance test beyond either.

### Hover-expand is the wide state, overlaid, not pushed (#3190, C65 final)
> **Status:** BUILT (#3190, C65 final)

**Matt's ruling, verbatim:** *"Reed measured width, and width was never the requirement."* Three prior passes (#3186, #3187, and one before) each asserted a width or a position and passed while the feature was visibly broken on screen: at narrow, the hover-expanded rail rendered **behind** `.chat-panel`'s Jobs column instead of on top of it; at medium, hover widened the rail's box but the project **labels never appeared** — only dots and chevrons, because `.sidebar.collapsed .projects-section { display: none }` kept firing through the hover-expanded width growth. Both bugs are the same root cause: hover-expand was built, and verified, as "the collapsed state, but wider" — a third, unverified state in practice — when it should be, and now is, **the exact same DOM and CSS classes as the sidebar's own ≥1200 (non-collapsed) state**, positioned over the content instead of pushing it.

**Mechanism — one code path, not two that happen to agree.** `.sidebar`'s `.collapsed` class is the single gate that, until this fix, controlled three separate things (enumerated in Reed's scope audit, issue #3190): (1) the CSS rule hiding `.projects-section`, (2) the Svelte `{#if $sidebarCollapsed && groupingActive && categoryGroups.length > 0}` guard that **mounts** `.ps-rail` at all — a template condition, not CSS, because an absent `{#if}` block has no box for a selector to ever reach — and (3) `NotificationPanel`'s bell-popover left-offset math (out of scope here; see below). Hover never flips `$sidebarCollapsed` itself — it is a pure width-band JS boolean, driven only by `responsiveLayout.ts`, and `:hover` is a CSS pseudo-class that cannot and does not write to it. So "hover removes `.collapsed`" is not literal — it is this: every rule gate #1 and #2 needed now carries the **same** `.sidebar:hover`/`.sidebar.expanded` compound-selector escape this file already uses, unconditionally, for the six other label/width reveal rules that predate this scope (`.nav-label`, `.ps-cat__label`, `.category-dot`, `.project-label`, `.folders-container::before`, `.folder-label`) — the identical mechanism that already produces the ≥1200 appearance, extended to the same two gates it had never reached. That is what makes "same DOM as wide, by construction" true: it is the same selector convention, applied to the same element, not a parallel rule set that a reviewer has to diff against the wide-state rules to confirm agreement.

**#2875 still holds — rail and labels never render simultaneously, checked explicitly.** Commit `61cb6d30` (#2875/#2879) added `.sidebar.collapsed .projects-section { display: none }` specifically so the rail (`.ps-rail`, dots and chevrons) and the full label list never render at the same time duplicating every category dot. Fixing gate #1 alone (labels reveal on hover) without also hiding `.ps-rail` on that same hover/expanded condition would reopen exactly that bug — both would be visible at once mid-hover. So both move together, in the same commit: `.ps-rail`'s `{#if}` guard drops `$sidebarCollapsed` from its condition (gate #2 — it is now structurally parallel to `.projects-section`, which is already unconditionally mounted and lets CSS do the hiding), and new CSS hides `.ps-rail` on `.sidebar.collapsed:hover`/`.sidebar.collapsed.expanded` using the same compound-selector convention as every other reveal rule in the file, inverted. The two states — "dots visible, labels hidden" at rest, "labels visible, dots hidden" on hover/expanded — are mutually exclusive by construction (each side of the same pair of dual-selector rules), not by two independently-written rules that happen not to overlap today.

**Stacking — `z-index` on `.sidebar` only, nothing on `.main-content`.** The narrow-width failure (menu behind the Jobs column) is `.chat-panel` (`z-index: 100` at narrow, `1001` at medium/wide — both explicit and positive) out-ranking `.sidebar` (`z-index: auto`) under `.app`'s single stacking context (`position: fixed; inset: 0` on `.app` is what makes it the one relevant context here — not `.main-content`, which this file's own C10/#3120 comment is about, and which is a parent of neither `.chat-panel` nor the sidebar's sibling relationship to it; `.chat-panel` is `.main-content`'s **sibling**, so giving `.main-content` its own stacking context would change nothing about this specific comparison, and would reseal every descendant z-index inside `.main-content` against everything outside it — exactly the blast radius the C10 comment already warns against). Fixed with a scoped rule, `.sidebar:hover, .sidebar.expanded { z-index: 1500; }` — high enough to beat `.chat-panel`'s highest value (1001) and `.main-content`'s own internal maximum (~1000, bubbling up through the same no-stacking-context mechanism), well below the app's top-level overlay tier (9999/10000), and scoped only to the transient hover/expanded window so the sidebar's z-ordering at rest and at wide is unchanged. **Nothing is added to `.main-content`.**

**#3187's `.chat-panel.home-view { left: 40px }` survives unchanged.** It bounds the chat panel's own resting-state box so it cannot physically grow into the rail's space at 40px; this fix's z-index only matters during the transient hover-expanded window, a different sub-problem solved a different way. Not touched by this commit.

**Correction (#3191, final pass on C65): "overlaid, not pushed, same as it does at wide" was true at exactly one of three bands, not three.** Reed's audit (PR #3191, comment timestamped during review) read the six committed baselines by eye and found: at narrow, the Jobs column genuinely did not move (`+ New job` sat at the same x on hover as at rest, its text visibly truncated behind the rail); at medium and wide, it moved right by exactly the rail's growth (40px → 220px) — a real flex reflow, not an overlay. The cause: `.sidebar`'s `z-index: 1500` above was scoped to `.sidebar.collapsed:hover`/`.sidebar.collapsed.expanded`, and `.collapsed` is only ever `true` below 1200px (`responsiveLayout.ts`) — so the rule was **unreachable at wide**, not merely "inert because nothing happened to overlap there." And at every band, `.sidebar` was still a normal, in-flow flex item (`position: relative`, width `var(--sidebar-rail-width, 40px)` unconditionally) — growing it on hover necessarily reflows its flex siblings unless something takes it out of flow. Narrow only *looked* correct because of an unrelated mechanism: #3187's `@media (max-width: 899px) { .chat-panel { position: fixed } }` happens to remove `.chat-panel` from flow below 900px anyway, so `.sidebar`'s own in-flow growth had no in-flow sibling left to push. That mechanism doesn't exist at medium/wide, where `.chat-panel` stays a normal flex sibling.

**Fix:** a new `.sidebar-rail-spacer` element — a plain, always-in-flow, fixed-`var(--sidebar-rail-width, 40px)`-wide flex item rendered immediately before `.sidebar` in the DOM — now occupies `.sidebar`'s old flex-layout role permanently. `.sidebar` itself is `position: absolute; top: 0; left: 0` **unconditionally**, not just during hover, so it is never a flex item for layout purposes at any width band — `.chat-panel`/`.main-content` size against the spacer instead, identically at rest whether or not `.sidebar` is hovered. The `z-index: 1500` rule moved off `.collapsed` entirely, onto the same `.sidebar:hover`/`.sidebar.expanded` selectors that already gate the width growth, so it now applies — and matters — at all three bands, not just below 1200px. Net effect: `.sidebar`'s own rest-state box (40px, absolutely positioned exactly over the spacer) is visually identical to before at every band, including wide, where the rest-state baseline is unchanged pixel-for-pixel; growing to 220px on hover now paints over `.chat-panel`'s unmoved box everywhere, rather than pushing it at two of three bands. "Overlaid, not pushed, same as it does at wide" is now true at all three — confirmed against the regenerated `c65-medium-hover`/`c65-wide-hover` baselines, where `+ New job` no longer shifts and the Jobs column text truncates behind the rail exactly as it already did at narrow.

**Bell-popover math — found, reported, not fixed.** `NotificationPanel.svelte`'s bell popover anchors itself with `left: ${$sidebarCollapsed ? 52 : 224}px`, assuming the sidebar is only ever exactly 40px or exactly 220px, 1:1 with the `$sidebarCollapsed` boolean. This assumption is already defeated today, independent of this scope, by the existing `sidebarExpanded`/`.expanded` path (opening the bell itself sets `panelOpen → sidebarExpanded`, widening the sidebar to 220px while `$sidebarCollapsed` stays `true`) — hover-expand exhibits the identical pre-existing mismatch once the rail genuinely reaches 220px on hover, because `$sidebarCollapsed` never changes value either way (see Mechanism above). Fixing it needs new state threaded into a second component, which is new plumbing, not a CSS/template change — **out of scope for #3190**, reported here for a follow-up issue rather than bundled in.

**Verification:** `toHaveScreenshot()`, not assertions — `tests/e2e/c65-sidebar-overlay-visual.spec.ts` (WebKit, mocked IPC), six committed baselines: narrow/medium/wide × rest/hover. **No width, position, or `elementFromPoint` assertion exists anywhere in this spec or this fix** — the prior spec's two assertion classes are exactly what passed three times while this was broken (see the correction above). Reed's audit is a visual read of the six images (labels legible, menu on top of everything), added to the Visual Smoke Check gate (`.github/workflows/visual-smoke-check.yml`) so a future PR that regresses the overlay fails the gate on pixel diff, not on a green assertion that can't see it.

---

## Composer Textarea Horizontal Overflow (`.message-input`)
> **Status:** BUILT (#3147, C27, live test #5 — third diagnosis, and the first one that targets the right element)

**`.message-input` (`src/lib/components/ChatPanel.svelte`) sets `overflow-x: hidden` explicitly, alongside its existing `overflow-y: auto`.**

- **Root cause, confirmed by direct inspection, not live repro:** the rule set `padding: 10px 14px`, `overflow-y: auto`, and declared **zero** `overflow-x` anywhere in the rule or any `.message-input*` selector (`:focus`, `::placeholder`, `:disabled` — none touch overflow). Per the CSS Overflow Module Level 3 §3 coupling rule: a box with exactly one of `overflow-x`/`overflow-y` set to a value other than `visible` computes the OTHER axis to `auto` too, instead of staying at its own initial value. So `overflow-x` here was silently `auto`, never declared, never reviewed — a real, standard CSS mechanism, not a WebKit-specific quirk.
- **Two prior C27 attempts shipped on unconfirmed theories, both targeting the WRONG element.** `ad0c074a`/#3139 (viewport-units → `%` on `.app`) and #3144/#3145 (`%` → `position: fixed; inset: 0` on `.app`, documented in the section immediately above) both modified `.app`, the root flex container. Neither was ever the cause of the composer's stray horizontal scrollbar — that symptom lives entirely inside `.message-input`, several layers below `.app`, and nothing about `.app`'s own sizing mechanism touches a descendant textarea's `overflow-x` computation. This is the first of the three attempts aimed at the actual element.
- **The fix is mechanism-agnostic, same framing as `.app`'s own fix above:** explicitly setting `overflow-x: hidden` removes the entire CLASS of horizontal-scrollbar-on-this-element failures — any future cause of the box's content exceeding its width — not just the one fractional-rounding instance Matt saw at Cmd+ zoom.
- **UNCONFIRMED MECHANISM, stated plainly, not implied as established.** The zoom-specific trigger (Matt's live report: a stray scrollbar-shaped artifact at Cmd+ zoom) was investigated but not confirmed. Reed tested the specific "fractional padding rounds to ~1px under zoom" theory using two different available proxies for native `WKWebView.pageZoom` (which Playwright has no binding for in either engine, and which has no CDP equivalent):
  1. **CSS `zoom` on `<body>`** (closest conceptual analog to a full-page scale, distinct from `transform: scale`, which doesn't affect layout boxes at all): Chromium showed zero overflow at every tested zoom level (0.8–1.5); Playwright's bundled WebKit showed large overflow (23px–171px, not ~1px) — a real divergence, but the wrong magnitude and an asymmetric growth pattern consistent with a `<textarea>`-specific (replaced-element) CSS-`zoom` sizing quirk, a different known WebKit bug class, not confirmation of the stray-1px-scrollbar theory.
  2. **Chromium's `Emulation.setDeviceMetricsOverride` with a fractional `deviceScaleFactor`** (an analog for "padding rounds to a fractional device pixel"): zero overflow at every scale tested (1.05–1.67) — `clientWidth`/`scrollWidth` are reported in CSS pixels in this engine and track together regardless of device pixel ratio, so this proxy produced no divergence in either direction.
  
  **Honest conclusion: this is the THIRD unconfirmed C27 theory in a row** (after viewport-units and `display: contents`, both in the section above) — neither proxy reproduces a ~1px discrepancy specifically, on a mechanism that is not actually what the native API does. The fix ships because it is mechanism-agnostic, not because the zoom cause is confirmed.
- **Test:** `src/lib/components/ChatPanel.c27MessageInputOverflow.test.ts` — a jsdom source-text pin, same convention as the existing `.app`-sizing pin, explicitly disclaimed in its own header: it proves the literal `overflow-x: hidden` string is present in the rule, nothing about whether a real WKWebView scrollbar is fixed. No mutation can prove the real symptom is fixed — stated rather than invented.

---

## Global Audio Player
> **Status:** PR5a skeleton landed (#401, #1801) — NodeView wiring in PR5b.

- Single global audio player; one active track at a time.
- Lives in the left nav (`Sidebar.svelte` → `NavPlayer.svelte`), NOT inline in notes.
- Persists across all views — the `<audio>` element is owned by the app-shell chrome (Sidebar), which never unmounts on view navigation. This is the structural guarantee behind #1801: audio cannot stop when the user navigates, because the Sidebar is never remounted by `CanvasRenderer.svelte`'s `{#if}/{:else if}` view switching.
- Renders in BOTH menu widths: collapsed (34px play/pause + cyan progress ring, no title) and expanded-on-hover (full transport with cover, title, breadcrumb, scrubber, elapsed/total, go-to-note ↗). Width is CSS-driven via `.sidebar:hover` — no JS state.
- Docked in the menu foot, ABOVE Notifications bell + Settings gear, as a sibling block — never an overlay. Bell + gear remain clickable.
- Empty state: no track loaded → player + divider hidden; menu reads exactly as without the feature.
- State machine: `idle | loading | playing | paused | failed`.
- Store: `src/lib/stores/player.ts` (Svelte writables, `writable<PlayerTrack | null>`, etc.)
- NodeView wiring (`AttachmentNodeView` sets `playerTrack`) is separate — see #401 PR5b.

