<script lang="ts">
  import { onMount, onDestroy, untrack } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { activeView, activeTaskId } from '$lib/stores/app';
  import type { Task, TaskStatus, Project } from '$lib/types';
  import * as api from '$lib/api';
  import { resolveDropIndex, normalizeInsertIndex, cardRectsFromElements, shouldShowIndicator } from '$lib/utils/dragReorderIndex';
  import Card from './Card.svelte';
  import { normalizeTask } from '$lib/utils/cardNormalizers';
  import { getStageColor } from '$lib/utils/stageColors';
  import TaskContextMenu from './TaskContextMenu.svelte';
  import ConfirmDialog from './ConfirmDialog.svelte';

  import Editor from './Editor.svelte';
  import * as noteService from '$lib/services/noteService';
  import * as entityService from '$lib/services/entityService';
  import { navigateWikiLink } from '$lib/utils/wikiLinkNavigation';
  import { activeNote } from '$lib/stores/app';
  import { get } from 'svelte/store';
  import StatusBar from './StatusBar.svelte';
  import { canvasState } from '$lib/stores/canvas';
  import { resolveHeaderButtonMode, type HeaderButtonMode } from '$lib/utils/newSheetButtonLayout';
  import { noteMetaToTask } from '$lib/utils/cardNormalizers';
  import {
    ENTITY_NOTE,
    ENTITY_SHEET,
    REASON_DELETED,
    subscribeDataChanged,
    type DataChangedPayload,
  } from '$lib/refreshEvent';

  // probe: instrument all $activeNote write sites — investigating #2214 delete-close latch
  const DEBUG = typeof localStorage !== 'undefined' && localStorage.getItem('pane-debug') === '1';
  function logAnSet(caller: string, newVal: unknown): void {
    if (!DEBUG) return;
    const oldId = get(activeNote)?.meta?.id ?? 'null';
    const newId = (newVal as any)?.meta?.id ?? 'null';
    console.log(`[PANE-DEBUG] $activeNote SET in ${caller} | old="${oldId}" new="${newId}"`);
    console.log('[PANE-DEBUG] stack:', new Error().stack?.split('\n').slice(1, 5).join('\n'));
  }

  let tasks: Task[] = $state([]);
  let folderNameMap: Map<string, string> = $state(new Map());
  // #2339 PR 15a: card-face summaries (col/row/formula counts + col types)
  // for whichever visible tasks are sheet notes, keyed by note id. Fetched
  // in one batched call per loadTasks() pass — see cardSummaryFor() below,
  // the single seam every <Card> call site in this file reads through.
  let sheetSummaries: Map<string, import('$lib/types').SheetCardSummary> = $state(new Map());

  function cardSummaryFor(task: Task) {
    if (task.note_type !== 'sheet') return undefined;
    const s = sheetSummaries.get(task.id);
    if (!s) return undefined;
    return { colCount: s.col_count, rowCount: s.row_count, formulaCount: s.formula_count, colTypes: s.col_types };
  }

  let contextMenuTask: Task | null = $state(null);
  let contextMenuPos: { x: number; y: number } | null = $state(null);
  let deleteConfirmTask: Task | null = $state(null);

  let loading = $state(false);
  let error = $state('');
  let draggedTask: Task | null = $state(null);
  let dragOverColumn: TaskStatus | null = $state(null);

  // #2334 (container-level relocation): Cabinet reorder-drag state. Distinct from
  // draggedTask/dragOverColumn (the pre-existing status-change drag path) so the
  // event-propagation contract can distinguish "drag originated on a Cabinet card"
  // (reorder) from "drag originated on a task card" (status-change) for THIS
  // specific gesture. handleCardDragStart sets both this AND draggedTask so a
  // Cabinet-card drag that gets dropped outside a card (into empty Cabinet column
  // space, or another column entirely) still flows correctly through the existing
  // handleDrop status-change path.
  // cabinetDropIndicatorIndex is the coordinate-resolved insertion slot (0..N),
  // shared by both the drop-indicator render and the actual drop resolution — one
  // source of truth for "where will this land" (see dragReorderIndex.ts).
  let cabinetDragSourceId: string | null = $state(null);
  let cabinetDropIndicatorIndex: number | null = $state(null);
  let cabinetColumnContentEl: HTMLDivElement | undefined = $state(undefined);

  // #2874 item F: holds a noteId from a pane-select-note dispatch (Calendar
  // click → CanvasRenderer.handleTaskClick, same event NotesBrowser already
  // listens for) that arrived before `tasks` had finished loading. Resolved
  // by the $effect below once `tasks` transitions from empty to populated —
  // same completion-signal pattern as NotesBrowser's own pendingSelectNoteId.
  let pendingSelectTaskId: string | null = $state(null);

  // #2874 Kanban collapse: every column can be collapsed, not just Done or
  // columns over 5 cards (Matt's ruling 1). Done still defaults collapsed
  // (ruling 3) when no persisted state exists yet for this project.
  let collapsedColumns = $state(new Set<TaskStatus>(['done'])); // overridden per-project in the $effect below once projectId is known

  // #2874 Kanban collapse persistence (Matt's ruling 2: survives a restart,
  // per project). Reuses the SAME mechanism `docs/spec/ui-layout.md`'s
  // "Column Collapse (Rails)" § "Persistence (SQLite, per-folder)" already
  // documents and `NotesBrowser.svelte`'s cabinet/tasks collapse rails
  // already implement for a near-identical per-scope column-collapse
  // feature: the generic `ai_settings` KV table via the existing
  // `get_ai_setting`/`set_ai_setting` Tauri commands (src-tauri/src/notes.rs,
  // already registered in lib.rs) — zero new Rust commands, zero new
  // migrations. That spec section explicitly documents Matt's ruling that a
  // PRIOR localStorage-based stub for this exact kind of state was rejected
  // in favor of SQLite; `ai_settings` is deliberately per-device (not in
  // `CRR_TABLES`), which is correct here too — collapse state is a local UI
  // preference, not synced data. Key format mirrors the folder-view's
  // `folder:{scopeId}:{column}:collapsed`: `kanban:{projectId}:{status}:collapsed`,
  // values are the literal strings 'true'/'false' (same convention).
  const KANBAN_COLUMN_STATUSES: TaskStatus[] = ['cabinet', 'todo', 'in_progress', 'review', 'done'];

  function kanbanCollapseKey(projectId: string, status: TaskStatus): string {
    return `kanban:${projectId}:${status}:collapsed`;
  }

  // Monotonic counter guarding the async get_ai_setting reads below — mirrors
  // NotesBrowser.svelte's collapseLoadSeq (#2335): if two project switches
  // fire in quick succession, an older/slower resolve must not overwrite
  // newer project's collapse state.
  let collapseLoadSeq = 0;

  async function loadCollapsedColumns(projectId: string): Promise<void> {
    const mySeq = ++collapseLoadSeq;
    try {
      const results = await Promise.all(
        KANBAN_COLUMN_STATUSES.map(status =>
          invoke<string | null>('get_ai_setting', { key: kanbanCollapseKey(projectId, status) })
        )
      );
      if (mySeq !== collapseLoadSeq) return; // superseded — discard stale resolve
      const next = new Set<TaskStatus>();
      KANBAN_COLUMN_STATUSES.forEach((status, i) => {
        const val = results[i];
        // Missing key (first-ever visit to this project) defaults per Matt's
        // ruling 3: Done collapsed, everything else expanded.
        const collapsed = val === null ? status === 'done' : val === 'true';
        if (collapsed) next.add(status);
      });
      collapsedColumns = next;
    } catch (e) {
      if (mySeq !== collapseLoadSeq) return; // superseded — discard stale resolve
      console.error('[KANBAN-COLLAPSE] Failed to load collapse state for project', projectId, ':', e);
      // Fail open to the documented default rather than leaving stale state
      // from a previous project.
      collapsedColumns = new Set<TaskStatus>(['done']);
    }
  }

  async function saveCollapsedColumn(projectId: string, status: TaskStatus, collapsed: boolean): Promise<void> {
    try {
      await invoke('set_ai_setting', {
        key: kanbanCollapseKey(projectId, status),
        value: String(collapsed),
      });
    } catch (e) {
      console.error('[KANBAN-COLLAPSE] Failed to save collapse state for project', projectId, 'status', status, ':', e);
    }
  }

  // Editor resize
  let kanbanEditorWidth = $state(
    parseInt(localStorage.getItem('pane-project-kanban-editor-width') || '600')
  );
  // #2339 PR9 ruling 2: sheets persist overlay width under their OWN
  // localStorage key, separate from the note-width key above, defaulting
  // to 1000px (README ruling 4). Notes' key/semantics are untouched.
  let kanbanSheetWidth = $state(
    parseInt(localStorage.getItem('pane-project-kanban-editor-width-sheet') || '1000')
  );
  let splitViewRef: HTMLElement | undefined = $state();

  // #2331: guards handleColumnsClick (below) against a synthetic click landing outside
  // .kanban-editor-panel after a resize-drag ends with the pointer off the panel.
  // Prophylactic — per the scope audit, Kanban's DOM (flex siblings under
  // .kanban-split-view, not a shared-ancestor-with-listener like folder view's .fv-stage)
  // may not reproduce #2331 via the same synthetic-click pathway, but the guard is applied
  // uniformly to both views per Matt's ruling rather than relying on DOM-structure coincidence.
  let isEditorResizing = $state(false);

  function startEditorResize(e: MouseEvent) {
    e.preventDefault();
    // #2339 PR9 ruling 2: resize a separate width state/localStorage key when
    // the panel is showing a sheet note vs. a doc note — determined once at
    // drag-start from the currently active note, same as the panel's own
    // isActiveSheet check below.
    const isSheetResize = $activeNote?.meta?.note_type === 'sheet';
    // #2331: set BEFORE attaching mousemove/mouseup, same as folder view's startFvNoteResize.
    isEditorResizing = true;
    const startX = e.clientX;
    const startWidth = isSheetResize ? kanbanSheetWidth : kanbanEditorWidth;
    function onMouseMove(ev: MouseEvent) {
      const delta = startX - ev.clientX;
      // #2336: max is the full split-view width — the panel can grow to fill everything
      // right of the sidebar (the sidebar itself is a flex sibling outside
      // .kanban-split-view, so clientWidth here already excludes it; no -200 gutter reserved
      // for columns anymore — columns squeeze/scroll instead, which is the preferred behavior).
      const maxWidth = splitViewRef ? splitViewRef.clientWidth : 1200;
      const newWidth = Math.max(350, Math.min(maxWidth, startWidth + delta));
      if (isSheetResize) kanbanSheetWidth = newWidth;
      else kanbanEditorWidth = newWidth;
    }
    function onMouseUp() {
      if (isSheetResize) {
        localStorage.setItem('pane-project-kanban-editor-width-sheet', String(kanbanSheetWidth));
      } else {
        localStorage.setItem('pane-project-kanban-editor-width', String(kanbanEditorWidth));
      }
      document.removeEventListener('mousemove', onMouseMove);
      document.removeEventListener('mouseup', onMouseUp);
      document.body.style.cursor = '';
      document.body.style.userSelect = '';
      // #2331: deferred clear — same timing rationale as folder view's startFvNoteResize.
      // A synthetic click (if one fires) arrives after this handler returns but within the
      // same dispatch sequence; setTimeout(..., 0) defers the clear past that point.
      setTimeout(() => { isEditorResizing = false; }, 0);
    }
    document.addEventListener('mousemove', onMouseMove);
    document.addEventListener('mouseup', onMouseUp);
    document.body.style.cursor = 'col-resize';
    document.body.style.userSelect = 'none';
  }

  const columns: { status: TaskStatus; title: string; color: string }[] = [
    { status: 'cabinet', title: 'Cabinet', color: getStageColor('cabinet') },
    { status: 'todo', title: 'To Do', color: getStageColor('todo') },
    { status: 'in_progress', title: 'In Progress', color: getStageColor('in_progress') },
    { status: 'review', title: 'Review', color: getStageColor('review') },
    { status: 'done', title: 'Done', color: getStageColor('done') }
  ];

  async function handleNewTaskEvent() {
    await handleCreateNewTask();
  }

  // handleTaskMetaChanged (the old `pane-task-meta-changed` partial-field
  // patch) deleted (#2972 stage 3, item 2 — ruling 4 folds it in): the
  // handler below already does a strict superset (full entity patch, plus
  // insert/remove) for the same `reason: "edited"` case.

  // #2972 stage 2 (§34.5, ruling 5's literal example — see §34): the board's
  // single pane-data-changed handler. Branches on entity_id/reason and
  // patches/inserts/removes exactly one card — never a full loadTasks()
  // reload. Replaces the old `window.addEventListener('pane-data-changed',
  // loadTasks)`/`('pane-note-created', loadTasks)` full-reload registrations
  // (§34.2's "Replaced" verdict for both). Delivered via Tauri's listen()
  // (not window.addEventListener) because this is the backend-emitted
  // channel — Tauri IPC is invisible to DOM listeners (#2215).
  async function handleDataChangedEvent(payload: DataChangedPayload) {
    const { entity_id, entity_type, reason } = payload;
    // The board only ever shows note/sheet entities as cards.
    if (entity_type !== ENTITY_NOTE && entity_type !== ENTITY_SHEET) return;

    if (reason === REASON_DELETED) {
      tasks = tasks.filter(t => t.id !== entity_id);
      return;
    }

    try {
      // Single-entity read (not a full loadTasks()) — §34.5's required
      // shape. `api.readNote()`, not `entityService.getNote()` — the raw
      // backend `NoteMeta` shape (`status`/`priority`/`due_date`) needs
      // `api.readNote()`'s existing task_status/task_priority/task_due_date
      // remap (api.ts's `readNote`) before `noteMetaToTask()` below can read
      // it; `entityService.getNote()` returns the unmapped raw shape.
      const noteData = await api.readNote('', entity_id);
      const projectId = $canvasState.context.projectId;
      const belongsHere = projectId ? noteData.meta.project_id === projectId : true;
      const idx = tasks.findIndex(t => t.id === entity_id);

      if (!belongsHere) {
        // Moved out of this board's project, or created elsewhere — remove
        // if present, otherwise nothing to do.
        if (idx !== -1) tasks = tasks.filter(t => t.id !== entity_id);
        return;
      }

      const patched = noteMetaToTask(noteData.meta);
      if (idx === -1) {
        // Not yet present — created, or moved into this board. Insert.
        tasks = [...tasks, patched];
        loadSheetSummaries().catch(() => {}); // best-effort, matches loadTasks()'s own non-blocking posture
      } else {
        // Already present — patch only this entry's changed fields, same
        // DOM node identity preserved by Svelte's keyed {#each ... (task.id)}.
        tasks = tasks.map(t => (t.id === entity_id ? { ...t, ...patched } : t));
      }
    } catch (e) {
      console.error('[KANBAN] Failed to patch/insert task for pane-data-changed event:', entity_id, e);
    }
  }

  function handleColumnsClick(e: MouseEvent) {
    // #2331: state-based guard — while an editor-panel resize-drag is in progress (or in
    // the deferred-clear window immediately after mouseup), swallow the click outright.
    // Prophylactic, mirrors folder view's .fv-stage guard — see isEditorResizing above.
    if (isEditorResizing) return;
    // Don't close if clicking on a card or the editor panel
    if ((e.target as HTMLElement).closest('.pane-card, .kanban-editor-panel')) return;
    logAnSet('KanbanBoard.handleColumnsClick', null);
    $activeNote = null;
    contextMenuTask = null;
    contextMenuPos = null;
  }

  function showTaskContextMenuById(e: MouseEvent, id: string) {
    const task = tasks.find(t => t.id === id);
    if (task) showTaskContextMenu(e, task);
  }

  function showTaskContextMenu(e: MouseEvent, task: Task) {
    contextMenuTask = task;
    contextMenuPos = { x: e.clientX, y: e.clientY };
  }

  function requestTaskDelete(task: Task) {
    contextMenuTask = null;
    contextMenuPos = null;
    deleteConfirmTask = task;
  }

  async function confirmTaskDelete() {
    if (!deleteConfirmTask) return;
    const id = deleteConfirmTask.id;
    deleteConfirmTask = null;
    tasks = tasks.filter(t => t.id !== id);
    try {
      await api.moveToTrash(id);
    } catch (e) {
      console.error('Failed to delete task:', e);
      await loadTasks();
    }
  }

  function handleKeydown(e: KeyboardEvent) {
    const meta = e.metaKey || e.ctrlKey;
    if (meta && e.key === 'n' && !e.shiftKey) {
      e.preventDefault();
      e.stopPropagation();
      handleCreateNewTask();
    }
  }

  // #2972 stage 3 (item 4): one shared helper hearing BOTH channels —
  // backend `listen()` AND the app `CustomEvent` — instead of Stage 2's
  // backend-only `listen()` registration. Resolves to the real unlisten
  // fn once registered; chained (not awaited) in onDestroy, same pattern
  // as Sidebar.svelte/AllTasksView.svelte/NotesBrowser.svelte's
  // registrations.
  let unlistenDataChanged: Promise<() => void> | null = null;

  // #2874 item F: resolves a queued pane-select-note request once `tasks`
  // transitions from empty to populated (loadTasks()'s async load has
  // completed) — same shape as NotesBrowser's own pendingSelectNoteId effect.
  $effect(() => {
    if (pendingSelectTaskId && tasks.length > 0) {
      const taskId = pendingSelectTaskId;
      const task = tasks.find(t => t.id === taskId);
      pendingSelectTaskId = null;
      if (task) handleNoteSelect(task);
    }
  });

  function handlePaneSelectNote(e: Event) {
    const noteId = (e as CustomEvent).detail?.noteId;
    if (!noteId) return;
    const task = tasks.find(t => t.id === noteId);
    if (task) {
      handleNoteSelect(task);
    } else {
      pendingSelectTaskId = noteId;
    }
  }

  onMount(() => {
    // #3044 delta-2 (Reed): used to be an unconditional `$activeNote = null`
    // (clearing a stale carry-over from whatever view was active before
    // Kanban mounted) — which ALSO wiped a note THIS SAME mount was
    // deliberately opening (Calendar opening a board card, or a wiki-link
    // click from Kanban's own editor panel navigating elsewhere and back).
    // Shared with NotesBrowser.svelte's onMount — see
    // noteService.clearStaleActiveNoteOnMount's own doc comment.
    noteService.clearStaleActiveNoteOnMount();
    window.addEventListener('pane-new-task', handleNewTaskEvent);
    window.addEventListener('keydown', handleKeydown);
    window.addEventListener('pane-select-note', handlePaneSelectNote);
    unlistenDataChanged = subscribeDataChanged((payload) => {
      handleDataChangedEvent(payload);
    });

    // #2339 PR 15c: header-width-based New Sheet/New Note collapse tracking.
    if (kanbanHeaderEl) {
      headerResizeObs = new ResizeObserver(entries => {
        for (const entry of entries) {
          kanbanHeaderWidth = entry.contentRect.width;
        }
      });
      headerResizeObs.observe(kanbanHeaderEl);
    }
  });

  $effect(() => {
    const projectId = $canvasState.context.projectId;
    if (projectId) {
      untrack(() => {
        void loadCollapsedColumns(projectId);
        loadTasks();
      });
    }
  });

  onDestroy(() => {
    window.removeEventListener('pane-new-task', handleNewTaskEvent);
    window.removeEventListener('keydown', handleKeydown);
    window.removeEventListener('pane-select-note', handlePaneSelectNote);
    unlistenDataChanged?.then(fn => fn());
    headerResizeObs?.disconnect();
  });

  async function handleNoteSelect(task: Task) {
    // Load note content and set activeNote directly (no navigation)
    try {
      const noteData = await entityService.getNote(task.id);
      // Transform entityService.NoteContent to app's NoteContent format
      const transformedNote: any = {
        meta: {
          id: noteData.meta.id,
          title: noteData.meta.title,
          slug: task.id,
          notebook: task.project || 'Inbox',
          preview: noteData.meta.content_preview || '',
          updated_at: noteData.meta.updated_at,
          created_at: noteData.meta.created_at,
          task_status: noteData.meta.status || task.status,
          task_priority: noteData.meta.priority || task.priority,
          task_due_date: noteData.meta.due_date || task.due_date,
          file_path: noteData.meta.file_path,
          project_id: noteData.meta.project_id,
          folder_id: noteData.meta.folder_id,
          tags: noteData.meta.tags,
          pinned: noteData.meta.pinned,
          word_count: noteData.meta.word_count,
          note_type: noteData.meta.note_type,
        },
        content: noteData.content,
        jsonContent: null,
      };
      logAnSet('KanbanBoard.handleNoteSelect', transformedNote);
      $activeNote = transformedNote;
    } catch (e) {
      console.error('Failed to load task note:', e);
    }
  }

  async function loadTasks() {
    loading = true;
    error = '';

    try {
      const projectId = $canvasState.context.projectId;
      if (projectId) {
        // Load tasks and folders independently so one failure does not blank
        // the entire board (fixes Promise.all fail-fast regression, issue #1935).
        const [tasksResult, foldersResult] = await Promise.allSettled([
          api.listNotesForKanban(projectId),
          api.listFolders(projectId),
        ]);

        if (tasksResult.status === 'fulfilled') {
          tasks = tasksResult.value;
        } else {
          console.error('Failed to load kanban tasks:', tasksResult.reason);
          error = 'Failed to load tasks. Please check if the database is properly initialized.';
          tasks = [];
        }

        if (foldersResult.status === 'fulfilled') {
          folderNameMap = new Map(foldersResult.value.map((f: Project) => [f.id, f.name]));
        } else {
          console.warn('Failed to load folders for kanban; folder names will be unavailable:', foldersResult.reason);
          folderNameMap = new Map();
        }
      } else {
        tasks = await api.listTasks();
        folderNameMap = new Map();
      }
      if (!error) error = '';
      await loadSheetSummaries();
    } catch (e) {
      console.error('Failed to load tasks:', e);
      error = 'Failed to load tasks. Please check if the database is properly initialized.';
      // Don't let this fail completely - provide empty tasks
      tasks = [];
    } finally {
      loading = false;
    }
  }

  // #2339 PR 15a: fetches card summaries for whichever loaded tasks are
  // sheet notes, one batched call. Best-effort — a failure here must not
  // block the board from rendering; sheet cards just fall back to no
  // micro-grid data (still render title/glyph/pills fine without it).
  async function loadSheetSummaries() {
    const sheetIds = tasks.filter(t => t.note_type === 'sheet').map(t => t.id);
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

  function getTasksForColumn(status: TaskStatus): Task[] {
    const filtered = tasks.filter(task => task.status === status);
    // Cabinet is a user-arranged shelf (drag-to-reorder, #2334) — must render
    // in sort_order order, not backend/array order. Task columns (todo/
    // in_progress/review/done) keep system ordering (priority/due_date) and
    // are intentionally left unsorted here.
    if (status === 'cabinet') {
      return filtered.sort((a, b) => (a.sort_order ?? 0) - (b.sort_order ?? 0) || (new Date(b.created_at).getTime() - new Date(a.created_at).getTime()));
    }
    return filtered;
  }

  function toggleColumnCollapse(status: TaskStatus) {
    const collapsed = !collapsedColumns.has(status);
    if (collapsed) {
      collapsedColumns.add(status);
    } else {
      collapsedColumns.delete(status);
    }
    collapsedColumns = new Set(collapsedColumns);
    // Write-immediately on toggle (single discrete click, no debounce needed
    // — same rationale as NotesBrowser.svelte's setColumnCollapsed).
    const projectId = $canvasState.context.projectId;
    if (projectId) void saveCollapsedColumn(projectId, status, collapsed);
  }

  function handleDragStart(id: string) {
    const task = tasks.find(t => t.id === id);
    if (task) draggedTask = task;
  }

  // ── Cabinet drag-reorder (#2334, relocated to container level) ──────────
  // The reorder drop handler lives on the cabinet column's .column-content
  // CONTAINER, not on per-card wrappers — the entire column surface, including
  // the seams between cards, is a valid reorder target. Drop-index resolution
  // and the drop indicator both derive from the same coordinate math
  // (dragReorderIndex.ts) — one source of truth for "where will this land".
  //
  // Mode disambiguation (relocated, not redesigned):
  //   - cabinetDragSourceId set  → drag originated on a Cabinet card → reorder,
  //     resolve drop index from pointer Y, call reorderNotes.
  //   - cabinetDragSourceId NOT set → existing status-change semantics, pass
  //     through to handleDrop.
  // The propagation contract from #2346 survives: stopPropagation() on the
  // container-level handler prevents handleDrop from also firing during a
  // reorder.
  //
  // The drag SOURCE stays per-card (handleCardDragStart, wired to Card's own
  // ondragstart via onDragStart) — only the DROP handling moved to the container.

  function handleCardDragStart(id: string, status: TaskStatus) {
    const task = tasks.find(t => t.id === id);
    if (!task) return;
    draggedTask = task;
    if (status === 'cabinet') cabinetDragSourceId = id;
  }

  function cabinetColumnDragOver(e: DragEvent) {
    e.preventDefault();
    if (!cabinetDragSourceId) return; // status-change drags use the column-level dragover highlight only
    e.stopPropagation();
    if (e.dataTransfer) e.dataTransfer.dropEffect = 'move';
    if (!cabinetColumnContentEl) return;
    const cabinetTasks = getTasksForColumn('cabinet');
    const rects = cardRectsFromElements(cabinetColumnContentEl.querySelectorAll('.cabinet-drag-wrap'));
    const rawDropIndex = resolveDropIndex(e.clientY, rects.length ? rects : cabinetTasks.map(t => ({ id: t.id, top: 0, height: 0 })));
    const sourceIdx = cabinetTasks.findIndex(t => t.id === cabinetDragSourceId);
    // #2334: suppress the indicator at the two identity-move slots (dropping
    // back into the dragged card's own slot, or the seam immediately below
    // it) so the UI never implies a move will happen when it won't.
    cabinetDropIndicatorIndex = shouldShowIndicator(sourceIdx, rawDropIndex) ? rawDropIndex : null;
  }

  function cabinetColumnDragLeave(e: DragEvent) {
    // Only clear when actually leaving the container (not moving between children).
    if (e.currentTarget instanceof HTMLElement && e.relatedTarget instanceof Node && e.currentTarget.contains(e.relatedTarget)) return;
    cabinetDropIndicatorIndex = null;
  }

  async function cabinetColumnDrop(e: DragEvent) {
    // Not a Cabinet-originated drag — let the event bubble to the column-level
    // ondrop={handleDrop} unmodified (existing status-change semantics, e.g. a
    // task-column card dropped into Cabinet).
    if (!cabinetDragSourceId) {
      return;
    }

    e.preventDefault();
    e.stopPropagation();
    // #3166 C52: `stopPropagation()` above means the column-level
    // `ondrop={handleDrop}` — the ONLY other place that clears
    // `dragOverColumn` — never fires for a Cabinet-internal reorder.
    // `dragOverColumn` is set unconditionally by `handleDragOver`/
    // `handleDragEnter` (neither checks `cabinetDragSourceId`), so
    // without this reset the Cabinet column stays stuck green after ANY
    // card reorder within it (image-thumbnail cards included — nothing
    // image-specific about the mechanism). Reset here, alongside the
    // other local-state resets below, so it covers every early `return`
    // in this function too (identity move, unresolved source/drop index),
    // not only the success path at the bottom.
    dragOverColumn = null;
    const dropIndicatorIndex = cabinetDropIndicatorIndex;
    cabinetDropIndicatorIndex = null;

    const sourceId = cabinetDragSourceId;
    cabinetDragSourceId = null;
    draggedTask = null;

    const cabinetTasks = getTasksForColumn('cabinet');
    const sourceIdx = cabinetTasks.findIndex(t => t.id === sourceId);
    if (sourceIdx === -1) return;

    let rawDropIndex = dropIndicatorIndex;
    if (rawDropIndex === null && cabinetColumnContentEl) {
      const rects = cardRectsFromElements(cabinetColumnContentEl.querySelectorAll('.cabinet-drag-wrap'));
      rawDropIndex = resolveDropIndex(e.clientY, rects.length ? rects : cabinetTasks.map(t => ({ id: t.id, top: 0, height: 0 })));
    }
    if (rawDropIndex === null) return;

    const insertIdx = normalizeInsertIndex(sourceIdx, rawDropIndex);
    if (insertIdx === sourceIdx) {
      console.debug('[Cabinet] no-op: identity move, sourceIdx=%d rawDropIndex=%d insertIdx=%d', sourceIdx, rawDropIndex, insertIdx);
      return; // dropped back in the same slot — no-op
    }

    const reordered = [...cabinetTasks];
    const [moved] = reordered.splice(sourceIdx, 1);
    reordered.splice(insertIdx, 0, moved);

    // Gap-based sort_orders (step 1000, matching the new-note-on-top gap scheme)
    // so future new-note inserts (COALESCE(MIN(sort_order), 0) - 1000) never collide.
    const withOrders = reordered.map((t, idx) => ({ ...t, sort_order: idx * 1000 }));
    const orderById = new Map(withOrders.map(t => [t.id, t.sort_order]));

    // Optimistic local update — preserves the new Cabinet order immediately.
    tasks = tasks.map(t => orderById.has(t.id) ? { ...t, sort_order: orderById.get(t.id)! } : t);

    try {
      await entityService.reorderNotes(withOrders.map(t => [t.id, t.sort_order] as [string, number]));
    } catch (err) {
      console.error('[Cabinet] reorder persist failed:', err);
      await loadTasks(); // Resync with server truth on failure
    }
  }

  function cabinetCardDragEnd() {
    cabinetDragSourceId = null;
    cabinetDropIndicatorIndex = null;
    draggedTask = null;
  }

  function handleDragOver(event: DragEvent, status: TaskStatus) {
    event.preventDefault();
    if (event.dataTransfer) {
      event.dataTransfer.dropEffect = 'move';
    }
    dragOverColumn = status;
  }

  async function handleDrop(event: DragEvent, targetStatus: TaskStatus) {
    event.preventDefault();
    dragOverColumn = null;

    if (!draggedTask || draggedTask.status === targetStatus) {
      if (draggedTask) {
        // Defect 2 (#2334 diagnosis): silent no-op guard — must be visible at debug level.
        console.debug('[Cabinet] drop: status unchanged, no-op', { noteId: draggedTask.id, from: draggedTask.status, to: targetStatus });
      }
      draggedTask = null;
      return;
    }

    const taskToMove = draggedTask;
    // Optimistic update
    tasks = tasks.map(t =>
      t.id === taskToMove.id
        ? { ...t, status: targetStatus, updated_at: new Date().toISOString() }
        : t
    );
    draggedTask = null;

    try {
      await api.updateTaskStatus(taskToMove.id, targetStatus);
    } catch (e) {
      console.error('Failed to update task status:', e);
      // Revert on error
      tasks = await api.listTasks();
    }
  }

  function handleDragEnter(event: DragEvent, status: TaskStatus) {
    event.preventDefault();
    dragOverColumn = status;
  }

  function handleDragLeave(event: DragEvent) {
    event.preventDefault();
  }

  async function handleCreateNewTask() {
    try {
      const projectId = $canvasState.context.projectId;
      if (!projectId) {
        console.error('No project context available');
        return;
      }

      // #2377: kanban-created notes go to the project default folder (General),
      // deterministically. Omitting folder_id triggers the backend auto-assign in
      // create_note_internal (notes.rs:129-148), which always resolves to the
      // project's General folder — no unsorted listFolders[0] needed.
      const newNoteMeta = await api.createNoteInProject(projectId, undefined, 'Untitled', 'cabinet');

      // Reload tasks to show the new note
      await loadTasks();

      // Load and open the new note in the editor
      const noteData = await entityService.getNote(newNoteMeta.id);
      const transformedNote: any = {
        meta: {
          id: noteData.meta.id,
          title: noteData.meta.title,
          slug: newNoteMeta.id,
          notebook: $canvasState.context.projectName || 'Inbox',
          preview: noteData.meta.content_preview || '',
          updated_at: noteData.meta.updated_at,
          created_at: noteData.meta.created_at,
          task_status: 'cabinet',
          task_priority: noteData.meta.priority,
          task_due_date: noteData.meta.due_date,
          file_path: noteData.meta.file_path,
          project_id: noteData.meta.project_id,
          folder_id: noteData.meta.folder_id,
          tags: noteData.meta.tags,
          pinned: noteData.meta.pinned,
          word_count: noteData.meta.word_count,
        },
        content: noteData.content,
        jsonContent: null,
      };
      logAnSet('KanbanBoard.handleCreateNewTask', transformedNote);
      $activeNote = transformedNote;
    } catch (e) {
      console.error('Failed to create new task:', e);
    }
  }

  // #2339 PR9 ruling 1: New Sheet button beside New Note. Calls create_sheet
  // (via api.createSheet) and opens the resulting sheet note directly in the
  // overlay — no cell editing wired up (PR 9 is read-only).
  let creatingSheet = $state(false);
  async function handleCreateNewSheet() {
    if (creatingSheet) return;
    creatingSheet = true;
    try {
      const projectId = $canvasState.context.projectId;
      if (!projectId) {
        console.error('No project context available');
        return;
      }
      const sheetData = await api.createSheet({
        project_id: projectId,
        title: 'Untitled Sheet',
        status: 'cabinet',
      });
      await loadTasks();
      const transformedNote: any = {
        meta: {
          id: sheetData.note_meta.id,
          title: sheetData.note_meta.title,
          slug: sheetData.note_meta.id,
          notebook: $canvasState.context.projectName || 'Inbox',
          preview: '',
          updated_at: sheetData.note_meta.updated_at,
          created_at: sheetData.note_meta.created_at,
          task_status: 'cabinet',
          task_priority: (sheetData.note_meta as any).priority,
          task_due_date: (sheetData.note_meta as any).due_date,
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
      logAnSet('KanbanBoard.handleCreateNewSheet', transformedNote);
      $activeNote = transformedNote;
    } catch (e) {
      console.error('Failed to create new sheet:', e);
    } finally {
      creatingSheet = false;
    }
  }

  // #2339 PR 15c — New Sheet button polish: three-step narrow collapse keyed to
  // the HOST HEADER's own rendered width, per docs/design/sheets/NOTES.md §A /
  // README.md "Addendum rulings" #7. Same ResizeObserver-on-own-wrapper pattern
  // as SessionDropdownHeader.svelte's containerWidth tracking.
  let kanbanHeaderEl: HTMLElement | null = $state(null);
  let kanbanHeaderWidth = $state(1200); // start wide to avoid a narrow-mode flash
  let headerResizeObs: ResizeObserver | null = null;
  let headerButtonMode: HeaderButtonMode = $derived(resolveHeaderButtonMode(kanbanHeaderWidth));

  // Caret popover (menu mode, header width < 720px). #2339 PR 15c collision
  // check: KanbanBoard's .kanban-header has no existing overflow/kebab menu
  // (grepped for kebab/overflow-menu/dots-btn/chevD-style affordances — none
  // found), so this caret is a genuinely new affordance, not a join. See PR body.
  let sheetCaretOpen = $state(false);

  function handleNewSheetFromCaret() {
    sheetCaretOpen = false;
    handleCreateNewSheet();
  }

  async function handleTaskUpdated(updatedTask: Task) {
    tasks = tasks.map(t => t.id === updatedTask.id ? updatedTask : t);
  }

  async function handleTaskDeleted(taskId: string) {
    tasks = tasks.filter(t => t.id !== taskId);
  }

  function handleNoteSelectById(id: string) {
    const task = tasks.find(t => t.id === id);
    if (task) handleNoteSelect(task);
  }

  // #3044 item 5: openTaskDetail deleted — proven unreachable (zero
  // callers anywhere in this file or elsewhere in src/, confirmed by
  // grep across the whole tree).

  // #2419/#3044: wiki-link click navigation from Kanban editor panel — open an
  // existing note. A missing note stays inert (no create, no confirm).
  async function handleKanbanWikiLinkNavigation(title: string) {
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
      console.error('[handleKanbanWikiLinkNavigation] Failed to navigate from wiki link:', e);
    }
  }

  // Column icons removed - using colored dots instead
</script>

<div class="kanban-board">
  <div class="kanban-header" bind:this={kanbanHeaderEl}>
    <div class="header-left">
      <button
        class="back-btn"
        onclick={() => $activeView = 'notes'}
        title="Back to Notebooks"
      >
        <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
          <path d="m15 18-6-6 6-6"/>
        </svg>
      </button>
      <h2>{$canvasState.context.projectName || 'Tasks'}</h2>
    </div>
    <div class="kanban-actions">
      <!-- #2339 PR 15c: New Sheet sits LEFT of New Note (New Note stays rightmost
           and stays the labelled/primary action at every header width), per
           sheets-addendum.jsx's HostHeader ordering and its own annotation
           ("New Note stays primary and stays rightmost... New Sheet sits to its
           left, so the primary button never moves from where people reach for it"). -->
      {#if headerButtonMode !== 'menu'}
        <button
          class="add-sheet-btn btn--outline"
          class:add-sheet-btn--icon-only={headerButtonMode === 'icon'}
          onclick={handleCreateNewSheet}
          disabled={creatingSheet}
          title="New Sheet"
        >
          <svg width="14" height="14" viewBox="0 0 14 14" fill="none" stroke="currentColor">
            <path d="M7 2V12M2 7H12" stroke-width="1.5" stroke-linecap="round"/>
          </svg>
          {#if headerButtonMode === 'full'}New Sheet{/if}
        </button>
      {/if}
      <button
        class="add-task-btn"
        onclick={handleCreateNewTask}
        title="New Note"
      >
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
              <button class="sheet-caret-item" onclick={handleNewSheetFromCaret} disabled={creatingSheet}>
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
  </div>



  <div class="kanban-split-view" bind:this={splitViewRef}>
    <div class="kanban-left-panel" onclick={handleColumnsClick}>
    {#if loading}
      <div class="loading">Loading tasks...</div>
    {:else if error}
      <div class="error">{error}</div>
    {:else}
    <div class="kanban-columns">
      {#each columns as column}
        <div
          class="kanban-column"
          class:collapsed={collapsedColumns.has(column.status)}
          class:drag-target={dragOverColumn === column.status && (draggedTask?.status !== column.status || column.status === 'cabinet')}
          role="region"
          aria-label={column.title}
          ondragover={(e) => handleDragOver(e, column.status)}
          ondragenter={(e) => handleDragEnter(e, column.status)}
          ondragleave={handleDragLeave}
          ondrop={(e) => handleDrop(e, column.status)}
        >
          <div 
            class="column-header" 
            onclick={() => toggleColumnCollapse(column.status)}
            style:cursor="pointer"
            title={collapsedColumns.has(column.status) ? 'Expand column' : 'Collapse column'}
          >
            <span class="column-dot" style:background={column.color}></span>
            <span class="column-title">{column.title}</span>
            <span class="column-count">{getTasksForColumn(column.status).length}</span>
            <span class="collapse-indicator">
              {collapsedColumns.has(column.status) ? '▶' : '▼'}
            </span>
          </div>

          {#if !collapsedColumns.has(column.status)}
            {#if column.status === 'cabinet'}
              <!-- #2334 (container-level relocation): the reorder drop handler lives
                   HERE, on .column-content — not on per-card wrappers. The entire
                   column surface, including the seams/gaps between cards, is a
                   valid reorder target. Mode disambiguation (cabinetDragSourceId
                   set → reorder, unset → falls through to the column-level
                   handleDrop) happens in cabinetColumnDrop. -->
              <div
                class="column-content"
                bind:this={cabinetColumnContentEl}
                ondragover={cabinetColumnDragOver}
                ondragleave={cabinetColumnDragLeave}
                ondrop={cabinetColumnDrop}
              >
                {#if cabinetDropIndicatorIndex === 0}
                  <div class="cabinet-drop-indicator"></div>
                {/if}
                {#each getTasksForColumn('cabinet') as task, i (task.id)}
                  <!-- #2334: drag SOURCE stays per-card (handleCardDragStart via
                       Card's ondragstart) — only drop handling moved to the
                       container above. No per-card ondragover/ondrop/ondragleave
                       here anymore. -->
                  <div class="cabinet-drag-wrap" data-cabinet-id={task.id} ondragend={cabinetCardDragEnd}>
                    <Card
                      data={normalizeTask(task, $canvasState.context.projectName || '', folderNameMap, cardSummaryFor(task))}
                      onDragStart={(id) => handleCardDragStart(id, column.status)}
                      onSelect={(id) => handleNoteSelectById(id)}
                      onContextMenu={(e, id) => showTaskContextMenuById(e, id)}
                    />
                  </div>
                  {#if cabinetDropIndicatorIndex === i + 1}
                    <div class="cabinet-drop-indicator"></div>
                  {/if}
                {/each}

                {#if getTasksForColumn('cabinet').length === 0}
                  <div class="empty-column">
                    No {column.title.toLowerCase()} tasks
                  </div>
                {/if}
              </div>
            {:else}
            <div class="column-content">
              {#each getTasksForColumn(column.status) as task (task.id)}
                  <Card
                    data={normalizeTask(task, $canvasState.context.projectName || '', folderNameMap, cardSummaryFor(task))}
                    onDragStart={(id) => handleDragStart(id)}
                    onSelect={(id) => handleNoteSelectById(id)}
                    onContextMenu={(e, id) => showTaskContextMenuById(e, id)}
                  />
              {/each}

              {#if getTasksForColumn(column.status).length === 0}
                <div class="empty-column">
                  No {column.title.toLowerCase()} tasks
                </div>
              {/if}
            </div>
            {/if}
          {:else}
            <div class="column-collapsed">
              {#if getTasksForColumn(column.status).length > 0}
                <div class="collapsed-summary">
                  {getTasksForColumn(column.status).length} tasks
                </div>
              {:else}
                <div class="collapsed-summary empty">
                  No tasks
                </div>
              {/if}
            </div>
          {/if}
        </div>
      {/each}
    </div>
    {/if}
    </div>
    {#if $activeNote}
      {@const note = $activeNote}
      {@const isActiveSheet = note.meta?.note_type === 'sheet'}
      <div class="kanban-editor-panel" style:width="{isActiveSheet ? kanbanSheetWidth : kanbanEditorWidth}px">
        <div
          class="kanban-editor-resize"
          onmousedown={startEditorResize}
          role="separator"
          aria-orientation="vertical"
          tabindex="-1"
        ></div>
        {#key note.meta.id}
          <Editor
            content={note.content}
            title={note.meta.title}
            noteId={note.meta.id}
            slug={note.meta.slug}
            noteMeta={note.meta}
            jsonContent={note.jsonContent}
            onUpdate={() => {}}
            onTitleUpdate={(title) => { note.meta.title = title; }}
            onNavigate={handleKanbanWikiLinkNavigation}
            currentProject={note.meta.notebook}
          />
          <StatusBar note={note.meta} />
        {/key}
      </div>
    {/if}
  </div>

  {#if contextMenuTask && contextMenuPos}
    <TaskContextMenu
      task={contextMenuTask}
      position={contextMenuPos}
      onClose={() => { contextMenuTask = null; contextMenuPos = null; }}
      onDelete={(t) => requestTaskDelete(t)}
    />
  {/if}

  {#if deleteConfirmTask}
    <ConfirmDialog
      title="Move to Trash?"
      message={`"${deleteConfirmTask.title || 'Untitled'}" will be moved to trash.`}
      confirmLabel="Move to Trash"
      onConfirm={confirmTaskDelete}
      onCancel={() => { deleteConfirmTask = null; }}
    />
  {/if}
</div>

<style>
  .kanban-board {
    height: 100%;
    display: flex;
    flex-direction: column;
    background: #0a0a0f;
  }

  .kanban-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 24px;
    border-bottom: 1px solid rgba(255,255,255,0.06);
    flex-shrink: 0;
    position: sticky;
    top: 0;
    background: #0a0a0f;
    z-index: 10;
  }

  .header-left {
    display: flex;
    align-items: center;
    gap: 12px;
  }

  .back-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 32px;
    height: 32px;
    border: none;
    border-radius: 4px;
    background: transparent;
    color: #888888;
    cursor: pointer;
    transition: all 0.15s ease;
  }

  .back-btn:hover {
    background: rgba(255,255,255,0.06);
    color: #e0e0e0;
  }

  .kanban-header h2 {
    font-size: 20px;
    font-weight: 600;
    color: #e0e0e0;
    margin: 0;
  }

  .kanban-actions {
    display: flex;
    align-items: center;
    gap: 12px;
  }



  .add-task-btn {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 8px 16px;
    border: none;
    border-radius: 4px;
    background: #00E4E4;
    color: #0a0a0f;
    font-size: 13px;
    font-weight: 500;
    cursor: pointer;
    transition: all 0.15s ease;
    white-space: nowrap;
  }

  @media (max-width: 768px) {
    .kanban-header {
      padding: 16px;
      flex-direction: column;
      align-items: stretch;
      gap: 12px;
    }

    .header-left {
      min-width: unset;
      width: 100%;
    }

    .kanban-actions {
      justify-content: space-between;
      width: 100%;
    }

    .add-task-btn,
    .add-sheet-btn {
      padding: 8px 16px;
      font-size: 13px;
      flex: 1;
      max-width: 150px;
      margin-left: auto;
    }
  }

  @media (max-width: 480px) {
    .add-task-btn,
    .add-sheet-btn {
      padding: 6px 12px;
      font-size: 12px;
      max-width: 120px;
    }


  }

  .add-task-btn:hover {
    background: #00C8C8;
  }

  /* #2339 PR 15c: same box model as .add-task-btn (New Note) — height, padding,
     radius, gap, font are IDENTICAL between the two buttons, so the pair reads
     as siblings. Fill (background/color/border-color) intentionally NOT set
     here — it comes from the global .btn--outline class (src/app.css) applied
     alongside this class in the template, so it never fights that class's
     higher-specificity scoped selector. See app.css's .btn--outline comment. */
  .add-sheet-btn {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 8px 16px;
    border-width: 1px;
    border-style: solid;
    border-radius: 4px;
    font-size: 13px;
    font-weight: 500;
    cursor: pointer;
    transition: all 0.15s ease;
    white-space: nowrap;
  }

  .add-sheet-btn:disabled {
    opacity: 0.5;
    cursor: default;
  }

  /* #2339 PR 15c: 720–900px header width — New Sheet drops its label but keeps
     a 32px square icon-only box (design: "holds its square box—the glyph alone
     is legible and the target stays full size"). */
  .add-sheet-btn--icon-only {
    width: 32px;
    height: 32px;
    padding: 0;
    justify-content: center;
    gap: 0;
  }

  /* #2339 PR 15c: caret popover (header width < 720px) — New Sheet folds in
     beside New Note. New collision-check finding (PR body): .kanban-header
     has no pre-existing overflow/kebab menu, so this caret is a genuinely
     new affordance, not a join. */
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
    border-radius: 4px;
    background: transparent;
    color: #888888;
    cursor: pointer;
    transition: all 0.15s ease;
  }

  .sheet-caret-btn:hover {
    background: rgba(255,255,255,0.06);
    color: #e0e0e0;
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
    color: #888888;
  }

  .sheet-caret-icon--accent {
    color: #00E4E4;
  }

  .loading, .error {
    display: flex;
    align-items: center;
    justify-content: center;
    height: 200px;
    color: #666666;
    font-size: 14px;
  }

  .error {
    color: #EF4444;
  }

  .kanban-columns {
    display: flex;
    gap: 24px;
    padding: 24px;
    overflow-x: auto;
    flex: 1;
    min-height: 0;
  }

  .kanban-column {
    flex: 1;
    min-width: 280px;
    display: flex;
    flex-direction: column;
    background: rgba(255,255,255,0.02);
    border-radius: 4px;
    border: 1px solid rgba(255,255,255,0.04);
  }

  .kanban-column.collapsed {
    flex: 0 0 44px;
    min-width: 44px;
    max-width: 44px;
  }

  .kanban-column.collapsed .column-header {
    writing-mode: vertical-rl;
    text-orientation: mixed;
    white-space: nowrap;
    padding: 12px 8px;
    justify-content: flex-start;
    gap: 8px;
    height: 100%;
    border-bottom: none;
  }

  .kanban-column.collapsed .column-content,
  .kanban-column.collapsed .column-collapsed {
    display: none;
  }

  .column-header {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 12px 12px 8px;
    border-bottom: 1px solid rgba(255,255,255,0.04);
    flex-shrink: 0;
  }

  .column-dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    flex-shrink: 0;
  }

  .column-title {
    font-size: 13px;
    font-weight: 600;
    color: #e0e0e0;
    flex: 1;
  }

  .column-count {
    font-size: 12px;
    color: #666666;
    margin-left: auto;
  }

  .collapse-indicator {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 16px;
    height: 16px;
    color: #666666;
    font-size: 10px;
    flex-shrink: 0;
  }

  .column-collapsed {
    padding: 16px;
    text-align: center;
  }

  .collapsed-summary {
    color: #666666;
    font-size: 12px;
    font-style: italic;
    padding: 8px;
    border-radius: 4px;
    background: rgba(255,255,255,0.03);
  }

  .collapsed-summary.empty {
    opacity: 0.5;
  }

  .column-content {
    flex: 1;
    padding: 8px;
    display: flex;
    flex-direction: column;
    gap: 8px;
    overflow-y: auto;
  }

  .empty-column {
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 32px 16px;
    color: #666666;
    font-size: 13px;
    border: 1px dashed rgba(255,255,255,0.06);
    border-radius: 4px;
  }

  /* #2334 (container-level relocation): .cabinet-drag-wrap is now just the drag
     SOURCE wrapper (card-sized, for ondragend) — no drop-target styling here
     anymore. The insertion-point indicator is .cabinet-drop-indicator below,
     rendered by .column-content at the coordinate-resolved slot, not by a
     per-card border. */
  .cabinet-drag-wrap {
    /* intentionally minimal — drag source only, see comment above */
  }

  /* #2334: coordinate-resolved insertion-point indicator, rendered by the
     Cabinet column's .column-content at cabinetDropIndicatorIndex — same source
     of truth as the drop-index resolution itself (dragReorderIndex.ts). Distinct
     from the column-level .kanban-column.drag-target status-change affordance. */
  .cabinet-drop-indicator {
    height: 2px;
    margin: -1px 0;
    background: #00E4E4;
    border-radius: 1px;
  }

  /* Drag & drop styles */
  .kanban-column:hover {
    background: rgba(255,255,255,0.03);
  }

  .kanban-column.drag-target {
    border-color: rgba(0,228,228,0.4);
    background: rgba(0,228,228,0.03);
  }

  /* List view styles */


  .kanban-split-view {
    display: flex;
    flex: 1;
    min-height: 0;
    overflow: hidden;
  }

  .kanban-left-panel {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    display: flex;
    flex-direction: column;
  }

  .kanban-editor-panel {
    width: 600px; /* overridden by style:width binding */
    border-left: 1px solid rgba(255,255,255,0.06);
    display: flex;
    flex-direction: column;
    background: #0a0a0f;
    overflow: visible; /* Allow pills dropdown to overflow */
    position: relative; /* Anchor for drag handle */
    flex-shrink: 0; /* Don't shrink when width is set */
  }

  .kanban-editor-panel :global(.title-row) {
    z-index: 20;
    position: relative;
    overflow: visible;
    flex-shrink: 0;
  }

  .kanban-editor-panel :global(.metadata-pills-inline) {
    z-index: 20;
    position: relative;
    overflow: visible;
  }

  /* position: fixed is owned by Editor.svelte (#2208 contract) — do not override here */
  /* .kanban-editor-panel :global(.pill-dropdown) z-index is already 1000 in Editor.svelte */

  /* Only the editor content scrolls */
  .kanban-editor-panel :global(.editor-container) {
    overflow-y: auto;
    flex: 1;
  }

  .kanban-editor-resize {
    position: absolute;
    top: 0;
    left: -2px;
    bottom: 0;
    width: 4px;
    cursor: col-resize;
    background: transparent;
    transition: background 0.15s ease;
    z-index: 56;
  }

  .kanban-editor-resize:hover {
    background: rgba(0, 228, 228, 0.3);
  }

  .empty-editor {
    flex: 1;
    display: flex;
    align-items: center;
    justify-content: center;
    color: #666666;
    font-size: 14px;
  }
</style>