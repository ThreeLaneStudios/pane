<script lang="ts">
  // Permanent debug mode: set localStorage.pane-debug=1 to enable verbose logging.
  // Silent by default — no perf cost in production.
  const DEBUG = typeof localStorage !== 'undefined' && localStorage.getItem('pane-debug') === '1';

  import { onMount, onDestroy, untrack } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { activeProject, projects, notes, activeViewTab, activeNote, editorListWidth, cardListCollapsed, activeFolder } from '$lib/stores/app';
  import { canvasState } from '$lib/stores/canvas';
  import * as api from '$lib/api';
  import * as noteService from '$lib/services/noteService';
  import { reorderNotes } from '$lib/services/entityService';
  import {
    ENTITY_NOTE,
    ENTITY_SHEET,
    ENTITY_FOLDER,
    ENTITY_PROJECT,
    REASON_DELETED,
    REASON_EDITED,
    subscribeDataChanged,
    dispatchDataChanged,
    type DataChangedPayload,
  } from '$lib/refreshEvent';
  import { resolveDropIndex, normalizeInsertIndex, cardRectsFromElements, shouldShowIndicator } from '$lib/utils/dragReorderIndex';
  import type { Project, Folder, NoteMeta, TaskStatus, TaskPriority } from '$lib/types';
  import SlideOver from './SlideOver.svelte';
  import Editor from './Editor.svelte';
  import Card from './Card.svelte';
  import { normalizeNoteMeta } from '$lib/utils/cardNormalizers';
  import ConfirmDialog from './ConfirmDialog.svelte';
  import { navigateWikiLink } from '$lib/utils/wikiLinkNavigation';
  import { resolveHeaderButtonMode, type HeaderButtonMode } from '$lib/utils/newSheetButtonLayout';
  // ChatSidebar is rendered globally in +page.svelte — don't duplicate here

  let folders: Folder[] = $state([]);
  let taskNotes: NoteMeta[] = $state([]);
  // #2874 item 18 fix 1: holds a noteId from a pane-select-note dispatch
  // that arrived before taskNotes had finished loading (loadTaskNotes()
  // still in flight). Resolved by the $effect below once taskNotes
  // transitions from empty to populated — replaces CanvasRenderer's old
  // fixed 200ms setTimeout guess with an actual completion signal.
  let pendingSelectNoteId: string | null = $state(null);
  let selectedFolder: string | null = $state(null);
  let projectFolders: Project[] = $state([]);
  // #2339 PR 15a: card-face summaries (col/row/formula counts + col types)
  // for whichever visible taskNotes are sheet notes, keyed by note id. Same
  // pattern as KanbanBoard.svelte's sheetSummaries/cardSummaryFor — one
  // batched fetch per loadTaskNotes() pass.
  let sheetSummaries: Map<string, import('$lib/types').SheetCardSummary> = $state(new Map());

  function cardSummaryFor(note: NoteMeta) {
    if (note.note_type !== 'sheet') return undefined;
    const s = sheetSummaries.get(note.id);
    if (!s) return undefined;
    return { colCount: s.col_count, rowCount: s.row_count, formulaCount: s.formula_count, colTypes: s.col_types };
  }

  // #2334 canonical cabinet array — the ONLY cabinet array in this file.
  // Filtered (cabinet-only) AND sorted by task_sort_order in one reactive
  // derivation, so every consumer (DOM rendering, sourceIdx lookup in
  // dragover, sourceIdx lookup in drop, shouldShowIndicator, reorder
  // dispatch) sees the same order. Previously this file had TWO divergent
  // arrays — an unsorted `cabinetNotes` used for index math and a sorted
  // `cabinetNotesLive` used for rendering — which fell out of sync after an
  // optimistic reorder (task_sort_order updated on taskNotes, but the
  // unsorted filter's array order never changed), causing sourceIdx to be
  // computed against stale positions and the drop indicator to suppress the
  // wrong slots. KanbanBoard's getTasksForColumn('cabinet') never had this
  // bug because it re-filters AND re-sorts from one source on every call —
  // that's the reference pattern this derivation follows.
  let cabinetNotes = $derived(
    taskNotes
      .filter(n => !n.task_status || n.task_status === 'cabinet')
      .sort((a, b) => (a.task_sort_order ?? 0) - (b.task_sort_order ?? 0) || (new Date(b.created_at).getTime() - new Date(a.created_at).getTime()))
  );



  let showSlideOver = $state(false);
  let selectedNote: NoteMeta | null = $state(null);
  let selectedProjectNote: NoteMeta | null = $state(null);
  
  // Unified context menu state machine
  type ContextMenuState = 
    | { phase: 'actions', x: number, y: number, note: NoteMeta }
    | { phase: 'projects', x: number, y: number, note: NoteMeta, projects: Project[] }
    | { phase: 'folders', x: number, y: number, note: NoteMeta, projectId: string, projectName: string, folders: Project[] }
    | null;
  
  let contextMenuState: ContextMenuState = $state(null);
  let deleteConfirmNote: NoteMeta | null = $state(null);
  
  // listWidth uses editorListWidth store
  let isResizing = $state(false);
  
  // Card list panel width (split pane)
  let cardListWidth = $state(parseInt(localStorage.getItem('pane-card-list-width') || '340'));
  let isCardListResizing = $state(false);

  // Folder panel width state removed (#2180): replaced by fv-stage fixed columns
  
  // Persist card list collapsed state
  $effect(() => {
    localStorage.setItem('pane-card-list-collapsed', String($cardListCollapsed));
  });
  
  function startCardListResize(e: MouseEvent) {
    e.preventDefault();
    isCardListResizing = true;
    const startX = e.clientX;
    const startWidth = cardListWidth;
    function onMouseMove(ev: MouseEvent) {
      cardListWidth = Math.max(180, Math.min(450, startWidth + (ev.clientX - startX)));
    }
    function onMouseUp() {
      isCardListResizing = false;
      localStorage.setItem('pane-card-list-width', String(cardListWidth));
      document.removeEventListener('mousemove', onMouseMove);
      document.removeEventListener('mouseup', onMouseUp);
      document.body.style.cursor = '';
      document.body.style.userSelect = '';
    }
    document.addEventListener('mousemove', onMouseMove);
    document.addEventListener('mouseup', onMouseUp);
    document.body.style.cursor = 'col-resize';
    document.body.style.userSelect = 'none';
  }
  
  // Folder-view note overlay state (#2180 PR B)
  // noteLeft = distance (px) from fv-stage left edge to the panel's left edge.
  // Persisted globally; default 300 (Cabinet visible, panel covers Tasks + void).

  // Shared panel width constraints — used by both folder-view and kanban resize logic.
  // One source of truth; do not repeat these literals elsewhere.
  // #2336: NOTE_PANEL_MAX_WIDTH (formerly a hardcoded 800px ceiling) is gone — the panel
  // may now grow to fill the entire stage width (i.e. noteLeft floors at 0, flush against
  // the sidebar edge). The max is now `stageWidth` itself, computed live at each clamp
  // site below rather than a constant, since it's inherently window-size-dependent.
  const NOTE_PANEL_MIN_WIDTH = 350;

  // Folder-view column collapse rails (#2252).
  // JS-side mirrors of the CSS vars declared on .fv-stage (--fv-col: 300px, --fv-rail: 46px) —
  // needed here because noteLeft's boundary-anchor recompute (below) is a JS $state number,
  // not CSS. Colocated with NOTE_PANEL_MIN_WIDTH/MAX_WIDTH per the same "one source of truth,
  // don't repeat literals" rule. Do not hardcode 300/46 anywhere else in this file.
  const FV_COL = 300;
  const FV_RAIL = 46;

  // Per-folder (or per-project-root, when no folder is selected) collapse state.
  // Loaded from ai_settings on $activeFolder/$activeProject change (effect below),
  // written immediately on toggle via set_ai_setting. Default: both expanded.
  let cabinetCollapsed = $state(false);
  let tasksCollapsed = $state(false);

  let noteLeft = $state(parseInt(localStorage.getItem('pane-fv-note-width') || '300'));
  let fvStageEl: HTMLDivElement | undefined = $state(undefined);

  // #2339 PR9 ruling 2: sheet overlay width persists under its OWN
  // localStorage key (`pane-fv-note-width`'s key/semantics above are
  // untouched), defaulting to 1000px (README ruling 4). Unlike noteLeft
  // (which stores the panel's *offset from the stage's left edge*),
  // fvSheetWidth stores the panel's actual *width* directly — NotesBrowser's
  // overlay is position-derived, so the sheet panel's left offset is computed
  // on the fly from `stageWidth - fvSheetWidth` (see fvSheetLeftPx() below)
  // rather than persisted as an offset like noteLeft is.
  let fvSheetWidth = $state(parseInt(localStorage.getItem('pane-fv-sheet-width') || '1000'));
  let isActiveSheetNote = $derived($activeNote?.meta?.note_type === 'sheet');

  // Computes the sheet panel's `left` (distance from stage left edge),
  // mirroring noteLeft's clamp logic but derived from fvSheetWidth on every
  // call rather than cached in $state — called directly from the template
  // so it's always current without needing its own resize-observer/effect.
  function fvSheetLeftPx(): number {
    const stageWidth = fvStageEl?.offsetWidth ?? (fvSheetWidth + NOTE_PANEL_MIN_WIDTH);
    const clamped = Math.max(NOTE_PANEL_MIN_WIDTH, Math.min(stageWidth, fvSheetWidth));
    return stageWidth - clamped;
  }

  // #2331: guards the click-outside handlers (.fv-cards-panel onclick, .fv-void onclick;
  // #2329 re-scoped these off the old shared .fv-stage handler) against the synthetic click
  // that the browser fires on the mousedown/mouseup common ancestor when a resize-drag
  // starting inside .fv-note ends with the pointer outside it. True for the duration of a
  // resize gesture, including the one event-loop tick after mouseup where the synthetic
  // click actually arrives (see the setTimeout in the mouseup handler below).
  let isFvNoteResizing = $state(false);

  // #2192: Decouples panel DOM presence from $activeNote nullability.
  // Goes true when any note loads (via effect below). Goes false ONLY on explicit dismiss
  // (✕ button, Esc, click-outside). This keeps .fv-note mounted during the mandatory null
  // gap in noteService.selectNote, preventing the column flash that was visible in #2192.
  let fvNoteOpen = $state(false);

  // Sync fvNoteOpen → true whenever $activeNote transitions from null to non-null.
  // Do NOT set it in every card click handler — that's fragile. One reactive source.
  //
  // CRITICAL — two rules that must both hold (#2228 latch fix):
  //   1. Do NOT read fvNoteOpen inside this effect body (even in debug logs) — Svelte 5
  //      tracks every reactive read inside an $effect, so reading fvNoteOpen here creates
  //      a dependency that wakes the effect when goBackToCards() sets fvNoteOpen=false,
  //      which then sees $activeNote still non-null and re-asserts fvNoteOpen=true (loop).
  //      Use untrack() to read fvNoteOpen in debug logs without adding the dependency.
  //   2. goBackToCards() must null $activeNote BEFORE setting fvNoteOpen=false (Option B)
  //      so the effect sees null and skips the fvNoteOpen=true branch even if it fires.
  $effect(() => {
    if ($activeNote !== null) {
      if (DEBUG) console.log('[PANE-DEBUG] fvNoteOpen = true (was', untrack(() => fvNoteOpen), ') via $activeNote effect, noteId=', $activeNote.meta?.id);
      fvNoteOpen = true;
    }
  });

  // Clamp noteLeft on mount so a persisted value from a wider monitor doesn't produce a
  // broken-sliver or over-wide panel on first render. Fires once fvStageEl is bound.
  // (The drag clamp in startFvNoteResize handles subsequent resizes.)
  $effect(() => {
    if (!fvStageEl) return;
    const stageWidth = fvStageEl.offsetWidth;
    const panelWidth = stageWidth - noteLeft;
    // #2336: max is stageWidth itself (panel can fill the whole stage, noteLeft floors at 0).
    const clamped = Math.max(NOTE_PANEL_MIN_WIDTH, Math.min(stageWidth, panelWidth));
    noteLeft = stageWidth - clamped;
  });

  // #2252: noteLeft anchor recompute is triggered from the collapse TOGGLE handlers
  // (toggleCabinetCollapsed / toggleTasksCollapsed below), not from a reactive $effect
  // here. A reactive effect keyed off collapse state would also fire on ordinary note-open
  // (fvNoteOpen false→true) and clobber the user's persisted/resized noteLeft every time a
  // note is opened while nothing changed — that would break the existing PR B "panel width
  // persists independently" behavior. Instead: when a column's collapse state actually
  // toggles while a note is open, the panel's left edge must move directly to the new
  // column boundary (stale noteLeft would otherwise land mid-column, not at the seam).
  function fvColumnBoundary(): number {
    return (cabinetCollapsed ? FV_RAIL : FV_COL) + (tasksCollapsed ? FV_RAIL : FV_COL);
  }

  // Escape key to go back from editor to cards
  $effect(() => {
    function handleKeydown(e: KeyboardEvent) {
      if ((e.metaKey || e.ctrlKey) && e.shiftKey && e.key === "B") {
        e.preventDefault();
        $cardListCollapsed = !$cardListCollapsed;
        return;
      }
      if (e.key === 'Escape' && $activeNote) {
        e.preventDefault();
        goBackToCards();
      }

    }
    window.addEventListener('keydown', handleKeydown);
    return () => window.removeEventListener('keydown', handleKeydown);
  });
  

  

  
  function goBackToCards() {
    if (DEBUG) console.log('[PANE-DEBUG] goBackToCards() entry', { fvNoteOpen, activeNoteId: $activeNote?.meta?.id, selectedProjectNoteId: selectedProjectNote?.id });
    // Cancel any pending selectNote async so it cannot restore $activeNote and
    // trigger fvNoteOpen=true AFTER this intentional close (#2174 regression fix).
    noteService.cancelPendingSelection();
    // #2228 latch fix (Option B): null $activeNote FIRST, THEN fvNoteOpen.
    // The $activeNote→fvNoteOpen $effect checks `if ($activeNote !== null)` before
    // setting fvNoteOpen=true. By nulling the store first, the effect sees null and
    // skips the re-open branch even if Svelte's store and $state systems don't fully
    // batch across the two assignments. Defense-in-depth alongside the untrack() fix
    // above (which removes the fvNoteOpen reactive dep that was waking the effect).
    if (DEBUG) { const _prevId = untrack(() => $activeNote?.meta?.id ?? 'null'); console.log(`[PANE-DEBUG] $activeNote SET in goBackToCards | old="${_prevId}" new="null"`); console.log('[PANE-DEBUG] stack:', new Error().stack?.split('\n').slice(1, 5).join('\n')); }
    $activeNote = null;
    const _prevFvNoteOpen = fvNoteOpen;
    fvNoteOpen = false;
    if (DEBUG) console.log('[PANE-DEBUG] fvNoteOpen =', false, '(was', _prevFvNoteOpen, ')');
    const _prevSelectedProjectNote = selectedProjectNote;
    selectedProjectNote = null;
    if (DEBUG) console.log('[PANE-DEBUG] selectedProjectNote =', null, '(was', _prevSelectedProjectNote?.id, ')');
  }

  // Folder-view note overlay resize (#2180 PR B).
  // #2336: max clamp is now stageWidth (panel can fill the whole stage; noteLeft floors at 0).
  // The clamp is on panelWidth (= stageWidth - noteLeft); noteLeft is derived.
  function startFvNoteResize(e: MouseEvent) {
    e.preventDefault();
    // #2331: set BEFORE attaching mousemove/mouseup so the guard is live for the entire
    // gesture, including the synthetic click .fv-cards-panel or .fv-void may receive if
    // mouseup lands outside .fv-note (#2329 re-scoped the close-handler off .fv-stage).
    isFvNoteResizing = true;
    const startX = e.clientX;
    const startLeft = noteLeft;
    // fvStageEl is always bound by the time the resize handle is reachable — the handle
    // only renders inside {#if $activeNote}, which requires fvStageEl to already be in
    // the DOM. The fallback (startLeft + NOTE_PANEL_MIN_WIDTH) is the minimum stageWidth
    // that keeps the current noteLeft geometrically valid; it cannot produce a negative result.
    const stageWidth = fvStageEl?.offsetWidth ?? (startLeft + NOTE_PANEL_MIN_WIDTH);
    function onMouseMove(ev: MouseEvent) {
      const delta = startX - ev.clientX;          // positive = dragged left = panel widens
      const panelWidth = stageWidth - startLeft + delta;
      const clamped = Math.max(NOTE_PANEL_MIN_WIDTH, Math.min(stageWidth, panelWidth));
      noteLeft = stageWidth - clamped;
    }
    function onMouseUp() {
      localStorage.setItem('pane-fv-note-width', String(noteLeft));
      document.removeEventListener('mousemove', onMouseMove);
      document.removeEventListener('mouseup', onMouseUp);
      document.body.style.cursor = '';
      document.body.style.userSelect = '';
      // #2331: the browser's synthetic click (fired when mouseup lands outside .fv-note)
      // arrives AFTER this mouseup handler returns, but within the same event loop's event
      // dispatch sequence — clearing the flag synchronously here would clear it before that
      // synthetic click reaches the .fv-cards-panel/.fv-void onclick guards (#2329). Deferring
      // via setTimeout(..., 0) pushes the clear to the next macrotask, after the synthetic
      // click has already been dispatched and observed the flag as true.
      setTimeout(() => { isFvNoteResizing = false; }, 0);
    }
    document.addEventListener('mousemove', onMouseMove);
    document.addEventListener('mouseup', onMouseUp);
    document.body.style.cursor = 'col-resize';
    document.body.style.userSelect = 'none';
  }

  // #2339 PR9 ruling 2: sheet-panel resize, parallel to startFvNoteResize but
  // operating directly on fvSheetWidth (a width, not an offset) and persisting
  // to the separate `pane-fv-sheet-width` key — does not touch noteLeft or
  // the note-width key.
  function startFvSheetResize(e: MouseEvent) {
    e.preventDefault();
    isFvNoteResizing = true;
    const startX = e.clientX;
    const startWidth = fvSheetWidth;
    const stageWidth = fvStageEl?.offsetWidth ?? (startWidth + NOTE_PANEL_MIN_WIDTH);
    function onMouseMove(ev: MouseEvent) {
      const delta = startX - ev.clientX; // positive = dragged left = panel widens
      fvSheetWidth = Math.max(NOTE_PANEL_MIN_WIDTH, Math.min(stageWidth, startWidth + delta));
    }
    function onMouseUp() {
      localStorage.setItem('pane-fv-sheet-width', String(fvSheetWidth));
      document.removeEventListener('mousemove', onMouseMove);
      document.removeEventListener('mouseup', onMouseUp);
      document.body.style.cursor = '';
      document.body.style.userSelect = '';
      setTimeout(() => { isFvNoteResizing = false; }, 0);
    }
    document.addEventListener('mousemove', onMouseMove);
    document.addEventListener('mouseup', onMouseUp);
    document.body.style.cursor = 'col-resize';
    document.body.style.userSelect = 'none';
  }

  // ===== #2252: Folder-view column collapse rails + persistence =====
  //
  // Reuses the generic ai_settings KV store (get_ai_setting/set_ai_setting Tauri commands,
  // src-tauri/src/notes.rs, already registered in lib.rs) verbatim — zero new Rust commands.
  // Keys: `folder:{scopeId}:cabinet:collapsed` / `folder:{scopeId}:tasks:collapsed`,
  // values are the literal strings 'true'/'false' (same convention as onboarding_completed,
  // see src-tauri/src/projects.rs). ai_settings is NOT in CRR_TABLES — per-device, never syncs.
  //
  // scopeId: when a folder is selected, $activeFolder.id (a `projects` row with non-null
  // parent_id — the exact ID delete_folder's cleanup below matches against). When the
  // folder view is showing a project's root-level notes (no subfolder selected — e.g. a
  // project with no folders yet), $activeProject.id is used instead so root-level collapse
  // state still persists per-project rather than being silently dropped. Both share the same
  // `folder:` key prefix — intentional: it identifies "the column-scope currently being
  // viewed", not literally an FK to a folder row, and delete_folder's cleanup is scoped by
  // exact ID match so a project-root row (keyed by project id) is never touched by a folder
  // deletion, only by the project's own deletion path (out of scope for #2252 — project
  // deletion already hard-deletes the whole project tree; the 2 leftover ai_settings rows
  // are the same class of harmless orphan the audit flagged as pre-existing/acceptable).
  function fvColumnScopeId(): string | null {
    return $activeFolder?.id ?? $activeProject?.id ?? null;
  }

  // #2252 Bug 3 fix: identity-memo guard. This effect's only tracked dependencies
  // (fvColumnScopeId() -> $activeFolder/$activeProject) are already correctly scoped —
  // it does not read $activeNote/fvNoteOpen. The reported "fires on every note open/close"
  // symptom traces to upstream store-reference churn (separate issue #2324): $activeFolder/
  // $activeProject can be reassigned to a new object reference with the same .id during note
  // navigation, which re-triggers this effect via Svelte's reference-inequality reactivity.
  // This guard makes the effect robust to that churn by comparing the resolved *id* value
  // (not object identity) and no-op'ing when it hasn't actually changed, without needing to
  // hunt down and fix every upstream reassignment site (out of scope here; see #2324).
  let lastCollapseLoadScopeId: string | null = null;

  // Monotonic counter guarding the async get_ai_setting reads below — mirrors selectSeq in
  // noteService.ts. If two scope changes fire in quick succession, an older/slower Promise.all
  // resolve could otherwise land after a newer scope's effect run and overwrite
  // cabinetCollapsed/tasksCollapsed with stale values (#2335).
  let collapseLoadSeq = 0;

  // Load persisted collapse state whenever the viewed folder/project changes. Missing key
  // (first-ever visit to this folder) defaults to expanded — get_ai_setting returns None/null
  // for keys that were never written, so no special-case insert is needed on folder creation.
  $effect(() => {
    const scopeId = fvColumnScopeId();
    if (scopeId === lastCollapseLoadScopeId) return;
    lastCollapseLoadScopeId = scopeId;
    if (!scopeId) {
      cabinetCollapsed = false;
      tasksCollapsed = false;
      return;
    }
    const mySeq = ++collapseLoadSeq;
    (async () => {
      try {
        const [cabinetVal, tasksVal] = await Promise.all([
          invoke<string | null>('get_ai_setting', { key: `folder:${scopeId}:cabinet:collapsed` }),
          invoke<string | null>('get_ai_setting', { key: `folder:${scopeId}:tasks:collapsed` }),
        ]);
        if (mySeq !== collapseLoadSeq) return; // superseded — discard stale resolve
        cabinetCollapsed = cabinetVal === 'true';
        tasksCollapsed = tasksVal === 'true';
      } catch (e) {
        if (mySeq !== collapseLoadSeq) return; // superseded — discard stale resolve
        console.error('[FV-COLLAPSE] Failed to load collapse state for scope', scopeId, ':', e);
        // Fail open (expanded) rather than leaving stale state from a previous folder.
        cabinetCollapsed = false;
        tasksCollapsed = false;
      }
    })();
  });

  // Write-immediately on toggle (single discrete click, no debounce needed — mirrors the
  // audit's guidance, distinct from the drag-based noteLeft persistence which debounces to
  // mouseup because a drag fires many times per gesture).
  async function setColumnCollapsed(column: 'cabinet' | 'tasks', collapsed: boolean) {
    const scopeId = fvColumnScopeId();
    if (column === 'cabinet') cabinetCollapsed = collapsed;
    else tasksCollapsed = collapsed;

    // If a note panel is open, the overlay's left anchor must jump straight to the new
    // column boundary — not a reactive side-effect (see fvColumnBoundary() comment above),
    // an explicit call here so it only fires on an actual toggle.
    if (fvNoteOpen) {
      noteLeft = fvColumnBoundary();
    }

    if (!scopeId) return;
    try {
      await invoke('set_ai_setting', {
        key: `folder:${scopeId}:${column}:collapsed`,
        value: String(collapsed),
      });
    } catch (e) {
      console.error(`[FV-COLLAPSE] Failed to save ${column} collapse state for scope`, scopeId, ':', e);
    }
  }

  // #2252 Bug 1 fix (LAUNCH-BLOCKING): stop the click from bubbling up to
  // .fv-cards-panel's onclick (goBackToCards()) (#2329 re-scoped this handler off
  // .fv-stage onto .fv-cards-panel; .fv-collapse buttons live inside .grid-folder-content,
  // which is inside .fv-cards-panel, so they're still in the bubble path and still need
  // this guard). All 6 collapse-button onclick bindings pass the event implicitly (Svelte
  // passes the DOM event when a handler is bound directly), so adding an (e: MouseEvent)
  // param here and calling e.stopPropagation() first fixes all 6 sites without touching
  // any of the onclick={...} bindings themselves.
  function toggleCabinetCollapsed(e: MouseEvent) {
    e.stopPropagation();
    setColumnCollapsed('cabinet', !cabinetCollapsed);
  }

  function toggleTasksCollapsed(e: MouseEvent) {
    e.stopPropagation();
    setColumnCollapsed('tasks', !tasksCollapsed);
  }

  // Check if the entire project is a system project (e.g., Pane System)
  let isSystemProject = $derived(
    $activeProject?.name === 'Pane System'
  );
  
  // Helper: Check if a note is in a system folder (for context menu protection)
  function isNoteInSystemFolder(note: NoteMeta): boolean {
    if (isSystemProject) return true;
    const folder = projectFolders.find(f => f.id === note.folder_id);
    return !!(folder && (folder.is_system || folder.name === 'AI'));
  }
  

  

  

  
  function clampMenuPosition(x: number, y: number): { x: number, y: number } {
    const menuWidth = 240;
    const menuHeight = 320;
    const clampedX = x + menuWidth > window.innerWidth ? x - menuWidth : x;
    const clampedY = y + menuHeight > window.innerHeight ? y - menuHeight : y;
    return { x: Math.max(0, clampedX), y: Math.max(0, clampedY) };
  }

  function showContextMenu(e: MouseEvent, note: NoteMeta) {
    e.preventDefault();
    // Don't show context menu for notes in Pane System (no actions available)
    if (isSystemProject) return;
    const pos = clampMenuPosition(e.clientX, e.clientY);
    contextMenuState = { phase: 'actions', x: pos.x, y: pos.y, note };
  }
  
  async function startMoveNote() {
    if (!contextMenuState || contextMenuState.phase !== 'actions') return;
    const { x, y, note } = contextMenuState;
    try {
      const projects = (await api.listProjects()).filter(p => !p.parent_id && p.id !== $activeProject?.id);
      contextMenuState = { phase: 'projects', x, y, note, projects };
    } catch (e) {
      console.error('Failed to load projects:', e);
      contextMenuState = null;
    }
  }
  
  async function selectMoveProject(project: Project) {
    if (!contextMenuState || contextMenuState.phase !== 'projects') return;
    const { x, y, note } = contextMenuState;
    try {
      const folders = (await api.listFolders(project.id)).filter(f => !f.is_system);
      contextMenuState = { 
        phase: 'folders', 
        x, 
        y, 
        note, 
        projectId: project.id,
        projectName: project.name,
        folders
      };
    } catch (e) {
      console.error('Failed to load folders:', e);
      contextMenuState = null;
    }
  }
  
  async function executeMove(folder: Project) {
    if (!contextMenuState || contextMenuState.phase !== 'folders') return;
    const { note, projectId: targetProjectId } = contextMenuState;
    const targetFolderId = folder.id;
    
    contextMenuState = null;
    
    try {
      await invoke('move_note', {
        noteId: note.id,
        targetProjectId,
        targetFolderId,
      });
      // #2215: Immediate visual removal before DB refresh — same belt-and-suspenders
      // pattern as confirmDeleteNote(), guards against stale $notes store re-sync.
      taskNotes = taskNotes.filter(n => n.id !== note.id);
      await loadTaskNotes();
      if ($activeNote?.meta.id === note.id) {
        // #2192: Route through goBackToCards() so fvNoteOpen is cleared.
        goBackToCards();
      }
    } catch (e) {
      console.error('Failed to move note:', e);
    }
  }
  
  function goBackToActions() {
    if (!contextMenuState || contextMenuState.phase === 'actions') return;
    const { x, y, note } = contextMenuState;
    contextMenuState = { phase: 'actions', x, y, note };
  }
  
  function goBackToProjects() {
    if (!contextMenuState || contextMenuState.phase !== 'folders') return;
    const { x, y, note, projects } = contextMenuState as any;
    // Reload projects if we don't have them
    if (!projects) {
      api.listProjects().then(projs => {
        const filtered = projs.filter(p => !p.parent_id && p.id !== $activeProject?.id);
        contextMenuState = { phase: 'projects', x, y, note, projects: filtered };
      });
    } else {
      contextMenuState = { phase: 'projects', x, y, note, projects };
    }
  }
  
  function deleteContextNote() {
    if (!contextMenuState || contextMenuState.phase !== 'actions') return;
    const note = contextMenuState.note;
    contextMenuState = null;
    deleteConfirmNote = note;
  }

  async function confirmDeleteNote() {
    if (!deleteConfirmNote) return;
    const note = deleteConfirmNote;
    deleteConfirmNote = null;
    try {
      await api.moveToTrash(note.id);
      // #2214: Close panel if it was showing the deleted note, OR if we are in
      // the null-gap (#2192) where fvNoteOpen is true but $activeNote is already null.
      //
      // Secondary finding (#2228): _b1 (selectedProjectNote?.id === note.id) is dead in
      // practice — selectedProjectNote is only set by selectProjectNote() which is called
      // from card left-clicks, not from the right-click context-menu flow that produces
      // deleteConfirmNote. When the user RC-clicks a card without first LC-clicking it,
      // selectedProjectNote is null (never set for that card) so _b1 is always false.
      // Harmless guard, documented here; not changed since the fix is non-trivial (would
      // require setting selectedProjectNote on right-click too) and the other guards cover.
      const _b1 = selectedProjectNote?.id === note.id;
      const _b2 = $activeNote?.meta.id === note.id;
      const _b3 = fvNoteOpen && !$activeNote;
      if (DEBUG) console.log('[PANE-DEBUG] confirmDeleteNote guard:', {
        deletedId: note.id,
        selectedProjectNoteId: selectedProjectNote?.id,
        activeNoteId: $activeNote?.meta?.id,
        fvNoteOpen,
        branch1_selectedProjectNote: _b1,
        branch2_activeNote: _b2,
        branch3_nullGap: _b3,
        willClose: _b1 || _b2 || _b3,
      });
      if (_b1 || _b2 || _b3) {
        goBackToCards();
      }
      taskNotes = taskNotes.filter(n => n.id !== note.id);
      await loadTaskNotes();
    } catch (e) {
      console.error('Failed to delete note:', e);
    }
  }
  
  // deleteNote(note) removed — dead code (zero call sites). All deletes route
  // through confirmDeleteNote() via the ConfirmModal. (#2214)
  
  function startResize(e: MouseEvent) {
    isResizing = true;
    const startX = e.clientX;
    const startWidth = $editorListWidth;
    function onMouseMove(e: MouseEvent) {
      $editorListWidth = Math.max(240, Math.min(600, startWidth + (e.clientX - startX)));
    }
    function onMouseUp() {
      isResizing = false;
      window.removeEventListener('mousemove', onMouseMove);
      window.removeEventListener('mouseup', onMouseUp);
    }
    window.addEventListener('mousemove', onMouseMove);
    window.addEventListener('mouseup', onMouseUp);
  }
  let collapsedFolders: Record<string, boolean> = $state({});

  
  function stripMarkdown(text: string): string {
    return text.replace(/^#{1,6}\s+/gm, '').replace(/\*\*/g, '').replace(/\*/g, '').replace(/`/g, '').replace(/\[([^\]]+)\]\([^)]+\)/g, '$1').trim();
  }
  // draggedNote / dragOverTarget: shared by the live Cabinet-column status-drop
  // path (handleColumnDrop) and the Cabinet reorder-drag path (#2334).
  let draggedNote: NoteMeta | null = $state(null);
  let dragOverTarget: string | null = $state(null);

  // #2334 (container-level relocation): Cabinet-only reorder-drag state. Distinct
  // from draggedNote/dragOverTarget (status-change drag) per the event-propagation
  // contract — reorder drags never set draggedNote, so a Cabinet-internal reorder
  // can never be misread as a cross-column status-change drop.
  // cabinetDropIndicatorIndex is the coordinate-resolved insertion slot (0..N),
  // shared by both the drop-indicator render and the actual drop resolution —
  // one source of truth for "where will this land" (see dragReorderIndex.ts).
  let cabinetDragSourceId: string | null = $state(null);
  let cabinetDropIndicatorIndex: number | null = $state(null);
  let cabinetListEl: HTMLDivElement | undefined = $state(undefined);

  // activeTab uses activeViewTab store (survives remounts)



  // Event-source inventory (#2215 class fix):
  //   pane-data-changed  → DOM CustomEvent (api.ts, Editor.svelte, NotesBrowser.svelte)
  //                         → window.addEventListener ✓
  //   pane-note-saved    → DOM CustomEvent (Editor.svelte)
  //                         → window.addEventListener ✓
  //   pane-note-created  → DOM CustomEvent (api.ts)
  //                         → window.addEventListener ✓
  //   pane-note-changed  → Tauri IPC only (notes.rs, ai/commands.rs via window.emit())
  //                         → MUST use listen(); DOM addEventListener never receives it.
  //                         → See onMount below.
  //
  // #2972 stage 2 (§34.5): `pane-data-changed`/`pane-note-created`'s old
  // full-`loadProjectData()`-reload listeners are replaced (§34.2's
  // "Replaced" verdict) by `handleDataChangedEvent` below, patching the
  // affected row/folder in place. `pane-note-saved` is unrelated to #2972
  // (not one of the six events this issue folds in) — its own
  // `loadProjectData()` trigger is untouched.
  $effect(() => {
    const dataHandler = () => { loadProjectData(); };
    window.addEventListener('pane-note-saved', dataHandler);
    return () => {
      window.removeEventListener('pane-note-saved', dataHandler);
    };
  });

  // #2972 stage 2 (§34.5 WARN closure, PR #3016 / Reed's 2026-10-11 audit —
  // Matt's sign-off: "fix before merge"): single pane-data-changed handler
  // — patches, inserts or removes just the ONE affected row in the
  // cabinet/folder list in place, via the new single-entity read
  // (`api.getProjectOrFolder`). No more `listFolders($activeProject.id)`
  // round-trip for every event. Only `entity_type: folder` has a row in
  // `projectFolders` to patch — a `project`-level event has no entry in
  // this per-project folder subtree to touch (the active project itself
  // isn't a member of its own `projectFolders` array), so it's a no-op
  // here, same as it always effectively was (the old blanket refetch for a
  // project event just re-returned the identical folder list).
  async function handleDataChangedEvent(payload: DataChangedPayload) {
    const { entity_id, entity_type, reason } = payload;
    if (!$activeProject) return;

    if (entity_type === ENTITY_FOLDER) {
      if (reason === REASON_DELETED) {
        // A delete removes it locally with no read at all.
        projectFolders = projectFolders.filter(f => f.id !== entity_id);
        return;
      }
      try {
        const folder = await api.getProjectOrFolder(entity_id);
        // Scope check mirrors what `listFolders($activeProject.id)`'s
        // recursive CTE already guaranteed: only folders whose parent chain
        // leads back to $activeProject belong in `projectFolders`. A direct
        // child of the active project, or a child of a folder already known
        // to be in this project's subtree, qualifies.
        const belongs =
          folder.parent_id === $activeProject.id ||
          projectFolders.some(f => f.id === folder.parent_id);
        const idx = projectFolders.findIndex(f => f.id === entity_id);
        if (!belongs) {
          if (idx !== -1) projectFolders = projectFolders.filter(f => f.id !== entity_id);
          return;
        }
        if (idx === -1) {
          projectFolders = [...projectFolders, folder].sort((a, b) => a.name.localeCompare(b.name));
        } else {
          projectFolders = projectFolders
            .map(f => (f.id === entity_id ? { ...f, ...folder } : f))
            .sort((a, b) => a.name.localeCompare(b.name));
        }
      } catch (e) {
        console.error('[NOTES-BROWSER] Failed to patch folder for pane-data-changed event:', entity_id, e);
      }
      return;
    }

    if (entity_type === ENTITY_PROJECT) return;

    if (entity_type !== ENTITY_NOTE && entity_type !== ENTITY_SHEET) return;

    if (reason === REASON_DELETED) {
      taskNotes = taskNotes.filter(n => n.id !== entity_id);
      return;
    }

    try {
      // `api.readNote()` — not `entityService.getNote()` — remaps the raw
      // backend `status`/`priority`/`due_date` fields onto
      // `task_status`/`task_priority`/`task_due_date`, matching the shape
      // `taskNotes` (and every card renderer reading it) expects, same as
      // `listNotesByProject()`'s existing `transformNoteMeta()` convention.
      const noteData = await api.readNote('', entity_id);
      const meta = noteData.meta;
      const belongsHere =
        meta.project_id === $activeProject.id &&
        (!$activeFolder || meta.folder_id === $activeFolder.id);
      const idx = taskNotes.findIndex(n => n.id === entity_id);

      if (!belongsHere) {
        if (idx !== -1) taskNotes = taskNotes.filter(n => n.id !== entity_id);
        return;
      }

      if (idx === -1) {
        taskNotes = [...taskNotes, meta];
      } else {
        taskNotes = taskNotes.map(n => (n.id === entity_id ? { ...n, ...meta } : n));
      }
    } catch (e) {
      console.error('[NOTES-BROWSER] Failed to patch/insert note for pane-data-changed event:', entity_id, e);
    }
  }

  $effect(() => {
    const handler = (e: CustomEvent) => {
      const noteId = e.detail?.noteId;
      if (!noteId) return;
      if (taskNotes.length > 0) {
        const note = taskNotes.find(n => n.id === noteId || n.slug === noteId);
        if (note) selectProjectNote(note);
      } else {
        // #2874 item 18 fix 1: loadTaskNotes() hasn't resolved yet (no more
        // setTimeout upstream in CanvasRenderer to paper over this). Queue
        // the request instead of dropping it silently — the taskNotes
        // watcher $effect below resolves it once the load completes.
        pendingSelectNoteId = noteId;
      }
    };
    window.addEventListener('pane-select-note', handler as any);
    return () => window.removeEventListener('pane-select-note', handler as any);
  });

  // #2874 item 18 fix 1: resolves a queued pane-select-note request once
  // taskNotes transitions from empty to populated (i.e. loadTaskNotes()'s
  // async load has completed). This is the completion signal that replaces
  // CanvasRenderer's old fixed-delay setTimeout.
  $effect(() => {
    if (pendingSelectNoteId && taskNotes.length > 0) {
      const noteId = pendingSelectNoteId;
      const note = taskNotes.find(n => n.id === noteId || n.slug === noteId);
      pendingSelectNoteId = null;
      if (note) selectProjectNote(note);
    }
  });

  function toggleFolderCollapse(folderId: string) {
    collapsedFolders[folderId] = !collapsedFolders[folderId];
  }

  async function selectProjectNote(note: NoteMeta) {
    if (DEBUG) console.log('[PANE-DEBUG] selectedProjectNote =', note.id, '(was', selectedProjectNote?.id, ') in selectProjectNote');
    selectedProjectNote = note;
    // FIX #619: Use noteService.selectNote to ensure canvasState stays in sync
    try {
      await noteService.selectNote(note);
    } catch (e) {
      console.error('Failed to load note:', e);
    }
  }

  $effect(() => {
    if ($activeProject) loadProjectData();
  });

  async function loadProjectData() {
    if (!$activeProject) return;
    try {
      projectFolders = await api.listFolders($activeProject.id);
      projectFolders.sort((a, b) => a.name.localeCompare(b.name));
      
      // AI folder auto-select removed — agent folders are the structure now
      
      await loadTaskNotes();
    } catch (e) {
      console.error('Failed to load project data:', e);
    }
  }

  // Reload taskNotes when activeFolder changes
  $effect(() => {
    const _ = $activeFolder; // Track folder changes
    if ($activeProject) loadTaskNotes();
  });

  // Reload taskNotes when task fields change
  $effect(() => {
    function handleTaskFieldsUpdated() {
      loadTaskNotes();
    }
    window.addEventListener('task-fields-updated', handleTaskFieldsUpdated);
    return () => window.removeEventListener('task-fields-updated', handleTaskFieldsUpdated);
  });

  // handleTaskMetaChanged (the old `pane-task-meta-changed` partial-field
  // patch, formerly registered via a dedicated $effect here) deleted
  // (#2972 stage 3, item 2 — this is the "third listener" alongside
  // AllTasksView.svelte/KanbanBoard.svelte — ruling 4 folds it in):
  // `handleDataChangedEvent`'s `taskNotes` patch above already does a
  // strict superset (full entity patch, plus insert/remove) for the same
  // `reason: "edited"` case.

  // Sync local taskNotes when $notes store updates (e.g. status change from editor)
  $effect(() => {
    const storeNotes = $notes;
    if (storeNotes.length > 0 && taskNotes.length > 0) {
      // Update matching notes in taskNotes with fresh data from store
      const storeMap = new Map(storeNotes.map(n => [n.id, n]));
      let changed = false;
      const updated = taskNotes.map(tn => {
        const fresh = storeMap.get(tn.id);
        if (fresh && (fresh.task_status !== tn.task_status || fresh.task_priority !== tn.task_priority || fresh.title !== tn.title)) {
          changed = true;
          return { ...tn, ...fresh };
        }
        return tn;
      });
      if (changed) {
        taskNotes = updated;
      }
    }
  });

  // #3044 (revisited): the New Note button's own creation code is gone —
  // this just calls noteService.createNewNote(), the ONE shared function
  // Cmd+N (+page.svelte) also calls. That function resolves the project's
  // default_folder_id itself and sets $activeNote/canvasState directly, so
  // there is nothing left to do here beyond a simple reentrancy guard
  // against a rapid double-click.
  let isCreatingNote = false;
  async function handleNewNote() {
    if (isCreatingNote) return;
    isCreatingNote = true;
    try {
      await noteService.createNewNote();
      // createNewNote() sets $activeNote itself; mirror it into this card
      // list's own "which card is highlighted" state (selectProjectNote()'s
      // job on a left-click) so the just-created card shows active too.
      selectedProjectNote = $activeNote?.meta ?? null;
    } catch (e) {
      console.error('Failed to create note:', e);
    } finally {
      isCreatingNote = false;
    }
  }

  // #2339 PR9 ruling 1: New Sheet button, mirrors handleNewNote's folder-
  // resolution logic exactly (same non-system-folder / $activeFolder
  // validation) but calls create_sheet via api.createSheet instead of
  // createNoteInProject.
  let isCreatingSheet = $state(false);
  async function handleNewSheet() {
    if (isCreatingSheet) return;
    isCreatingSheet = true;
    try {
      const nonSystemFolders = projectFolders.filter(f => !f.is_system && f.name !== 'AI');
      const selectedId = $activeFolder?.id;
      const selectedIsValid = selectedId && nonSystemFolders.some(f => f.id === selectedId);
      const folderId = selectedIsValid ? selectedId
        : (nonSystemFolders.length > 0 ? nonSystemFolders[0].id : $activeProject?.id);
      if (!folderId || !$activeProject) {
        console.log('[handleNewSheet] No valid folder/project found');
        return;
      }
      const sheetData = await api.createSheet({
        project_id: $activeProject.id,
        folder_id: folderId,
        title: 'Untitled Sheet',
      });
      const transformedNote: any = {
        meta: {
          id: sheetData.note_meta.id,
          title: sheetData.note_meta.title,
          slug: sheetData.note_meta.id,
          notebook: $activeProject.name,
          preview: '',
          updated_at: sheetData.note_meta.updated_at,
          created_at: sheetData.note_meta.created_at,
          file_path: sheetData.note_meta.file_path,
          project_id: sheetData.note_meta.project_id,
          folder_id: sheetData.note_meta.folder_id,
          tags: sheetData.note_meta.tags,
          pinned: sheetData.note_meta.pinned,
          word_count: 0,
          note_type: 'sheet',
        },
        content: '',
        jsonContent: null,
      };
      await loadTaskNotes();
      $activeNote = transformedNote;
      selectedProjectNote = transformedNote.meta;
    } catch (e) {
      console.error('[handleNewSheet] Failed to create sheet:', e);
    } finally {
      isCreatingSheet = false;
    }
  }

  // #2339 PR 15c — New Sheet button polish: three-step narrow collapse keyed
  // to the HOST HEADER's (.grid-folder-header) own rendered width. Same
  // pattern as KanbanBoard.svelte/AllTasksView.svelte and
  // SessionDropdownHeader.svelte's containerWidth tracking.
  let folderHeaderEl: HTMLElement | null = $state(null);
  let folderHeaderWidth = $state(1200); // start wide to avoid a narrow-mode flash
  let headerResizeObs: ResizeObserver | null = null;
  let headerButtonMode: HeaderButtonMode = $derived(resolveHeaderButtonMode(folderHeaderWidth));

  // Caret popover (menu mode, header width < 720px). #2339 PR 15c collision
  // check: NotesBrowser's .grid-folder-header has no existing overflow/kebab
  // menu (grepped for kebab/overflow-menu/dots-btn/chevD-style affordances —
  // none found), so this caret is a genuinely new affordance, not a join.
  // See PR body.
  let sheetCaretOpen = $state(false);

  function handleNewSheetFromCaret() {
    sheetCaretOpen = false;
    handleNewSheet();
  }

  // #2419/#3044: wiki-link click navigation — open an existing note. A
  // missing note stays inert (no create, no confirm) per #3044's ruling.
  // Logic lives in wikiLinkNavigation.ts (shared with KanbanBoard/AllTasksView;
  // tested there).
  async function handleWikiLinkNavigation(title: string) {
    try {
      await navigateWikiLink(title, {
        searchFn: (q) => api.searchNotes(q),
        selectNoteFn: (note, contextOverride) => noteService.selectNote(note, contextOverride),
        getNoteRawFn: async (id) => {
          const raw: any = await api.getNoteRaw(id);
          return raw?.meta ?? null;
        },
      });
    } catch (e) {
      console.error('[handleWikiLinkNavigation] Failed to navigate from wiki link:', e);
    }
  }

  async function loadTaskNotes() {
    try {
      let allTaskNotes: NoteMeta[] = [];
      
      if (!$activeProject) return;
      
      if ($activeFolder) {
        allTaskNotes = await api.listNotesByFolder($activeFolder.id, $activeProject.id);
      } else {
        allTaskNotes = await api.listNotesByProject($activeProject.id);
      }
      
      taskNotes = allTaskNotes;
      await loadSheetSummaries();

    } catch (e) {
      console.error('Failed to load task notes:', e);
      taskNotes = [];
    }
  }

  // #2339 PR 15a: fetches card summaries for whichever loaded taskNotes are
  // sheet notes, one batched call. Best-effort — a failure here must not
  // block the folder view from rendering.
  async function loadSheetSummaries() {
    const sheetIds = taskNotes.filter(n => n.note_type === 'sheet').map(n => n.id);
    if (sheetIds.length === 0) {
      sheetSummaries = new Map();
      return;
    }
    try {
      const summaries = await api.getSheetCardSummaries(sheetIds);
      sheetSummaries = new Map(summaries.map(s => [s.note_id, s]));
    } catch (e) {
      console.warn('Failed to load sheet card summaries:', e);
      sheetSummaries = new Map();
    }
  }

  function handleCardClick(note: NoteMeta) {
    selectProjectNote(note);
  }

  function handleCloseSlideOver() {
    showSlideOver = false;
    selectedNote = null;
    loadTaskNotes(); // Refresh data after potential changes
  }

  function handleFilterByFolder(folderId: string | null) {
    selectedFolder = folderId;
    loadTaskNotes();
  }

  function formatDate(dateStr: string): string {
    if (!dateStr) return '';
    try {
      const date = new Date(dateStr);
      const now = new Date();
      const diffTime = date.getTime() - now.getTime();
      const diffDays = Math.ceil(diffTime / (1000 * 60 * 60 * 24));

      if (diffDays === 0) {
        return 'Today';
      } else if (diffDays === 1) {
        return 'Tomorrow';
      } else if (diffDays < 0) {
        return `${Math.abs(diffDays)}d overdue`;
      } else {
        return date.toLocaleDateString([], { month: 'short', day: 'numeric' });
      }
    } catch {
      return '';
    }
  }

  // getPriorityColor moved to Editor component

  function getStatusBorderColor(status: string | undefined): string {
    switch (status) {
      case 'todo': return '#A0A0A0';
      case 'in_progress': return '#f59e0b';
      case 'review': return '#2196F3';
      case 'done': return '#10b981';
      default: return 'transparent';
    }
  }

  function formatDateShort(dateStr: string): string {
    if (!dateStr) return '';
    try {
      const date = new Date(dateStr);
      return date.toLocaleDateString([], { month: 'short', day: 'numeric' });
    } catch { return ''; }
  }


  function getPreviewLines(preview: string | undefined): [string, string] {
    if (!preview) return ['', ''];
    const text = stripMarkdown(preview);
    // Split into roughly two lines (~60 chars each)
    if (text.length <= 60) return [text, ''];
    const mid = text.lastIndexOf(' ', 60);
    const split = mid > 20 ? mid : 60;
    return [text.slice(0, split), text.slice(split).trim()];
  }

  function getFolderName(notebook: string): string {
    if (notebook.includes('/')) return notebook.split('/').pop() || '';
    return '';
  }

  // Metadata pills state
  // Metadata pill handling moved to Editor component

  async function handleColumnDrop(e: DragEvent, targetStatus: string) {
    e.preventDefault();
    dragOverTarget = null;
    if (!draggedNote) return;
    if (draggedNote.task_status === targetStatus) {
      // Defect 2 (#2334 diagnosis): silent no-op guard — must be visible at debug level.
      console.debug('[Cabinet] drop: status unchanged, no-op', { noteId: draggedNote.id, from: draggedNote.task_status, to: targetStatus });
      draggedNote = null;
      return;
    }
    
    // Capture into locals BEFORE the await — `draggedNote` can be nulled
    // asynchronously mid-flight by ondragend={cabinetDragEnd} (#2334), which
    // fires per the DnD spec right after this handler's synchronous portion
    // suspends at the await below. Reading `draggedNote` again after the
    // await races that nuller. Matches KanbanBoard.svelte's handleDrop
    // (captures into `taskToMove` before its await) — see #2334 fix.
    const noteId = draggedNote.id;
    draggedNote = null;

    try {
      await invoke('update_note', { request: { note_id: noteId, status: targetStatus } });
      // Update local state
      taskNotes = taskNotes.map(n => 
        n.id === noteId ? { ...n, task_status: targetStatus as TaskStatus } : n
      );
      // #2972 stage 3 (§34.1.3): migrated from the bare, payload-less
      // `pane-data-changed` CustomEvent to the single entity-identified
      // event, `reason: "edited"`.
      dispatchDataChanged(noteId, ENTITY_NOTE, REASON_EDITED);
    } catch(e) { console.error('[Cabinet] status update failed:', e); }
  }

  // ── Cabinet drag-reorder (#2334, relocated to container level) ─────────────
  // The reorder drop handler lives on the LIST CONTAINER (.folder-card-list),
  // not on per-card wrappers — the entire column surface, including the seams
  // between cards, is a valid reorder target. Drop-index resolution and the
  // drop indicator both derive from the same coordinate math (dragReorderIndex.ts)
  // — one source of truth for "where will this land".
  //
  // Mode disambiguation (relocated, not redesigned, from the original per-card
  // design): the container-level handler branches on drag source:
  //   - cabinetDragSourceId set  → drag originated on a Cabinet card → reorder,
  //     resolve drop index from pointer Y, call reorderNotes.
  //   - cabinetDragSourceId NOT set → existing status-change semantics, pass
  //     through to handleColumnDrop.
  // The propagation contract from #2346 survives: stopPropagation() on the
  // container-level handler prevents handleColumnDrop from also firing during
  // a reorder.
  //
  // The drag SOURCE stays per-card (Card.svelte's own ondragstart via onDragStart
  // prop) — only the DROP handling moved to the container.

  function cabinetDragStart(note: NoteMeta) {
    cabinetDragSourceId = note.id;
    draggedNote = note;
  }

  function cabinetListDragOver(e: DragEvent) {
    e.preventDefault();
    if (!cabinetDragSourceId) return; // status-change drags use the column-level dragover highlight only
    e.stopPropagation();
    if (e.dataTransfer) e.dataTransfer.dropEffect = 'move';
    if (!cabinetListEl) return;
    // Uses the canonical `cabinetNotes` derived array (module-level, #2334) —
    // no local shadow, so this always matches what's rendered in the DOM.
    const cardEls = cabinetListEl.querySelectorAll('.cabinet-drag-wrap');
    const rects = cardRectsFromElements(cardEls);
    const rawDropIndex = resolveDropIndex(e.clientY, rects.length ? rects : cabinetNotes.map(n => ({ id: n.id, top: 0, height: 0 })));
    const sourceIdx = cabinetNotes.findIndex(n => n.id === cabinetDragSourceId);
    // #2334: suppress the indicator at the two identity-move slots (dropping
    // back into the dragged card's own slot, or the seam immediately below
    // it) so the UI never implies a move will happen when it won't.
    cabinetDropIndicatorIndex = shouldShowIndicator(sourceIdx, rawDropIndex) ? rawDropIndex : null;
  }

  function cabinetListDragLeave(e: DragEvent) {
    // Only clear when actually leaving the list container (not moving between children).
    if (e.currentTarget instanceof HTMLElement && e.relatedTarget instanceof Node && e.currentTarget.contains(e.relatedTarget)) return;
    cabinetDropIndicatorIndex = null;
  }

  async function cabinetListDrop(e: DragEvent) {
    // Case 2 (no cabinetDragSourceId): not a Cabinet-originated drag — let the
    // event bubble to the column-level ondrop={handleColumnDrop} unmodified
    // (existing status-change semantics, e.g. a Tasks-card dropped into Cabinet).
    if (!cabinetDragSourceId) {
      return;
    }

    e.preventDefault();
    e.stopPropagation();
    const dropIndicatorIndex = cabinetDropIndicatorIndex;
    cabinetDropIndicatorIndex = null;

    const sourceId = cabinetDragSourceId;
    cabinetDragSourceId = null;
    draggedNote = null;

    // Uses the canonical `cabinetNotes` derived array (module-level, #2334) —
    // no local shadow, so sourceIdx is always computed against the same
    // order the DOM cards were rendered in, including after a prior
    // same-session reorder's optimistic task_sort_order update.
    const sourceIdx = cabinetNotes.findIndex(n => n.id === sourceId);
    if (sourceIdx === -1) return;

    let rawDropIndex = dropIndicatorIndex;
    if (rawDropIndex === null && cabinetListEl) {
      const rects = cardRectsFromElements(cabinetListEl.querySelectorAll('.cabinet-drag-wrap'));
      rawDropIndex = resolveDropIndex(e.clientY, rects.length ? rects : cabinetNotes.map(n => ({ id: n.id, top: 0, height: 0 })));
    }
    if (rawDropIndex === null) return;

    const insertIdx = normalizeInsertIndex(sourceIdx, rawDropIndex);
    if (insertIdx === sourceIdx) {
      console.debug('[Cabinet] no-op: identity move, sourceIdx=%d rawDropIndex=%d insertIdx=%d', sourceIdx, rawDropIndex, insertIdx);
      return; // dropped back in the same slot — no-op
    }

    const reordered = [...cabinetNotes];
    const [moved] = reordered.splice(sourceIdx, 1);
    reordered.splice(insertIdx, 0, moved);

    // Gap-based sort_orders (step 1000, matching the new-note-on-top gap scheme)
    // so future new-note inserts (COALESCE(MIN(sort_order), 0) - 1000) never collide.
    const withOrders = reordered.map((n, idx) => ({ ...n, task_sort_order: idx * 1000 }));
    const orderById = new Map(withOrders.map(n => [n.id, n.task_sort_order]));

    // Optimistic local update — preserves the new Cabinet order immediately.
    taskNotes = taskNotes.map(n => orderById.has(n.id) ? { ...n, task_sort_order: orderById.get(n.id) } : n);

    try {
      await reorderNotes(withOrders.map(n => [n.id, n.task_sort_order!] as [string, number]));
    } catch (err) {
      console.error('[Cabinet] reorder persist failed:', err);
      await loadTaskNotes(); // Resync with server truth on failure
    }
  }

  function cabinetDragEnd() {
    cabinetDragSourceId = null;
    cabinetDropIndicatorIndex = null;
    draggedNote = null;
  }

  onMount(() => {
    // Item 15 §3: mirror KanbanBoard.svelte:374 — clear any stale
    // $activeNote carry-over from whatever view was active before folder
    // view mounted. Without this, the fvNoteOpen effect above (line ~191)
    // sees a non-null $activeNote left over from another view (e.g. Kanban)
    // and reopens it here, potentially still missing/wrong note_type
    // (2026-09-08 diagnosis §4 — the "$activeNote carry-over" transport
    // mechanism). NotesBrowser mounts exactly once per app session (single
    // non-keyed mount site in CanvasRenderer.svelte), so this only fires on
    // first arrival at folder view, not on every folder switch within it.
    //
    // #3044 delta-2 (Reed): that clear used to be unconditional, which also
    // wiped a note THIS SAME mount was deliberately opening — Cmd+N from a
    // non-Notes screen, or a wiki-link click from AllTasksView/Kanban, both
    // of which set $activeNote and write canvasState.view synchronously,
    // right before this onMount fires for the first time. One shared check
    // (noteService.clearStaleActiveNoteOnMount, used identically by
    // KanbanBoard.svelte's onMount) keeps $activeNote when it already IS
    // the note canvasState.context.noteId says this navigation targeted,
    // and clears it in every other case — the carry-over protection above
    // is unchanged.
    noteService.clearStaleActiveNoteOnMount();

    // #2972 stage 3 (item 4): one shared helper hearing BOTH channels —
    // backend `listen()` (Tauri IPC is invisible to window.addEventListener,
    // #2215) AND the app `CustomEvent` — replacing Stage 2's backend-only
    // `listen()` registration (which itself replaced the dead
    // `listen('pane-note-changed', ...)` registration, #2972 stage 1).
    const unlistenDataChanged = subscribeDataChanged((payload) => {
      handleDataChangedEvent(payload);
    });

    // #2339 PR 15c: header-width-based New Sheet/New Note collapse tracking.
    if (folderHeaderEl) {
      headerResizeObs = new ResizeObserver(entries => {
        for (const entry of entries) {
          folderHeaderWidth = entry.contentRect.width;
        }
      });
      headerResizeObs.observe(folderHeaderEl);
    }

    return () => {
      unlistenDataChanged.then(fn => fn());
    };
  });

  onDestroy(() => {
    headerResizeObs?.disconnect();
  });
</script>

{#if $activeProject}
  <div class="project-aggregate-view">
    <!-- Content area -->
    <div class="content-layout">
      <!-- Flex layout: folder view — fixed Cabinet + Tasks columns, right void (#2180 PR A) -->
      <div class="fv-work">
        
        <!-- Project Header (plain flex row — no longer a grid-spanning row) -->
        <div class="grid-folder-header" bind:this={folderHeaderEl}>
          <span class="folder-header-title">{$activeProject?.name || 'Project'}</span>
          {#if !isSystemProject}
            <div class="grid-folder-header-actions">
              <!-- #2339 PR 15c: New Sheet sits LEFT of New Note (New Note stays
                   rightmost/primary at every header width) — reordered from PR9's
                   New Note-then-New Sheet DOM order. class renamed .new-sheet-btn
                   -> .add-sheet-btn to match the naming convention already used by
                   KanbanBoard.svelte and AllTasksView.svelte's equivalent button. -->
              {#if headerButtonMode !== 'menu'}
                <button
                  class="add-sheet-btn btn--outline"
                  class:add-sheet-btn--icon-only={headerButtonMode === 'icon'}
                  onclick={handleNewSheet}
                  disabled={isCreatingSheet}
                  title="New Sheet"
                >
                  <svg width="14" height="14" viewBox="0 0 14 14" fill="none" stroke="currentColor">
                    <path d="M7 2V12M2 7H12" stroke-width="1.5" stroke-linecap="round"/>
                  </svg>
                  {#if headerButtonMode === 'full'}New Sheet{/if}
                </button>
              {/if}
              <button class="new-note-btn" onclick={handleNewNote} title="New Note">
                <svg width="14" height="14" viewBox="0 0 14 14" fill="none" stroke="currentColor">
                  <path d="M7 2V12M2 7H12" stroke-width="1.5" stroke-linecap="round"/>
                </svg>
                New Note
              </button>
              {#if headerButtonMode === 'menu'}
                <div class="sheet-caret-wrap">
                  <button
                    class="sheet-caret-btn"
                    onclick={() => sheetCaretOpen = !sheetCaretOpen}
                    title="More create options"
                    aria-haspopup="menu"
                    aria-expanded={sheetCaretOpen}
                  >
                    <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                      <polyline points="6,9 12,15 18,9"/>
                    </svg>
                  </button>
                  {#if sheetCaretOpen}
                    <div class="sheet-caret-overlay" onclick={() => sheetCaretOpen = false}></div>
                    <div class="sheet-caret-menu" role="menu">
                      <button class="sheet-caret-item" onclick={handleNewSheetFromCaret} disabled={isCreatingSheet}>
                        <span class="sheet-caret-icon sheet-caret-icon--accent">
                          <svg width="14" height="14" viewBox="0 0 14 14" fill="none" stroke="currentColor">
                            <path d="M7 2V12M2 7H12" stroke-width="1.5" stroke-linecap="round"/>
                          </svg>
                        </span>
                        New Sheet
                      </button>
                    </div>
                  {/if}
                </div>
              {/if}
            </div>
          {/if}
        </div>

        <!-- Stage: horizontal flex — fixed-width columns + right void -->
        <!-- position:relative here is the prerequisite for the absolute note overlay (PR B). -->
        <!-- #2329: close-handler is scoped to .fv-cards-panel (below), NOT .fv-stage.
             .fv-stage is now a plain layout container — .fv-note (the editor) is a
             sibling of .fv-cards-panel here, so editor-internal clicks (including
             clicks inside NodeView-mounted components like the checklist) never reach
             a close-handler at all; there is no bubble path from .fv-note into
             .fv-cards-panel. This mirrors KanbanBoard.svelte's .kanban-left-panel /
             .kanban-editor-panel sibling topology (audit: docs/audits/2026-08-06-2329-checklist-folder-view-scope.md). -->
        <div class="fv-stage" bind:this={fvStageEl}>

        <!-- #2329: close-handler now lives here, scoped to only the card-column
             subtree (mirrors KanbanBoard.svelte:597's .kanban-left-panel). .fv-note
             is NOT a descendant of this element (see sibling structure below), so
             the .fv-note closest() guard that used to be required here is gone —
             it's structurally impossible for an editor click to bubble into this
             handler. .pane-card, .fv-collapse, and .fv-railcol ARE still descendants
             of this wrapper (they render inside .grid-folder-content), so those three
             guards are retained. -->
        <div class="fv-cards-panel"
          onclick={(e) => {
            // #2331: state-based guard — while a resize-drag is in progress (or in the
            // deferred-clear window immediately after mouseup), swallow the click outright.
            if (isFvNoteResizing) return;
            if (fvNoteOpen
                && !(e.target as HTMLElement).closest('.pane-card')
                && !(e.target as HTMLElement).closest('.fv-collapse')
                && !(e.target as HTMLElement).closest('.fv-railcol')) {
              goBackToCards();
            }
          }}>

        <!-- Columns wrapper: Cabinet (300px) + Tasks (300px), fixed, never reflow -->
        <div class="grid-folder-content">
          <!-- Column Headers (sticky) -->
          <div class="grid-folder-toolbar">
            {#if true}
              {@const taskNotesFiltered = taskNotes.filter(n => n.task_status && n.task_status !== 'cabinet')}
            <!-- #2252: header cell narrows to var(--fv-rail) via .collapsed when the column
                 body below is collapsed (the "known trap" flagged in the scope audit —
                 the flex:1 50/50 split does not auto-narrow without this explicit override). -->
            <div class="column-header-left" class:collapsed={cabinetCollapsed}>
              {#if cabinetCollapsed}
                <button class="fv-collapse" onclick={toggleCabinetCollapsed} title="Expand Cabinet">
                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" width="16" height="16">
                    <rect x="3" y="4" width="18" height="16" rx="2.5"/><path d="M9 4v16"/><path d="M13 9.5 15.5 12 13 14.5"/>
                  </svg>
                </button>
              {:else}
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" width="16" height="16">
                  <rect width="20" height="5" x="2" y="3" rx="1"/>
                  <path d="M4 8v11a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8"/>
                  <path d="M10 12h4"/>
                </svg>
                Cabinet
                <span class="column-count-badge">{cabinetNotes.length}</span>
                <span class="column-header-spacer"></span>
                <button class="fv-collapse" onclick={toggleCabinetCollapsed} title="Collapse Cabinet">
                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" width="16" height="16">
                    <rect x="3" y="4" width="18" height="16" rx="2.5"/><path d="M9 4v16"/><path d="M15.5 9.5 13 12l2.5 2.5"/>
                  </svg>
                </button>
              {/if}
            </div>
            <div class="column-header-right" class:collapsed={tasksCollapsed}>
              {#if tasksCollapsed}
                <button class="fv-collapse" onclick={toggleTasksCollapsed} title="Expand Tasks">
                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" width="16" height="16">
                    <rect x="3" y="4" width="18" height="16" rx="2.5"/><path d="M9 4v16"/><path d="M13 9.5 15.5 12 13 14.5"/>
                  </svg>
                </button>
              {:else}
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" width="16" height="16">
                  <rect width="18" height="18" x="3" y="3" rx="2"/>
                  <path d="m9 12 2 2 4-4"/>
                </svg>
                Tasks
                <span class="column-count-badge">{taskNotesFiltered.length}</span>
                <span class="column-header-spacer"></span>
                <button class="fv-collapse" onclick={toggleTasksCollapsed} title="Collapse Tasks">
                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" width="16" height="16">
                    <rect x="3" y="4" width="18" height="16" rx="2.5"/><path d="M9 4v16"/><path d="M15.5 9.5 13 12l2.5 2.5"/>
                  </svg>
                </button>
              {/if}
            </div>
            {/if}
          </div>
          <!-- Scrollable columns -->
          <div class="folder-columns">
          <!-- LEFT: Cabinet Column -->
          <!-- #2252: .folder-column stays the width-toggling element (per the pre-existing CSS
               comment "PR C toggles width between var(--fv-col) and var(--fv-rail) via class").
               When collapsed, its content swaps to the .fv-railcol rail (design pattern) instead
               of the card list — NOT the doneCollapsed icon-only-stub pattern, which is not
               persisted and lacks the label/count anatomy this design requires. -->
          <div class="folder-column" class:collapsed={cabinetCollapsed} class:drag-over={dragOverTarget === 'cabinet'}
               ondragover={(e) => { e.preventDefault(); dragOverTarget = 'cabinet'; }}
               ondrop={(e) => handleColumnDrop(e, 'cabinet')}
               ondragleave={() => dragOverTarget = null}>
            {#if cabinetCollapsed}
              <button class="fv-railcol" onclick={toggleCabinetCollapsed} title="Expand Cabinet">
                <span class="fv-railcol__expand">
                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" width="16" height="16">
                    <rect x="3" y="4" width="18" height="16" rx="2.5"/><path d="M9 4v16"/><path d="M13 9.5 15.5 12 13 14.5"/>
                  </svg>
                </span>
                <span class="fv-railcol__icon">
                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" width="17" height="17">
                    <rect width="20" height="5" x="2" y="3" rx="1"/>
                    <path d="M4 8v11a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8"/>
                    <path d="M10 12h4"/>
                  </svg>
                </span>
                <span class="fv-railcol__label">Cabinet</span>
                <span class="fv-railcol__count">{cabinetNotes.length}</span>
              </button>
            {:else}
            <!-- #2334 (container-level relocation): the reorder drop handler lives HERE,
                 on the list container — not on per-card wrappers. The entire column
                 surface, including the seams/gaps between cards, is a valid reorder
                 target. Mode disambiguation (cabinetDragSourceId set → reorder,
                 unset → falls through to the column-level handleColumnDrop) happens
                 in cabinetListDrop. -->
            <div
              class="folder-card-list"
              bind:this={cabinetListEl}
              ondragover={cabinetListDragOver}
              ondragleave={cabinetListDragLeave}
              ondrop={cabinetListDrop}
            >
              {#if cabinetDropIndicatorIndex === 0}
                <div class="cabinet-drop-indicator"></div>
              {/if}
              {#each cabinetNotes as note, i (note.id)}
                <!-- #2334: drag SOURCE stays per-card (Card.svelte's ondragstart) — only
                     drop handling moved to the container above. No per-card
                     ondragover/ondrop/ondragleave here anymore. -->
                <div
                  class="cabinet-drag-wrap"
                  data-cabinet-id={note.id}
                  ondragend={cabinetDragEnd}
                >
                  <Card
                    data={normalizeNoteMeta(note, { projectNameOverride: $activeProject?.name, folderNameOverride: $activeFolder?.name, sheetSummary: cardSummaryFor(note) })}
                    active={selectedProjectNote?.id === note.id}
                    onSelect={(id) => { const n = taskNotes.find(x => x.id === id); if (n) selectProjectNote(n); }}
                    onContextMenu={(e, id) => { const n = taskNotes.find(x => x.id === id); if (n) showContextMenu(e, n); }}
                    onDragStart={(id) => { const n = taskNotes.find(x => x.id === id); if (n) cabinetDragStart(n); }}
                  />
                </div>
                {#if cabinetDropIndicatorIndex === i + 1}
                  <div class="cabinet-drop-indicator"></div>
                {/if}
              {/each}
            </div>
            {/if}
          </div>

          <!-- RIGHT: Tasks Column -->
          <div class="folder-column" class:collapsed={tasksCollapsed} class:drag-over={dragOverTarget === 'tasks'}
               ondragover={(e) => { e.preventDefault(); dragOverTarget = 'tasks'; }}
               ondrop={(e) => handleColumnDrop(e, 'todo')}
               ondragleave={() => dragOverTarget = null}>
            {#if tasksCollapsed}
              {@const taskNotesFiltered = taskNotes.filter(n => n.task_status && n.task_status !== 'cabinet')}
              <button class="fv-railcol" onclick={toggleTasksCollapsed} title="Expand Tasks">
                <span class="fv-railcol__expand">
                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" width="16" height="16">
                    <rect x="3" y="4" width="18" height="16" rx="2.5"/><path d="M9 4v16"/><path d="M13 9.5 15.5 12 13 14.5"/>
                  </svg>
                </span>
                <span class="fv-railcol__icon">
                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" width="16" height="16">
                    <rect width="18" height="18" x="3" y="3" rx="2"/>
                    <path d="m9 12 2 2 4-4"/>
                  </svg>
                </span>
                <span class="fv-railcol__label">Tasks</span>
                <span class="fv-railcol__count">{taskNotesFiltered.length}</span>
              </button>
            {:else}
            <div class="folder-card-list">
              <!-- TODO Group -->
              {#if taskNotes.filter(n => n.task_status === 'todo').length > 0}
                {@const todoNotes = taskNotes.filter(n => n.task_status === 'todo')}
                <div class="task-group">
                  <div class="group-header-text todo">
                    <span class="group-dot todo"></span>
                    Todo ({todoNotes.length})
                  </div>
                  {#each todoNotes as note}
                    <Card
                      data={normalizeNoteMeta(note, { projectNameOverride: $activeProject?.name, folderNameOverride: $activeFolder?.name, sheetSummary: cardSummaryFor(note) })}
                      active={selectedProjectNote?.id === note.id}
                      onSelect={(id) => { const n = taskNotes.find(x => x.id === id); if (n) selectProjectNote(n); }}
                      onContextMenu={(e, id) => { const n = taskNotes.find(x => x.id === id); if (n) showContextMenu(e, n); }}
                      onDragStart={(id) => { const n = taskNotes.find(x => x.id === id); if (n) { draggedNote = n; } }}
                    />
                  {/each}
                </div>
                <div class="group-divider-line"></div>
              {/if}

              <!-- IN PROGRESS Group -->
              {#if taskNotes.filter(n => n.task_status === 'in_progress').length > 0}
                {@const inProgressNotes = taskNotes.filter(n => n.task_status === 'in_progress')}
                <div class="task-group">
                  <div class="group-header-text in-progress">
                    <span class="group-dot in-progress"></span>
                    In Progress ({inProgressNotes.length})
                  </div>
                  {#each inProgressNotes as note}
                    <Card
                      data={normalizeNoteMeta(note, { projectNameOverride: $activeProject?.name, folderNameOverride: $activeFolder?.name, sheetSummary: cardSummaryFor(note) })}
                      active={selectedProjectNote?.id === note.id}
                      onSelect={(id) => { const n = taskNotes.find(x => x.id === id); if (n) selectProjectNote(n); }}
                      onContextMenu={(e, id) => { const n = taskNotes.find(x => x.id === id); if (n) showContextMenu(e, n); }}
                      onDragStart={(id) => { const n = taskNotes.find(x => x.id === id); if (n) { draggedNote = n; } }}
                    />
                  {/each}
                </div>
                <div class="group-divider-line"></div>
              {/if}

              <!-- REVIEW Group -->
              {#if taskNotes.filter(n => n.task_status === 'review').length > 0}
                {@const reviewNotes = taskNotes.filter(n => n.task_status === 'review')}
                <div class="task-group">
                  <div class="group-header-text review">
                    <span class="group-dot review"></span>
                    Review ({reviewNotes.length})
                  </div>
                  {#each reviewNotes as note}
                    <Card
                      data={normalizeNoteMeta(note, { projectNameOverride: $activeProject?.name, folderNameOverride: $activeFolder?.name, sheetSummary: cardSummaryFor(note) })}
                      active={selectedProjectNote?.id === note.id}
                      onSelect={(id) => { const n = taskNotes.find(x => x.id === id); if (n) selectProjectNote(n); }}
                      onContextMenu={(e, id) => { const n = taskNotes.find(x => x.id === id); if (n) showContextMenu(e, n); }}
                      onDragStart={(id) => { const n = taskNotes.find(x => x.id === id); if (n) { draggedNote = n; } }}
                    />
                  {/each}
                </div>
                <div class="group-divider-line"></div>
              {/if}

              <!-- DONE Group -->
              {#if taskNotes.filter(n => n.task_status === 'done').length > 0}
                {@const doneNotes = taskNotes.filter(n => n.task_status === 'done')}
                <div class="task-group">
                  <div class="group-header-text done">
                    <span class="group-dot done"></span>
                    Done ({doneNotes.length})
                  </div>
                  {#each doneNotes as note}
                    <Card
                      data={normalizeNoteMeta(note, { projectNameOverride: $activeProject?.name, folderNameOverride: $activeFolder?.name, sheetSummary: cardSummaryFor(note) })}
                      active={selectedProjectNote?.id === note.id}
                      onSelect={(id) => { const n = taskNotes.find(x => x.id === id); if (n) selectProjectNote(n); }}
                      onContextMenu={(e, id) => { const n = taskNotes.find(x => x.id === id); if (n) showContextMenu(e, n); }}
                      onDragStart={(id) => { const n = taskNotes.find(x => x.id === id); if (n) { draggedNote = n; } }}
                    />
                  {/each}
                </div>
              {/if}
            </div>
            {/if}
          </div>
          </div>
        </div><!-- close grid-folder-content -->
        </div><!-- close fv-cards-panel -->

        <!-- Right void: shown when no note is open. Covered by the note overlay when active. -->
        <!-- #2329: .fv-void has no interactive children, so it gets its own direct onclick
             calling goBackToCards() — no closest() gymnastics needed. This preserves
             click-outside-to-dismiss now that .fv-void sits outside .fv-cards-panel's
             close-handler scope. -->
        {#if !fvNoteOpen}
          <div class="fv-void" onclick={() => { if (fvNoteOpen && !isFvNoteResizing) goBackToCards(); }}>Select a note</div>
        {:else}
          <div class="fv-void" onclick={() => { if (fvNoteOpen && !isFvNoteResizing) goBackToCards(); }}></div>
        {/if}

        <!-- NOTE OVERLAY PANEL (#2180 PR B) -->
        <!-- position:absolute over the fixed columns — columns stay fully lit underneath. -->
        <!-- No backdrop/scrim. Dismiss via ✕ button, Esc, or click-outside (#2191). -->
        <!-- #2192: outer guard is fvNoteOpen (not $activeNote) so the panel shell stays
             mounted during the mandatory null gap in noteService.selectNote. The inner
             {#if $activeNote} guard prevents null-access on $activeNote.meta.id during
             the gap. During the gap the panel container stays in DOM (dark background)
             — columns never flash through. -->
        {#if fvNoteOpen}
          <div class="fv-note" style:left="{isActiveSheetNote ? fvSheetLeftPx() : noteLeft}px">
            <div class="fv-note__resize"
              onmousedown={isActiveSheetNote ? startFvSheetResize : startFvNoteResize}
              role="separator"
              aria-orientation="vertical"
              tabindex="-1"
            ></div>
            <button class="fv-note__close" onclick={goBackToCards} title="Close note (Esc)">✕</button>
            {#key $activeNote?.meta?.id}
              {#if $activeNote}
                <Editor
                  content={$activeNote.content}
                  title={$activeNote.meta.title}
                  noteId={$activeNote.meta.id}
                  slug={$activeNote.meta.slug}
                  noteMeta={$activeNote.meta}
                  jsonContent={$activeNote.jsonContent}
                  onUpdate={() => {}}
                  onTitleUpdate={(newTitle) => { taskNotes = taskNotes.map(n => n.id === $activeNote!.meta.id ? { ...n, title: newTitle } : n); }}
                  onNavigate={handleWikiLinkNavigation}
                  currentProject={$activeNote.meta.notebook}
                  onDelete={() => goBackToCards()}
                  onMoved={() => goBackToCards()}
                />
              {/if}
            {/key}
          </div>
        {/if}

        </div><!-- close fv-stage -->

      </div><!-- close fv-work -->

      <!-- Chat Sidebar rendered globally from +page.svelte -->
    </div>

    <!-- Unified Context Menu -->
    {#if contextMenuState}
      <div class="context-overlay" onclick={() => contextMenuState = null}></div>
      <div class="context-menu" style="left: {contextMenuState.x}px; top: {contextMenuState.y}px">
        {#if contextMenuState.phase === 'actions'}
          {#if !isNoteInSystemFolder(contextMenuState.note)}
            <button class="context-item" onclick={() => startMoveNote()}>
              <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                <path d="M15 3h4a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2h-4M10 17l5-5-5-5M13.8 12H3"/>
              </svg>
              Move to...
            </button>
            <button class="context-item delete" onclick={() => deleteContextNote()}>
              <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                <polyline points="3,6 5,6 21,6"></polyline>
                <path d="M19,6v14a2,2 0 0,1-2,2H7a2,2 0 0,1-2-2V6m3,0V4a2,2 0 0,1,2-2h4a2,2 0 0,1,2,2v2"></path>
              </svg>
              Delete
            </button>
          {/if}
        
        {:else if contextMenuState.phase === 'projects'}
          <button class="context-item back" onclick={() => goBackToActions()}>
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
              <polyline points="15,18 9,12 15,6"></polyline>
            </svg>
            Back
          </button>
          <div class="context-divider"></div>
          <div class="context-section-label">Move to project:</div>
          {#each contextMenuState.projects as project}
            <button class="context-item" onclick={() => selectMoveProject(project)}>
              {project.name}
            </button>
          {/each}
        
        {:else if contextMenuState.phase === 'folders'}
          <button class="context-item back" onclick={() => goBackToProjects()}>
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
              <polyline points="15,18 9,12 15,6"></polyline>
            </svg>
            Back
          </button>
          <div class="context-divider"></div>
          <div class="context-section-label">Folder in {contextMenuState.projectName}:</div>
          {#each contextMenuState.folders as folder}
            <button class="context-item" onclick={() => executeMove(folder)}>
              {folder.name}
            </button>
          {/each}
        {/if}
      </div>
    {/if}
    <!-- Slide-over panel -->
    <!-- SlideOver removed -->
  </div>
{/if}

{#if deleteConfirmNote}
  <ConfirmDialog
    title="Move to Trash?"
    message={`"${deleteConfirmNote.title || 'Untitled'}" will be moved to trash.`}
    confirmLabel="Move to Trash"
    onConfirm={confirmDeleteNote}
    onCancel={() => { deleteConfirmNote = null; }}
  />
{/if}

<style>

  /* ========== FOLDER VIEW FLEX LAYOUT (#2180) ========== */
  /* Replaces the old 3-column CSS grid (grid-main-area + folder-resize-handle + grid-editor-content). */
  /* Cabinet and Tasks are now fixed 300px columns; notes open as an overlay (PR B). */

  .fv-work {
    flex: 1;
    display: flex;
    flex-direction: column;
    overflow: hidden;
    min-height: 0;
    background: #0a0a0f;
  }

  /* The stage holds fixed-width columns and the right void area side-by-side. */
  /* position: relative is set here as the prerequisite for the note overlay (PR B). */
  /* overflow: hidden clips column scroll — does NOT clip position:fixed context menus */
  /* (position:fixed is viewport-relative and bypasses overflow clipping). */
  .fv-stage {
    /* CSS vars for column widths — single source of truth used by column cells and PR C collapse */
    --fv-col: 300px;
    --fv-rail: 46px;
    position: relative;
    flex: 1;
    display: flex;
    overflow: hidden;
    min-height: 0;
  }

  /* #2329: .fv-cards-panel wraps .grid-folder-content (only) so the click-outside close
     handler can be scoped to just the card-column subtree, structurally excluding .fv-note
     (mirrors KanbanBoard.svelte's .kanban-left-panel/.kanban-editor-panel sibling split).
     display:contents removes the wrapper from the box tree entirely, so .grid-folder-content
     participates in .fv-stage's flex layout exactly as if it were still a direct child —
     zero risk of breaking the fixed-column + void + note flex arrangement. Click events are
     unaffected: display:contents does not remove the element from the DOM, so bubbling
     through .fv-cards-panel (and closest() lookups against it, if ever needed) still works
     normally — only box generation/layout is suppressed. */
  .fv-cards-panel {
    display: contents;
  }

  /* Right void: shown when no note is open. Filled by the note overlay in PR B. */
  .fv-void {
    flex: 1;
    min-width: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    color: rgba(255,255,255,0.2);
    font-size: 13.5px;
  }

  /* ========== FOLDER VIEW COLUMN ========== */
  
  /* Project header — plain flex row, no longer a grid-spanning row (#2180) */
  .grid-folder-header {
    flex-shrink: 0;
    display: flex;
    align-items: center;
    justify-content: space-between;
    height: 48px;
    padding: 0 24px;
    border-bottom: 1px solid rgba(255,255,255,0.06);
    background: #0a0a0f;
  }

  .folder-header-title {
    font-size: 16px;
    font-weight: 600;
    color: #F0EDE8;
  }

  .new-note-btn {
    background: rgba(255,255,255,0.05);
    border: 1px solid rgba(255,255,255,0.08);
    border-radius: 6px;
    color: #A0A0A0;
    font-size: 12px;
    padding: 4px 10px;
    cursor: pointer;
    font-family: inherit;
    transition: all 0.15s;
    display: flex;
    align-items: center;
    gap: 4px;
  }

  .new-note-btn:hover {
    background: #00C8C8;
  }

  .new-note-btn svg {
    width: 14px;
    height: 14px;
    stroke: currentColor;
  }

  .grid-folder-header-actions {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  /* #2339 PR 15c: same box model as .new-note-btn — fill comes from the
     global .btn--outline class (src/app.css), applied alongside this class in
     the template. Renamed from .new-sheet-btn to .add-sheet-btn to match the
     naming convention used by KanbanBoard.svelte/AllTasksView.svelte's
     equivalent button. See app.css's .btn--outline comment. */
  .add-sheet-btn {
    border-width: 1px;
    border-style: solid;
    border-radius: 6px;
    font-size: 12px;
    padding: 4px 10px;
    cursor: pointer;
    font-family: inherit;
    transition: all 0.15s;
    display: flex;
    align-items: center;
    gap: 4px;
  }

  .add-sheet-btn:disabled {
    opacity: 0.5;
    cursor: default;
  }

  .add-sheet-btn svg {
    width: 14px;
    height: 14px;
    stroke: currentColor;
  }

  /* #2339 PR 15c: 720–900px header width — New Sheet drops its label but
     keeps a 32px square icon-only box. */
  .add-sheet-btn--icon-only {
    width: 32px;
    height: 32px;
    padding: 0;
    justify-content: center;
    gap: 0;
  }

  /* #2339 PR 15c: caret popover (header width < 720px). Collision-check
     finding (PR body): .grid-folder-header has no pre-existing
     overflow/kebab menu, so this caret is new, not a join. */
  .sheet-caret-wrap {
    position: relative;
  }

  .sheet-caret-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 32px;
    height: 32px;
    border: 1px solid transparent;
    border-radius: 6px;
    background: transparent;
    color: #A0A0A0;
    cursor: pointer;
    font-family: inherit;
    transition: all 0.15s;
  }

  .sheet-caret-btn:hover {
    background: rgba(255,255,255,0.06);
    color: #F0EDE8;
  }

  .sheet-caret-overlay {
    position: fixed;
    inset: 0;
    z-index: 999;
  }

  .sheet-caret-menu {
    position: absolute;
    top: calc(100% + 6px);
    right: 0;
    z-index: 1000;
    background: #1a1a1f;
    border: 1px solid rgba(255,255,255,0.12);
    border-radius: 6px;
    padding: 4px;
    min-width: 168px;
    box-shadow: 0 4px 20px rgba(0,0,0,0.6);
  }

  .sheet-caret-item {
    display: flex;
    align-items: center;
    gap: 9px;
    width: 100%;
    padding: 7px 9px;
    border: 0;
    background: transparent;
    border-radius: 4px;
    color: #ccc;
    font-family: inherit;
    font-size: 13px;
    text-align: left;
    cursor: pointer;
  }

  .sheet-caret-item:hover {
    background: rgba(255,255,255,0.08);
    color: #e0e0e0;
  }

  .sheet-caret-item:disabled {
    opacity: 0.5;
    cursor: default;
  }

  .sheet-caret-icon {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 16px;
    flex-shrink: 0;
    color: #A0A0A0;
  }

  .sheet-caret-icon--accent {
    color: #00C8C8;
  }

  /* ROW 2: Folder Column Headers (sticky inside content) */
  .grid-folder-toolbar {
    display: flex;
    position: sticky;
    top: 0;
    z-index: 5;
    height: 48px;
    min-height: 48px;
    align-items: center;
    border-bottom: 1px solid rgba(255,255,255,0.06);
    background: #0a0a0f;
  }

  .column-header-left,
  .column-header-right {
    flex: 1;
    padding: 12px 20px;
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 14px;
    font-weight: 600;
    color: #F0EDE8;
  }

  .column-header-left svg,
  .column-header-right svg {
    stroke: #F0EDE8;
  }

  .column-header-left {
    border-right: 1px solid rgba(255,255,255,0.08);
  }

  .column-count-badge {
    font-size: 12px;
    color: #666666;
    background: rgba(255,255,255,0.05);
    padding: 2px 8px;
    border-radius: 10px;
  }

  /* #2252: header narrows to the rail width when its column is collapsed — the "known
     trap" the scope audit flagged (flex:1 50/50 split does not auto-narrow on its own).
     Same transition timing as the column body below so header + body narrow in lockstep. */
  .column-header-left.collapsed,
  .column-header-right.collapsed {
    flex: none;
    width: var(--fv-rail);
    padding: 12px 0;
    justify-content: center;
    transition: width 0.15s ease;
  }

  .column-header-spacer {
    flex: 1;
  }

  /* Collapse/expand affordance in the column header (design: fv-collapse). */
  .fv-collapse {
    width: 28px;
    height: 28px;
    border-radius: 6px;
    background: transparent;
    border: 0;
    cursor: pointer;
    color: #666666;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    transition: background 0.12s ease, color 0.12s ease;
    flex-shrink: 0;
  }

  .fv-collapse:hover {
    background: rgba(255,255,255,0.06);
    color: #F0EDE8;
  }

  /* Columns wrapper: sizes from its children (Cabinet + Tasks columns). No explicit width —
     when PR C collapses a column to var(--fv-rail), the wrapper shrinks automatically. */
  .grid-folder-content {
    flex-shrink: 0;
    display: flex;
    flex-direction: column;
    overflow: hidden;
    border-right: 1px solid rgba(255,255,255,0.06);
    background: #0a0a0f;
  }

  .folder-columns {
    display: flex;
    flex: 1;
    overflow: hidden;
  }

  /* Fixed width per column via CSS var — never resizes or reflows (#2180). */
  /* #2252: PR C toggles width between var(--fv-col) and var(--fv-rail) via .collapsed. */
  .folder-column {
    width: var(--fv-col);
    flex-shrink: 0;
    display: flex;
    flex-direction: column;
    overflow: hidden;
    transition: width 0.15s ease;
  }

  .folder-column.collapsed {
    width: var(--fv-rail);
  }

  .folder-column:first-child {
    border-right: 1px solid rgba(255,255,255,0.08);
  }

  .folder-column.drag-over {
    background: rgba(0, 228, 228, 0.05);
    outline: 2px dashed rgba(0, 228, 228, 0.3);
    outline-offset: -2px;
  }

  /* ========== #2252: Collapsed column → slim rail ==========
     Ported from the design file (folder-view.css .fv-railcol*), NOT the doneCollapsed
     icon-only-stub pattern — this rail carries icon + vertical label + count + expand
     chevron, and its collapsed state persists (doneCollapsed does neither). */
  .fv-railcol {
    width: 100%;
    height: 100%;
    flex: 1;
    box-sizing: border-box;
    border: 0;
    background: transparent;
    display: flex;
    flex-direction: column;
    align-items: center;
    padding: 12px 0 14px;
    cursor: pointer;
    transition: background 0.12s ease;
    font-family: inherit;
  }

  .fv-railcol:hover {
    background: rgba(255,255,255,0.04);
  }

  .fv-railcol__expand {
    width: 28px;
    height: 28px;
    border-radius: 6px;
    display: flex;
    align-items: center;
    justify-content: center;
    color: #666666;
  }

  .fv-railcol:hover .fv-railcol__expand {
    color: #F0EDE8;
  }

  .fv-railcol__icon {
    color: #A0A0A0;
    margin: 6px 0 12px;
    display: flex;
  }

  .fv-railcol__label {
    writing-mode: vertical-rl;
    text-orientation: mixed;
    font-size: 12px;
    font-weight: 600;
    letter-spacing: 0.1em;
    text-transform: uppercase;
    color: #A0A0A0;
  }

  .fv-railcol__count {
    margin-top: 12px;
    min-width: 20px;
    height: 20px;
    padding: 0 5px;
    box-sizing: border-box;
    border-radius: 999px;
    background: rgba(255,255,255,0.08);
    color: #A0A0A0;
    font-size: 10.5px;
    font-weight: 600;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    font-variant-numeric: tabular-nums;
  }

  .folder-card-list {
    flex: 1;
    overflow-y: auto;
    padding: 12px;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  /* #2334 (container-level relocation): .cabinet-drag-wrap is now just the drag
     SOURCE wrapper (card-sized, for ondragend) — no drop-target styling here
     anymore. The insertion-point indicator is .cabinet-drop-indicator below,
     rendered by the list container at the coordinate-resolved slot, not by a
     per-card border. */
  .cabinet-drag-wrap {
    /* intentionally minimal — drag source only, see comment above */
  }

  /* #2334: coordinate-resolved insertion-point indicator, rendered by the list
     container (.folder-card-list) at cabinetDropIndicatorIndex — same source of
     truth as the drop-index resolution itself (dragReorderIndex.ts). Distinct
     from the column-level .folder-column.drag-over status-change affordance. */
  .cabinet-drop-indicator {
    height: 2px;
    margin: -1px 0;
    background: #00E4E4;
    border-radius: 1px;
  }

  .folder-card-list::-webkit-scrollbar {
    width: 6px;
  }

  .folder-card-list::-webkit-scrollbar-track {
    background: transparent;
  }

  .folder-card-list::-webkit-scrollbar-thumb {
    background: rgba(255,255,255,0.08);
    border-radius: 3px;
  }

  .task-group {
    margin-bottom: 8px;
  }

  .group-header-text {
    padding: 8px 12px 8px 8px;
    font-size: 11px;
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 0.08em;
    color: #666666;
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .group-header-text.todo {
    color: #A0A0A0;
  }

  .group-header-text.in-progress {
    color: #f59e0b;
  }

  .group-header-text.review {
    color: #2196F3;
  }

  .group-header-text.done {
    color: #10b981;
  }

  .group-dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
  }

  .group-dot.todo {
    background: #A0A0A0;
  }

  .group-dot.in-progress {
    background: #f59e0b;
  }

  .group-dot.review {
    background: #2196F3;
  }

  .group-dot.done {
    background: #10b981;
  }

  .group-divider-line {
    height: 1px;
    background: rgba(255,255,255,0.05);
    margin: 12px 8px;
  }

  /* .grid-editor-content removed (#2180 PR A): editor-in-col-3 model replaced by note overlay (PR B). */


  .project-aggregate-view {
    height: 100%;
    display: flex;
    flex-direction: column;
    background: #0a0a0f;
    color: #e0e0e0;
    font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Text', 'Segoe UI', sans-serif;
  }

  .project-header {
    flex-shrink: 0;
    border-bottom: 1px solid rgba(255,255,255,0.06);
    padding: 16px 24px;
    background: #0a0a0f;
  }

  .new-note-btn {
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 6px 12px;
    border: none;
    border-radius: 4px;
    background: #00E4E4;
    color: #0a0a0f;
    font-size: 12px;
    font-weight: 600;
    cursor: pointer;
    transition: all 0.15s;
  }
  .new-note-btn:hover {
    background: #00C8C8;
  }
  .header-top {
    display: flex;
    justify-content: space-between;
    align-items: center;
  }
  .header-top h2 {
    font-size: 20px;
    font-weight: 600;
    color: #e0e0e0;
    margin: 0 0 16px 0;
  }







  .content-layout {
    position: relative;
    display: flex;
    flex: 1;
    min-height: 0;
    min-width: 0;
  }

  .project-split-layout {
    display: flex;
    height: 100%;
    overflow: hidden;
    flex: 1;
  }

  .project-list-panel {
    min-width: 240px;
    max-width: 600px;
    border-right: 1px solid rgba(255,255,255,0.06);
    overflow-y: auto;
    display: flex;
    flex-direction: column;
  }

  .resize-handle {
    width: 4px;
    cursor: col-resize;
    background: transparent;
    flex-shrink: 0;
    transition: background 0.15s;
  }
  .resize-handle:hover { background: #00E4E4; }
  .editor-panel {
    position: relative;
    position: relative;
    flex: 1;
    overflow-y: auto;
    background: #0a0a0f;
  }

  .notes-list {
    display: flex;
    flex-direction: column;
    gap: 10px;
    padding: 0;
    overflow-y: auto;
    flex: 0 1 auto;
  }

  .folder-section {
    margin-bottom: 4px;
  }

  .section-header {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 12px;
    cursor: pointer;
    color: #888888;
  }

  .section-header:hover {
    background: #1a1a1c;
  }

  .section-dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    flex-shrink: 0;
  }

  .section-title {
    font-size: 12px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.5px;
    margin: 0;
    color: #666666;
  }

  .section-count {
    font-size: 11px;
    color: #52525B;
    margin-left: auto;
  }

  .section-notes {
    display: flex;
    flex-direction: column;
    gap: 10px;
    padding: 0 4px;
  }


  .chat-sidebar-container {
    width: 320px;
    flex-shrink: 0;
    border-left: 1px solid rgba(255,255,255,0.06);
  }
  .kanban-toggle {
    padding: 5px 12px;
    border: 1px solid rgba(255,255,255,0.08);
    border-radius: 4px;
    background: rgba(255,255,255,0.06);
    color: #888888;
    font-size: 11px;
    font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Text', 'Segoe UI', sans-serif;
    cursor: pointer;
    margin-left: 8px;
    white-space: nowrap;
    width: 82px;
    text-align: center;
  }
  .kanban-toggle:hover {
    background: #333;
    color: #e0e0e0;
    border-color: #00E4E4;
  }

  /* Split pane layout styles */
  .split-pane-container {
    display: flex;
    flex: 1;
    min-height: 0;
    min-width: 0;
    overflow: hidden;
  }

  .card-list-panel {
    flex-shrink: 0;
    display: flex;
    flex-direction: column;
    height: 100%;
    overflow: hidden;
    border-right: 1px solid rgba(255,255,255,0.06);
    background: #0a0a0f;
    transition: width 200ms ease, min-width 200ms ease;
    min-width: 280px;
  }


  .card-header {
    padding: 16px 14px 10px;
    font-size: 14px;
    font-weight: 600;
    color: rgba(255,255,255,0.7);
    border-bottom: 1px solid rgba(255,255,255,0.06);
    display: flex;
    align-items: center;
    justify-content: space-between;
    flex-shrink: 0;
  }

  .collapse-btn {
    width: 20px;
    height: 20px;
    border-radius: 3px;
    background: rgba(255,255,255,0.06);
    display: flex;
    align-items: center;
    justify-content: center;
    cursor: pointer;
    font-size: 10px;
    color: rgba(255,255,255,0.4);
    border: none;
  }
  .collapse-btn:hover {
    background: rgba(255,255,255,0.1);
    color: rgba(255,255,255,0.6);
  }

  .card-list-inner {
    padding: 6px;
    display: flex;
    flex-direction: column;
    gap: 4px;
    flex: 1;
    overflow-y: auto;
  }
  .card-list-resize-handle {
    width: 4px;
    cursor: col-resize;
    background: transparent;
    flex-shrink: 0;
    transition: background 0.15s ease;
  }
  .card-list-resize-handle:hover {
    background: rgba(0, 228, 228, 0.3);
  }
  .card-list-resize-handle.dragging {
    background: rgba(0, 228, 228, 0.5);
  }

  .card-list-collapsed-toggle {
    flex-shrink: 0;
    width: 28px;
    display: flex;
    align-items: flex-start;
    justify-content: center;
    padding-top: 12px;
    background: #0a0a0f;
    border-right: 1px solid rgba(255,255,255,0.06);
  }

  .card-list-expand-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 22px;
    height: 22px;
    border: none;
    background: transparent;
    color: #666666;
    cursor: pointer;
    transition: color 0.15s ease;
    padding: 0;
  }
  .card-list-expand-btn:hover {
    color: #00E4E4;
  }

  .card-list-collapse-btn {
    position: absolute;
    top: 8px;
    left: 8px;
    display: flex;
    align-items: center;
    justify-content: center;
    width: 22px;
    height: 22px;
    border: none;
    background: transparent;
    color: #666666;
    cursor: pointer;
    transition: color 0.15s ease;
    padding: 0;
    z-index: 5;
  }
  .card-list-collapse-btn:hover {
    color: #00E4E4;
  }

  .split-editor-panel {
    position: relative;
    flex: 1;
    display: flex;
    flex-direction: column;
    min-width: 300px;
    overflow: hidden;
  }

  .split-editor-body {
    flex: 1;
    overflow-y: auto;
    min-height: 0;
  }

  /* ===== CARD DESIGN v3 ===== */
  .pane-card {
    position: relative;
    background: rgba(255,255,255,0.05);
    cursor: pointer;
    transition: all 0.15s ease;
    overflow: hidden;
    border-left: 3px solid transparent;
    /* Wide mode (default) */
    height: 120px;
    border-radius: 6px;
    padding: 10px 10px 8px 10px;
    display: grid;
    grid-template-rows: 18px 1fr 16px;
  }

  .column-content .pane-card {
    flex-shrink: 0;
    height: 120px;
    min-height: 120px;
  }

  .pane-card:hover {
    background: rgba(255,255,255,0.07);
  }

  .pane-card.active {
    background: rgba(0,228,228,0.06);
  }

  .card-row {
    display: flex;
    align-items: center;
    gap: 4px;
    min-width: 0;
  }

  .card-mid {
    display: flex;
    flex-direction: column;
    justify-content: center;
    gap: 1px;
    min-width: 0;
    overflow: hidden;
  }

  /* card-row-bot merged into card-row */ .card-row-bot-unused {
    display: flex;
    align-items: center;
    gap: 4px;
    min-width: 0;
  }

  .card-title {
    font-size: 13px;
    font-weight: 600;
    color: #fff;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    min-width: 0;
  }

  .spacer { flex: 1; }

  .pill-priority {
    font-size: 10px;
    font-weight: 600;
    padding: 1px 6px;
    border-radius: 3px;
    white-space: nowrap;
    flex-shrink: 0;
    text-transform: uppercase;
    letter-spacing: 0.3px;
    background: rgba(255,255,255,0.1);
    color: rgba(255,255,255,0.3);
  }
  .pill-priority.urgent { background: rgba(255,68,68,0.2); color: #FF4444; }
  .pill-priority.high { background: rgba(255,140,0,0.2); color: #FF8C00; }
  .pill-priority.medium { background: rgba(255,214,0,0.2); color: #FFD600; }
  .pill-priority.low { background: rgba(255,255,255,0.1); color: rgba(255,255,255,0.3); }

  .card-folder {
    font-size: 10px;
    color: rgba(255,255,255,0.25);
    white-space: nowrap;
    flex-shrink: 0;
  }

  .card-preview {
    font-size: 11px;
    color: rgba(255,255,255,0.4);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    line-height: 1.3;
  }

  .pill-assignee {
    font-size: 10px;
    padding: 1px 6px;
    border-radius: 3px;
    border: 1px solid rgba(0,228,228,0.4);
    color: #00E4E4;
    white-space: nowrap;
    flex-shrink: 0;
  }

  .card-due {
    font-size: 10px;
    color: rgba(255,255,255,0.35);
    white-space: nowrap;
  }



  .hidden { visibility: hidden; }

  


  .split-editor-empty {
    flex: 1;
    display: flex;
    align-items: center;
    justify-content: center;
  }
  /* .empty-text removed (#2180 PR B): no HTML referent after PR A removed .editor-empty-state */

  /* ===== FOLDER VIEW NOTE OVERLAY (#2180 PR B) ===== */
  /* Resizable panel that renders OVER the fixed columns. Columns stay fully lit underneath. */
  /* No scrim/backdrop. Dismiss: ✕ button in panel or Esc key. */
  /* position:relative on .fv-stage (PR A) is the containing block for this absolute element. */
  .fv-note {
    position: absolute;
    top: 0;
    bottom: 0;
    /* left is set inline via noteLeft state */
    right: 0;
    z-index: 21;
    background: #0a0a0f;
    border-left: 1px solid rgba(255,255,255,0.1);
    box-shadow: -20px 0 44px rgba(0,0,0,0.45);
    display: flex;
    flex-direction: column;
    overflow: hidden;
    min-width: 0;
  }

  /* Editor fills the note panel. Override any background the Editor applies. */
  .fv-note :global(.editor-wrapper),
  .fv-note :global(.editor-container),
  .fv-note :global(.editor-header) {
    background: #0a0a0f !important;
  }

  /* Drag-to-resize handle on the panel's left edge. */
  /* Same visual idiom as the folder-view note resize handle (.fv-note__resize itself). */
  .fv-note__resize {
    position: absolute;
    top: 0;
    bottom: 0;
    left: -3px;
    width: 6px;
    cursor: col-resize;
    z-index: 22;
    background: transparent;
    transition: background 0.15s ease;
  }
  .fv-note__resize:hover {
    background: rgba(0, 228, 228, 0.3);
  }

  /* Close button (✕) — positioned absolute in top-right corner of the panel. */
  .fv-note__close {
    position: absolute;
    top: 8px;
    right: 10px;
    background: transparent;
    border: none;
    color: #666666;
    font-size: 16px;
    cursor: pointer;
    z-index: 23;
    padding: 4px 8px;
    border-radius: 4px;
    line-height: 1;
  }
  .fv-note__close:hover {
    color: #e0e0e0;
    background: rgba(255,255,255,0.06);
  }



  /* Unified Context Menu Styles */
  .context-overlay {
    position: fixed;
    top: 0;
    left: 0;
    right: 0;
    bottom: 0;
    z-index: 999;
  }

  .context-menu {
    position: fixed;
    background: #1a1a1f;
    border: 1px solid rgba(255,255,255,0.12);
    border-radius: 4px;
    padding: 4px;
    min-width: 180px;
    max-width: 240px;
    max-height: 320px;
    overflow-y: auto;
    z-index: 1000;
    box-shadow: 0 4px 20px rgba(0,0,0,0.6);
  }

  .context-item {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    padding: 7px 10px;
    background: none;
    border: none;
    color: #ccc;
    font-size: 13px;
    text-align: left;
    cursor: pointer;
    border-radius: 3px;
  }

  .context-item:hover {
    background: rgba(255,255,255,0.08);
    color: #e0e0e0;
  }

  .context-item.back {
    color: #888;
    font-size: 12px;
  }

  .context-item.back:hover {
    color: #ccc;
  }

  .context-item.delete {
    color: #EF4444;
  }

  .context-item.delete:hover {
    background: rgba(239,68,68,0.1);
  }

  .context-divider {
    height: 1px;
    background: rgba(255,255,255,0.08);
    margin: 4px 0;
  }

  .context-section-label {
    padding: 4px 10px;
    font-size: 11px;
    color: #666;
    text-transform: uppercase;
    letter-spacing: 0.5px;
  }
</style>
