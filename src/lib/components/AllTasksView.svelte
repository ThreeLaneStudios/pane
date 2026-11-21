<script lang="ts">
  import { onMount } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import type { NoteMeta, Project, TaskStatus, TaskPriority } from '$lib/types';
  import * as api from '$lib/api';
  import * as noteService from '$lib/services/noteService';
  import { activeNote } from '$lib/stores/app';
  import { get } from 'svelte/store';
  import Editor from './Editor.svelte';
  import StatusBar from './StatusBar.svelte';
  import { navigateWikiLink } from '$lib/utils/wikiLinkNavigation';
  // probe: instrument all $activeNote write sites — investigating #2214 delete-close latch
  const DEBUG = typeof localStorage !== 'undefined' && localStorage.getItem('pane-debug') === '1';
  function logAnSet(caller: string, newVal: unknown): void {
    if (!DEBUG) return;
    const oldId = get(activeNote)?.meta?.id ?? 'null';
    const newId = (newVal as any)?.meta?.id ?? 'null';
    console.log(`[PANE-DEBUG] $activeNote SET in ${caller} | old="${oldId}" new="${newId}"`);
    console.log('[PANE-DEBUG] stack:', new Error().stack?.split('\n').slice(1, 5).join('\n'));
  }
  import Card from './Card.svelte';
  import { normalizeNoteMeta } from '$lib/utils/cardNormalizers';
  import { getStageColor } from '$lib/utils/stageColors';
  import { stripMarkdown } from '$lib/utils/stripMarkdown';
  import ConfirmDialog from './ConfirmDialog.svelte';
  import {
    ENTITY_NOTE,
    ENTITY_SHEET,
    REASON_DELETED,
    subscribeDataChanged,
    type DataChangedPayload,
  } from '$lib/refreshEvent';

  // #2972 stage 2: this view's visible task statuses (no cabinet) — shared
  // by loadData()'s filter and handleDataChangedEvent()'s "does this note
  // belong on this screen" check below, so both stay in sync.
  const VISIBLE_TASK_STATUSES = ['todo', 'in_progress', 'review', 'done'] as const;

  let allTaskNotes: NoteMeta[] = $state([]);
  let subtaskCounts: Record<string, number> = $state({});
  // #2339 PR 15a: card-face summaries (col/row/formula counts + col types)
  // for whichever loaded allTaskNotes are sheet notes, keyed by note id.
  // Same pattern as KanbanBoard.svelte/NotesBrowser.svelte's sheetSummaries/
  // cardSummaryFor — one batched fetch per loadData() pass. AllTasksView
  // routes every task through this same Card.svelte/normalizeNoteMeta path
  // (confirmed at build time: no separate bespoke list-density row renderer
  // exists in this file), so wiring the summary in here is what satisfies
  // README ruling 5's "All Tasks keeps both the glyph swap and the cols ×
  // rows metric" — Card.svelte's sheet branch supplies both.
  let sheetSummaries: Map<string, import('$lib/types').SheetCardSummary> = $state(new Map());

  function cardSummaryFor(note: NoteMeta) {
    if (note.note_type !== 'sheet') return undefined;
    const s = sheetSummaries.get(note.id);
    if (!s) return undefined;
    return { colCount: s.col_count, rowCount: s.row_count, formulaCount: s.formula_count, colTypes: s.col_types };
  }
  let projects: Project[] = $state([]);
  let loading = $state(false);
  let selectedProject = $state('all');
  let selectedPriority = $state('all');
  let showEditor = $state(false);
  let allTasksEditorWidth = $state(parseInt(localStorage.getItem('pane-alltasks-editor-width') || '560'));
  // #2339 PR9 ruling 2: sheet overlay width persists under its OWN localStorage
  // key, separate from the note-width key above, defaulting to 1000px
  // (README ruling 4). Notes' key/semantics are untouched.
  let allTasksSheetWidth = $state(parseInt(localStorage.getItem('pane-alltasks-editor-width-sheet') || '1000'));
  let splitViewRef: HTMLElement | undefined = $state();

  // #2331 parity (audit §4.1): guards the relocated close-handler against a synthetic click
  // landing outside .editor-panel after a resize-drag ends with the pointer off the panel.
  // Set BEFORE attaching mousemove/mouseup, cleared via a deferred setTimeout(...,0) in
  // mouseup so a synthetic click firing after mouseup (but within the same dispatch
  // sequence) is still caught. Mirrors KanbanBoard.svelte:73-101.
  let isEditorResizing = $state(false);

  function startEditorResize(e: MouseEvent) {
    e.preventDefault();
    // #2339 PR9 ruling 2: resize a separate width state/localStorage key when
    // the panel is showing a sheet note vs. a doc note.
    const isSheetResize = $activeNote?.meta?.note_type === 'sheet';
    // #2331: set BEFORE attaching mousemove/mouseup listeners — ordering matters.
    isEditorResizing = true;
    const startX = e.clientX;
    const startWidth = isSheetResize ? allTasksSheetWidth : allTasksEditorWidth;
    function onMouseMove(ev: MouseEvent) {
      const delta = startX - ev.clientX;
      // #4.1: dynamic max-width from the live split-view ref, replacing the hardcoded 800.
      const maxWidth = splitViewRef ? splitViewRef.clientWidth : 1200;
      const newWidth = Math.max(350, Math.min(maxWidth, startWidth + delta));
      if (isSheetResize) allTasksSheetWidth = newWidth;
      else allTasksEditorWidth = newWidth;
    }
    function onMouseUp() {
      if (isSheetResize) {
        localStorage.setItem('pane-alltasks-editor-width-sheet', String(allTasksSheetWidth));
      } else {
        localStorage.setItem('pane-alltasks-editor-width', String(allTasksEditorWidth));
      }
      document.removeEventListener('mousemove', onMouseMove);
      document.removeEventListener('mouseup', onMouseUp);
      document.body.style.cursor = '';
      document.body.style.userSelect = '';
      // #2331: deferred clear — a synthetic click (if one fires) arrives after this handler
      // returns but within the same dispatch sequence; setTimeout(...,0) defers past that.
      setTimeout(() => { isEditorResizing = false; }, 0);
    }
    document.addEventListener('mousemove', onMouseMove);
    document.addEventListener('mouseup', onMouseUp);
    document.body.style.cursor = 'col-resize';
    document.body.style.userSelect = 'none';
  }
  let selectedNote: NoteMeta | null = $state(null);
  let draggedNote: NoteMeta | null = $state(null);
  let dragOverColumn: TaskStatus | null = $state(null);

  // Unified context menu state machine
  type ContextMenuState = 
    | { phase: 'actions', x: number, y: number, note: NoteMeta }
    | { phase: 'projects', x: number, y: number, note: NoteMeta, projects: Project[] }
    | { phase: 'folders', x: number, y: number, note: NoteMeta, projectId: string, projectName: string, folders: Project[] }
    | null;
  
  let contextMenuState: ContextMenuState = $state(null);
  let deleteConfirmNote: NoteMeta | null = $state(null);

  const taskColumns: { status: TaskStatus; title: string; color: string }[] = [
    { status: 'todo', title: 'To Do', color: getStageColor('todo') },
    { status: 'in_progress', title: 'In Progress', color: getStageColor('in_progress') },
    { status: 'review', title: 'Review', color: getStageColor('review') },
    { status: 'done', title: 'Done', color: getStageColor('done') },
  ];

  // Matt's ruling supersedes the donor's collapse gate (audit §1.4/§1.6): EVERY column is
  // collapsible, unconditionally — not just Done, not gated by task-count. Mirrors
  // KanbanBoard.svelte's collapsedColumns Set mechanism, minus the >5-task condition.
  // Done defaults collapsed on mount; not persisted (matches donor — #2252 does not apply,
  // see audit §1.5).
  let collapsedColumns = $state(new Set<TaskStatus>(['done']));

  function toggleColumnCollapse(status: TaskStatus) {
    if (collapsedColumns.has(status)) {
      collapsedColumns.delete(status);
    } else {
      collapsedColumns.add(status);
    }
    collapsedColumns = new Set(collapsedColumns);
  }

  // handleTaskMetaChanged (the old `pane-task-meta-changed` partial-field
  // patch) deleted (#2972 stage 3, item 2 — ruling 4 folds it in): the
  // handler below already does a strict superset (full entity patch, plus
  // insert/remove) for the same `reason: "edited"` case.

  // #2972 stage 2 (§34.5): single pane-data-changed handler, patch/insert/
  // remove one row in `allTaskNotes` — never a full loadData() reload.
  // Replaces the dead `listen('pane-note-changed', ...)` registration below
  // (Stage 1 deleted every backend emitter of that name, #2972 stage 1,
  // §34.1.1) and the full-reload `pane-data-changed`/`pane-note-created`
  // DOM listeners (§34.2's "Replaced" verdict for both).
  async function handleDataChangedEvent(payload: DataChangedPayload) {
    const { entity_id, entity_type, reason } = payload;
    if (entity_type !== ENTITY_NOTE && entity_type !== ENTITY_SHEET) return;

    if (reason === REASON_DELETED) {
      allTaskNotes = allTaskNotes.filter(n => n.id !== entity_id);
      return;
    }

    try {
      // `api.readNote()`, not `entityService.getNote()` — only `readNote()`
      // remaps the raw backend `status`/`priority`/`due_date` fields onto
      // `task_status`/`task_priority`/`task_due_date`, which is the shape
      // `allTaskNotes`/`VISIBLE_TASK_STATUSES` (and every card renderer
      // reading this array) expect, matching `listNotesByProject()`'s
      // existing `transformNoteMeta()` convention.
      const noteData = await api.readNote('', entity_id);
      const meta = noteData.meta;
      const belongsHere = VISIBLE_TASK_STATUSES.includes(
        meta.task_status as typeof VISIBLE_TASK_STATUSES[number]
      );
      const idx = allTaskNotes.findIndex(n => n.id === entity_id);

      if (!belongsHere) {
        if (idx !== -1) allTaskNotes = allTaskNotes.filter(n => n.id !== entity_id);
        return;
      }

      if (idx === -1) {
        allTaskNotes = [...allTaskNotes, meta];
      } else {
        allTaskNotes = allTaskNotes.map(n => (n.id === entity_id ? { ...n, ...meta } : n));
      }
    } catch (e) {
      console.error('[ALL-TASKS] Failed to patch/insert note for pane-data-changed event:', entity_id, e);
    }
  }

  onMount(() => {
    loadData();
    // #2972 stage 3 (item 4): one shared helper hearing BOTH channels —
    // backend `listen()` (Tauri IPC is invisible to window.addEventListener,
    // #2215) AND the app `CustomEvent` — replacing Stage 2's backend-only
    // `listen()` registration (which itself replaced the dead
    // `listen('pane-note-changed', ...)` registration, #2972 stage 1).
    const unlistenDataChanged = subscribeDataChanged((payload) => {
      handleDataChangedEvent(payload);
    });

    return () => {
      unlistenDataChanged.then(fn => fn());
    };
  });

  async function loadData(silent = false) {
    if (!silent) loading = true;
    try {
      const taskNotes = await noteService.listTasks();
      console.log('[AllTasksView] Loaded task notes:', taskNotes?.length || 0);
      // Filter to show todo, in_progress, review, and done (no cabinet)
      allTaskNotes = (taskNotes || []).filter(n =>
        VISIBLE_TASK_STATUSES.includes(n.task_status as typeof VISIBLE_TASK_STATUSES[number])
      );

      // Load subtask counts
      const noteIds = allTaskNotes.map(n => n.id);
      if (noteIds.length > 0) {
        try { subtaskCounts = await api.getSubtaskCounts(noteIds); } catch { subtaskCounts = {}; }
      }

      // #2339 PR 15a: batched sheet card summaries for whichever loaded
      // tasks are sheet notes. Best-effort — a failure here must not block
      // the list from rendering.
      const sheetIds = allTaskNotes.filter(n => n.note_type === 'sheet').map(n => n.id);
      if (sheetIds.length > 0) {
        try {
          const summaries = await api.getSheetCardSummaries(sheetIds);
          sheetSummaries = new Map(summaries.map(s => [s.note_id, s]));
        } catch (e) {
          console.warn('Failed to load sheet card summaries:', e);
          sheetSummaries = new Map();
        }
      } else {
        sheetSummaries = new Map();
      }

      try { projects = await api.listProjects(); } catch { projects = []; }
    } catch (e) {
      console.error('[AllTasksView] Failed to load tasks:', e);
      allTaskNotes = [];
    } finally {
      loading = false;
    }
  }

  // Get favorited project names for filtering
  const favoritedProjectNames = $derived.by(() => {
    return projects
      .filter(p => p.is_favorite)
      .map(p => p.name);
  });

  const filteredNotes = $derived.by(() => {
    let filtered = [...allTaskNotes];
    // Default: only show tasks from favorited projects
    if (favoritedProjectNames.length > 0) {
      filtered = filtered.filter(n => {
        const projectName = (n.notebook || '').split('/')[0];
        return favoritedProjectNames.includes(projectName);
      });
    }
    if (selectedPriority !== 'all') {
      filtered = filtered.filter(n => n.task_priority === selectedPriority);
    }
    return filtered;
  });

  function getTasksForColumn(status: TaskStatus): NoteMeta[] {
    return [...filteredNotes].filter(n => n.task_status === status)
      .sort((a, b) => {
        const po: Record<string, number> = { urgent: 0, high: 1, medium: 2, low: 3 };
        const ap = po[a.task_priority || 'medium'] ?? 2;
        const bp = po[b.task_priority || 'medium'] ?? 2;
        if (ap !== bp) return ap - bp;
        // Due date sort
        if (a.task_due_date && b.task_due_date) return new Date(a.task_due_date).getTime() - new Date(b.task_due_date).getTime();
        if (a.task_due_date) return -1;
        if (b.task_due_date) return 1;
        return 0;
      });
  }

  async function handleCardClick(note: NoteMeta) {
    selectedNote = note;
    showEditor = true;
    // Load note content WITHOUT triggering canvas navigation
    // (noteService.selectNote changes canvasState to note_editor, which navigates away from All Tasks)
    try {
      const noteContent = await api.readNote(note.notebook || '', note.slug || note.id);
      // Enrich task metadata from DB (mirrors what noteService.selectNote does without the nav side-effect)
      try {
        const taskFields = await api.getNoteTaskFields(note.id);
        if (noteContent.meta && taskFields) {
          noteContent.meta = { ...noteContent.meta, ...taskFields };
        }
      } catch (e2) {
        console.warn('[AllTasksView] getNoteTaskFields failed, using stale meta:', e2);
      }
      logAnSet('AllTasksView.handleNoteOpen', noteContent);
      $activeNote = noteContent;
    } catch (e) {
      console.error('[AllTasksView] Failed to load note:', e);
    }
  }

  // handleCloseEditor's body stays UNTOUCHED (audit §5.1 blast radius — must keep writing
  // only $activeNote = null so CanvasRenderer's #2385 reload-guard reasoning still holds).
  function handleCloseEditor() {
    showEditor = false;
    selectedNote = null;
    logAnSet('AllTasksView.handleCloseEditor', null);
    activeNote.set(null);
    setTimeout(() => loadData(true), 300);
  }

  function handleDragStart(note: NoteMeta) { draggedNote = note; }

  function handleDragOver(e: DragEvent, status: TaskStatus) {
    e.preventDefault();
    if (e.dataTransfer) e.dataTransfer.dropEffect = 'move';
    dragOverColumn = status;
  }

  // Donor pattern (KanbanBoard.svelte:459-484): optimistic local mutation first, API call
  // second, revert (via reload) only on error — no full reload on the happy path.
  async function handleDrop(e: DragEvent, targetStatus: TaskStatus) {
    e.preventDefault();
    dragOverColumn = null;
    if (!draggedNote || draggedNote.task_status === targetStatus) {
      if (draggedNote) {
        console.debug('[AllTasksView] drop: status unchanged, no-op', { noteId: draggedNote.id, from: draggedNote.task_status, to: targetStatus });
      }
      draggedNote = null;
      return;
    }
    const noteToMove = draggedNote;
    draggedNote = null;
    // Optimistic update
    allTaskNotes = allTaskNotes.map(n =>
      n.id === noteToMove.id
        ? { ...n, task_status: targetStatus, updated_at: new Date().toISOString() }
        : n
    );
    try {
      await noteService.updateTaskFields(noteToMove.id, { status: targetStatus });
    } catch (e2) {
      console.error('Drop failed:', e2);
      // Revert on error — resync with server truth.
      await loadData(true);
    }
  }

  function clampMenuPosition(x: number, y: number): { x: number, y: number } {
    const menuWidth = 240;
    const menuHeight = 320;
    const clampedX = x + menuWidth > window.innerWidth ? x - menuWidth : x;
    const clampedY = y + menuHeight > window.innerHeight ? y - menuHeight : y;
    return { x: Math.max(0, clampedX), y: Math.max(0, clampedY) };
  }

  function handleCardContextMenu(e: MouseEvent, note: NoteMeta) {
    e.preventDefault();
    e.stopPropagation();
    const pos = clampMenuPosition(e.clientX, e.clientY);
    contextMenuState = { phase: 'actions', x: pos.x, y: pos.y, note };
  }
  
  async function startMoveNote() {
    if (!contextMenuState || contextMenuState.phase !== 'actions') return;
    const { x, y, note } = contextMenuState;
    try {
      const projects = (await api.listProjects()).filter(p => !p.parent_id);
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
      await loadData(true);
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
        const filtered = projs.filter(p => !p.parent_id);
        contextMenuState = { phase: 'projects', x, y, note, projects: filtered };
      });
    } else {
      contextMenuState = { phase: 'projects', x, y, note, projects };
    }
  }

  function deleteCardNote() {
    if (!contextMenuState || contextMenuState.phase !== 'actions') return;
    const note = contextMenuState.note;
    contextMenuState = null;
    deleteConfirmNote = note;
  }

  async function confirmDeleteCardNote() {
    if (!deleteConfirmNote) return;
    const note = deleteConfirmNote;
    deleteConfirmNote = null;
    allTaskNotes = allTaskNotes.filter(n => n.id !== note.id);
    try {
      await api.moveToTrash(note.id);
    } catch (e2) { console.error('Delete failed:', e2); loadData(); }
  }

  function formatDate(dateStr: string): string {
    if (!dateStr) return '';
    try {
      const date = new Date(dateStr);
      const now = new Date();
      const diffDays = Math.ceil((date.getTime() - now.getTime()) / (1000 * 60 * 60 * 24));
      if (diffDays === 0) return 'Today';
      if (diffDays === 1) return 'Tomorrow';
      if (diffDays < 0) return `${Math.abs(diffDays)}d overdue`;
      return date.toLocaleDateString([], { month: 'short', day: 'numeric' });
    } catch { return ''; }
  }



  function getProjectName(notebook: string): string { return (notebook || '').split('/')[0]; }

  function clearFilters() { selectedPriority = 'all'; }

  // Donor pattern (KanbanBoard.svelte:538-556) — wiki-link click navigation from the All
  // Tasks editor panel. #3044 ruling: navigation-only, open an existing note; a missing
  // note stays inert (no create, no confirm).
  async function handleAllTasksWikiLinkNavigation(title: string) {
    try {
      await navigateWikiLink(title, {
        searchFn: (q) => api.searchNotes(q),
        selectNoteFn: (note, contextOverride) => noteService.selectNote(note, contextOverride),
        getNoteRawFn: async (id) => { const raw: any = await api.getNoteRaw(id); return raw?.meta ?? null; },
      });
    } catch (e) {
      console.error('[handleAllTasksWikiLinkNavigation] Failed to navigate from wiki link:', e);
    }
  }
</script>

<div class="all-tasks-view">
  <div class="tasks-header">
    <div class="header-top">
      <h2>All Tasks</h2>
      <div class="header-right">
        <span class="task-count">{filteredNotes.length} tasks</span>
      </div>
    </div>
    <div class="filter-bar">
      <select class="filter-select" bind:value={selectedPriority}>
        <option value="all">All Priorities</option>
        <option value="urgent">Urgent</option>
        <option value="high">High</option>
        <option value="medium">Medium</option>
        <option value="low">Low</option>
      </select>
      {#if selectedPriority !== 'all'}
        <button class="clear-btn" onclick={clearFilters}>Clear</button>
      {/if}
    </div>
  </div>

  <div class="tasks-body" bind:this={splitViewRef}>
    <!-- Close-handler relocation (#2329 class fix, audit §3): moved OFF .all-tasks-view (the
         view root, a shared ancestor of both the card columns AND the editor mount) onto this
         wrapper around .kanban-columns only — sibling of .editor-panel, mirroring Kanban's
         .kanban-left-panel (KanbanBoard.svelte:597). .editor-panel is now structurally outside
         this handler's bubble path, so a checklist click inside an open task can never reach it. -->
    <div class="kanban-container" onclick={(e) => {
      // #2331 guard is consumed FIRST, before the closest() denylist (audit §4.1 piece 4).
      if (isEditorResizing) return;
      if (showEditor && !(e.target as HTMLElement).closest('.editor-panel') && !(e.target as HTMLElement).closest('.pane-card')) {
        handleCloseEditor();
      }
    }}>
      {#if loading}
        <div class="loading-state">Loading tasks...</div>
      {:else}
        <div class="kanban-columns">
          {#each taskColumns as column}
            <div
              class="kanban-column"
              class:collapsed={collapsedColumns.has(column.status)}
              class:drag-target={dragOverColumn === column.status}
              role="region"
              aria-label={column.title}
              ondragover={(e) => handleDragOver(e, column.status)}
              ondrop={(e) => handleDrop(e, column.status)}
              ondragleave={() => dragOverColumn = null}
            >
              <div
                class="column-header"
                onclick={() => toggleColumnCollapse(column.status)}
                style:cursor="pointer"
                title={collapsedColumns.has(column.status) ? 'Expand column' : 'Collapse column'}
              >
                <div class="column-dot" style:background-color={column.color}></div>
                <span class="column-title">{column.title}</span>
                <span class="column-count">{getTasksForColumn(column.status).length}</span>
                <span class="collapse-indicator">
                  {collapsedColumns.has(column.status) ? '▶' : '▼'}
                </span>
              </div>
              {#if !collapsedColumns.has(column.status)}
                <div class="column-content">
                  {#each getTasksForColumn(column.status) as note (note.id)}
                  <Card
                    data={normalizeNoteMeta(note, { subtaskCount: subtaskCounts[note.id] || 0, sheetSummary: cardSummaryFor(note) })}
                    onSelect={(id) => { const n = allTaskNotes.find(x => x.id === id); if (n) handleCardClick(n); }}
                    onContextMenu={(e, id) => { const n = allTaskNotes.find(x => x.id === id); if (n) handleCardContextMenu(e, n); }}
                    onDragStart={(id) => { const n = allTaskNotes.find(x => x.id === id); if (n) handleDragStart(n); }}
                  />
                {/each}
                {#if getTasksForColumn(column.status).length === 0}
                  <div class="empty-column">No {column.title.toLowerCase()} tasks</div>
                {/if}
                </div>
              {/if}
            </div>
          {/each}
        </div>
      {/if}
    </div>

    {#if showEditor && $activeNote}
      {@const isActiveSheet = $activeNote.meta?.note_type === 'sheet'}
      <div class="editor-panel" style:width="{isActiveSheet ? allTasksSheetWidth : allTasksEditorWidth}px">
        <div class="editor-resize"
          onmousedown={startEditorResize}
          role="separator"
          aria-orientation="vertical"
          tabindex="-1"
        ></div>
        {#key $activeNote.meta?.id}
          {@const noteMeta = {
            ...$activeNote.meta,
            task_status: $activeNote.meta?.task_status || 'todo',
            task_priority: $activeNote.meta?.task_priority || undefined,
            task_due_date: $activeNote.meta?.task_due_date || undefined,
            task_assignee: $activeNote.meta?.task_assignee || undefined,
          }}
          <Editor
            slug={$activeNote.meta?.slug || $activeNote.meta?.id?.split('/').pop() || ''}
            currentProject={$activeNote.meta?.notebook || ''}
            content={$activeNote.content || ''}
            title={$activeNote.meta?.title || ''}
            jsonContent={$activeNote.jsonContent}
            noteId={$activeNote.meta?.id || ''}
            noteMeta={noteMeta}
            onUpdate={() => {}}
            onTitleUpdate={(newTitle) => { allTaskNotes = allTaskNotes.map(n => n.id === $activeNote?.meta?.id ? { ...n, title: newTitle } : n); }}
            onNavigate={handleAllTasksWikiLinkNavigation}
            onMoved={() => handleCloseEditor()}
          />
          <StatusBar note={noteMeta} />
        {/key}
      </div>
    {/if}
  </div>

  {#if contextMenuState}
    <div class="context-overlay" onclick={() => contextMenuState = null}></div>
    <div class="context-menu" style="left: {contextMenuState.x}px; top: {contextMenuState.y}px">
      {#if contextMenuState.phase === 'actions'}
        <button class="context-item" onclick={() => startMoveNote()}>
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
            <path d="M15 3h4a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2h-4M10 17l5-5-5-5M13.8 12H3"/>
          </svg>
          Move to...
        </button>
        <button class="context-item delete" onclick={deleteCardNote}>
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
            <polyline points="3,6 5,6 21,6"></polyline>
            <path d="M19,6v14a2,2 0 0,1-2,2H7a2,2 0 0,1-2-2V6m3,0V4a2,2 0 0,1,2-2h4a2,2 0 0,1,2,2v2"></path>
          </svg>
          Delete
        </button>
      
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
</div>

{#if deleteConfirmNote}
  <ConfirmDialog
    title="Move to Trash?"
    message={`"${deleteConfirmNote.title || 'Untitled'}" will be moved to trash.`}
    confirmLabel="Move to Trash"
    onConfirm={confirmDeleteCardNote}
    onCancel={() => { deleteConfirmNote = null; }}
  />
{/if}

<style>
  .all-tasks-view {
    position: relative;
    height: 100%;
    display: flex;
    flex-direction: column;
    background: #0a0a0f;
    color: #e0e0e0;
    font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Text', 'Segoe UI', sans-serif;
  }
  .tasks-header {
    flex-shrink: 0;
    border-bottom: 1px solid rgba(255,255,255,0.06);
    padding: 24px;
    position: sticky;
    top: 0;
    background: #0a0a0f;
    z-index: 10;
  }
  .header-top {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 12px;
  }
  .header-top h2 { font-size: 20px; font-weight: 600; color: #e0e0e0; margin: 0; }
  .header-right { display: flex; align-items: center; gap: 12px; }
  .task-count { font-size: 13px; color: #888888; }
  .filter-bar { display: flex; align-items: center; gap: 10px; }
  .filter-select {
    padding: 6px 10px; border: 1px solid rgba(255,255,255,0.06);
    border-radius: 4px; background: rgba(255,255,255,0.05); color: #e0e0e0;
    font-size: 12px; cursor: pointer;
  }
  .filter-select:hover { border-color: rgba(0,228,228,0.3); }
  .clear-btn {
    padding: 6px 10px; background: none; border: 1px solid rgba(255,255,255,0.06);
    border-radius: 4px; color: #888888; font-size: 12px; cursor: pointer;
  }
  .clear-btn:hover { color: #e0e0e0; border-color: rgba(255,255,255,0.1); }

  /* Split-view structure (audit §4.2), aligned to donor arrangement:
     .tasks-body === .kanban-split-view, .kanban-container === .kanban-left-panel. */
  .tasks-body { flex: 1; display: flex; overflow: hidden; min-height: 0; }
  /* overflow-x:auto kept as narrow-viewport fallback (audit §4.2.2) even though columns now
     grow via flex:1 like the donor — 4 columns (no Cabinet) can still overflow a narrow pane. */
  .kanban-container { flex: 1; min-width: 0; padding: 16px; overflow-x: auto; overflow-y: hidden; }
  .kanban-columns { display: flex; gap: 16px; height: 100%; min-height: 0; }
  .kanban-column {
    flex: 1;
    min-width: 280px;
    display: flex; flex-direction: column;
    background: rgba(255,255,255,0.02); border-radius: 4px; border: 1px solid rgba(255,255,255,0.04);
  }
  .kanban-column.drag-target { border-color: rgba(0,228,228,0.4); background: rgba(0,228,228,0.03); }

  /* All-columns-collapsible (Matt's ruling supersedes donor's Done-only/>5-task gate, audit
     §1.4/§1.6): adopts Kanban's 44px vertical-rail rendering (KanbanBoard.svelte:602, 910-922)
     for EVERY column, unconditionally. */
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
  }

  .column-header {
    display: flex; align-items: center; gap: 8px;
    padding: 12px 12px 8px; flex-shrink: 0;
  }
  .column-dot { width: 8px; height: 8px; border-radius: 50%; flex-shrink: 0; }
  .column-title { font-size: 13px; font-weight: 600; color: #e0e0e0; }
  .column-count { font-size: 12px; color: #666666; margin-left: auto; }
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
  .column-content {
    flex: 1; overflow-y: auto; padding: 8px;
    display: flex; flex-direction: column; gap: 8px;
  }

  /* Card styles now in shared Card.svelte component */

  .empty-column {
    display: flex; align-items: center; justify-content: center;
    padding: 32px 16px; color: #666666; font-size: 13px;
    border: 1px dashed rgba(255,255,255,0.06); border-radius: 4px;
  }
  .loading-state { display: flex; align-items: center; justify-content: center; height: 100%; color: #666666; }

  .editor-panel {
    flex-shrink: 0;
    display: flex; flex-direction: column; overflow: hidden;
    background: #0a0a0f;
    border-left: 1px solid rgba(255,255,255,0.06);
    position: relative; /* Anchor for drag handle */
  }

  /* Ensure chips and dropdowns render above editor content */
  .editor-panel :global(.title-row) {
    z-index: 20;
    position: relative;
    overflow: visible;
    flex-shrink: 0;
  }

  .editor-panel :global(.metadata-pills-inline) {
    z-index: 20;
    position: relative;
    overflow: visible;
  }

  /* position: fixed is owned by Editor.svelte (#2208 contract) — do not override here */
  /* .editor-panel :global(.pill-dropdown) z-index is already 1000 in Editor.svelte */

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

  .editor-resize {
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
  .editor-resize:hover {
    background: rgba(0, 228, 228, 0.3);
  }
</style>
