import * as api from '$lib/api';
import { notes, activeNote, activeNotebook, activeProject } from '$lib/stores/app';
import { editorSavePromise } from '$lib/stores/editorSave';
import { canvasState } from '$lib/stores/canvas';
import { get } from 'svelte/store';
import type { NoteMeta, NoteContent, TaskStatus, TaskPriority } from '$lib/types';

// probe: instrument all $activeNote write sites — investigating #2214 delete-close latch
const DEBUG = typeof localStorage !== 'undefined' && localStorage.getItem('pane-debug') === '1';
function logActiveNoteSet(caller: string, oldVal: unknown, newVal: unknown): void {
  if (!DEBUG) return;
  const oldId = (oldVal as any)?.meta?.id ?? 'null';
  const newId = (newVal as any)?.meta?.id ?? 'null';
  console.log(`[PANE-DEBUG] $activeNote SET in ${caller} | old="${oldId}" new="${newId}"`);
  console.log('[PANE-DEBUG] stack:', new Error().stack?.split('\n').slice(1, 5).join('\n'));
}

export async function createNote(notebook?: string): Promise<NoteContent> {
  const nb = notebook || get(activeNotebook) || 'Inbox';
  // CRITICAL: Null activeNote first to trigger Editor destroy+save with correct slug.
  // Without this, the old Editor's onDestroy fires AFTER $activeNote changed to the new note,
  // and reactive props may read the new note's slug — saving old content to new note's file.
  logActiveNoteSet('createNote[pre-null]', get(activeNote), null);
  activeNote.set(null);
  await editorSavePromise; // Wait for in-flight onDestroy save to complete
  await new Promise(r => setTimeout(r, 20)); // DB flush buffer
  
  const note = await api.createNote(nb, '');
  // Add to store immediately
  notes.update(n => [note.meta, ...n]);
  logActiveNoteSet('createNote[set-new]', get(activeNote), note);
  activeNote.set(note);
  return note;
}

/**
 * #3044 (revisited): the ONE shared "create a new note" function.
 * `NotesBrowser.svelte`'s New Note button and `+page.svelte`'s Cmd+N
 * handler both call this directly — neither has any creation code of its
 * own anymore. Deliberately reads/writes stores only, never a component's
 * local state, so it works whether or not any particular component is
 * mounted. That property is what makes the old `window.__paneNewNote`
 * race structurally impossible rather than relocated: there is no global
 * that can go stale/undefined depending on what's on screen, and no
 * "right component must already be mounted" precondition at all — this
 * function does not care.
 *
 * Inside a project (`$activeProject` set): creates the note in that
 * project's own `default_folder_id` — never a "first non-system folder"
 * scan, never a hardcoded `'AI'`/`'Pane System'` name check (both were the
 * leftover island #3044 already removed once from the old per-component
 * `handleNewNote`; this function is the one place that logic can no
 * longer regress back in) — then opens it. Opening reuses the exact same
 * activeNote-then-canvasState sequencing `selectNote()` below already
 * uses (set `$activeNote` first, THEN write `canvasState`), so it opens
 * correctly regardless of which screen Cmd+N or the button was pressed
 * from: already-rendering `NotesBrowser` (`notes_browser`/`note_editor`)
 * picks it up with no remount since the view value may not even change,
 * and any other project-scoped screen (e.g. `project_config`) navigates
 * straight into the note editor.
 *
 * Outside a project (`$activeProject` is null — Home, All Tasks,
 * Calendar, Trash, the project list): creates nothing and dispatches
 * `pane-new-note-blocked` on `window` instead; `+page.svelte` (always
 * mounted, the app root) listens for that and shows the message
 * "Open a project to make a note." exactly as Matt specified.
 */
export async function createNewNote(): Promise<void> {
  const project = get(activeProject);
  if (!project) {
    window.dispatchEvent(new CustomEvent('pane-new-note-blocked'));
    return;
  }

  // Same pre-null + flush-pending-save pattern createNote()/selectNote()
  // in this file both already use — flushes any in-flight editor save for
  // whatever note was open before this one takes its place, so a Cmd+N
  // pressed while a DIFFERENT note has unsaved edits can't let that note's
  // onDestroy save land against the new note's slug.
  logActiveNoteSet('createNewNote[pre-null]', get(activeNote), null);
  activeNote.set(null);
  await editorSavePromise;
  await new Promise(r => setTimeout(r, 20)); // DB flush buffer

  const folderId = project.default_folder_id ?? undefined;
  const newMeta = await api.createNoteInProject(project.id, folderId);
  const newNote: NoteContent = { meta: newMeta, content: '' };
  logActiveNoteSet('createNewNote[set-new]', get(activeNote), newNote);
  activeNote.set(newNote);

  canvasState.update(state => ({
    ...state,
    view: 'note_editor',
    context: {
      ...state.context,
      projectId: project.id,
      folderId,
      noteId: newMeta.id,
      noteTitle: newMeta.title,
      projectName: project.name,
      taskId: undefined, // Clear stale task context, same as selectNote()
    }
  }));
}

/**
 * #3044 delta-2 (Reed) — the shared check. Both `NotesBrowser.svelte`'s
 * and `KanbanBoard.svelte`'s `onMount` used to run an unconditional
 * `$activeNote = null` as their first statement. Each one's own comment
 * explains why: drop a STALE note carried over from whatever view was
 * active before THIS one mounted (e.g. a note left open in Kanban must
 * not reopen itself the instant NotesBrowser mounts for an unrelated
 * reason). That protection is real and must survive.
 *
 * The bug: it fired unconditionally, so it ALSO wiped a note a
 * navigation had just deliberately opened. `createNewNote()` above and
 * `selectNote()` below both do `activeNote.set(note)` immediately
 * followed by `canvasState.update(... view ...)` with no `await` between
 * the two — when that view write is this component's FIRST mount (Cmd+N
 * from a non-Notes screen; a wiki-link click from AllTasksView or
 * Kanban's own editor panel, both of which call `selectNote()` while
 * NotesBrowser isn't mounted), the mount's onMount runs in response and,
 * pre-fix, nulled the very note that write had just set — before the
 * caller, or the user, could ever observe it.
 *
 * Fix: keep `$activeNote` when it already IS the note `canvasState.
 * context.noteId` says THIS navigation targeted (`canvas.ts`'s
 * `CanvasState.context.noteId`, written atomically with `view` by both
 * functions above) — that equality is exactly the "was this mount caused
 * by a deliberate open, not a leftover from some other screen" signal.
 * Clear in every other case (no target noteId, or a target that doesn't
 * match what's currently active) — the stale-carry-over protection is
 * unchanged for every case it originally existed to cover.
 *
 * One function, not a copy per component — `NotesBrowser.svelte` and
 * `KanbanBoard.svelte`'s `onMount`s both call this and nothing else for
 * their activeNote handling.
 */
export function clearStaleActiveNoteOnMount(): void {
  const current = get(activeNote);
  const targetNoteId = get(canvasState).context.noteId;
  if (current && targetNoteId && current.meta?.id === targetNoteId) {
    // This mount was caused by a navigation that deliberately opened this
    // exact note (createNewNote()/selectNote()'s set-then-navigate) — keep it.
    return;
  }
  // Stale carry-over from a different view, or nothing to protect — clear it,
  // exactly as both call sites' original unconditional clear did.
  logActiveNoteSet('clearStaleActiveNoteOnMount', current, null);
  activeNote.set(null);
}

// Monotonic counter — discards out-of-order API responses from concurrent clicks.
let selectSeq = 0;

// Cancel any pending selectNote async call — used by goBackToCards() so an
// in-flight selection cannot reopen the panel after an intentional close.
export function cancelPendingSelection(): void {
  ++selectSeq;
}

/**
 * #2443 re-land (nav-lockup diagnosis §7, option 2 — single-writer):
 * `contextOverride` lets a caller that has ALREADY resolved the target note's
 * project/folder (e.g. wikiLinkNavigation.ts's exact-match branch) thread
 * those ids into THIS function's own canvasState.update() call below, instead
 * of writing canvasState via a separate navigateFn() call first. That old
 * two-write pattern raced against this function's unconditional
 * `view: 'note_editor'` write-back and produced a confirmed flip-flop — see
 * docs/audits/2026-08-05-nav-lockup-diagnosis.md §2. With contextOverride,
 * there is exactly one canvasState write per selectNote() call, always.
 */
export async function selectNote(
  note: NoteMeta,
  contextOverride?: { projectId?: string; folderId?: string }
): Promise<void> {
  const mySeq = ++selectSeq;
  const current = get(activeNote);
  
  // In v9, IDs are UUIDs not path-based. Use slug and notebook directly.
  const correctSlug = note.slug || note.id;
  const correctNotebook = note.notebook || '';
  if (correctNotebook !== note.notebook || correctSlug !== note.slug) {
    note = { ...note, notebook: correctNotebook, slug: correctSlug };
  }
  
  // Flush current note save if needed (Editor handles its own save on destroy)
  logActiveNoteSet('selectNote[pre-null]', get(activeNote), null);
  activeNote.set(null);
  
  // Await the editor's in-flight onDestroy save (set by Editor.svelte) instead of a heuristic delay
  await editorSavePromise;
  await new Promise(r => setTimeout(r, 20)); // DB flush buffer
  
  try {
    const noteWithContent = await api.readNote(note.notebook, note.slug);
    
    // Read task fields DIRECTLY from DB (most reliable source)
    let dbFields: Record<string, string | null> = {};
    try {
      dbFields = await api.getNoteTaskFields(note.id);
    } catch (e) {
      // No task fields
    }
    
    const enrichedMeta: NoteMeta = noteWithContent.meta ? {
      ...noteWithContent.meta,
      slug: noteWithContent.meta.slug || noteWithContent.meta.id || note.slug || note.id,
      notebook: noteWithContent.meta.notebook || note.notebook || '',
      title: dbFields.title || noteWithContent.meta?.title || note.title,
      task_status: dbFields.task_status || noteWithContent.meta.task_status || note.task_status,
      task_priority: dbFields.task_priority || noteWithContent.meta.task_priority || note.task_priority,
      task_due_date: dbFields.task_due_date || noteWithContent.meta.task_due_date || note.task_due_date,
      task_assignee: dbFields.task_assignee || noteWithContent.meta.task_assignee || note.task_assignee,
    } as NoteMeta : note;
    const enriched: NoteContent = {
      ...noteWithContent,
      meta: enrichedMeta,
    };
    if (mySeq !== selectSeq) return; // Superseded by a newer click — discard
    logActiveNoteSet('selectNote[set-enriched]', get(activeNote), enriched);
    activeNote.set(enriched);
    
    // FIX #619: Always update canvasState context with current note details
    // AI reads noteId from canvasState.context.noteId — must stay in sync with activeNote
    canvasState.update(state => ({
      ...state,
      view: 'note_editor',
      context: {
        ...state.context,
        // #2443 re-land: contextOverride (when provided) carries the TARGET
        // note's own project_id/folder_id — e.g. wiki-link navigation to a
        // note in a different project/folder than the one currently active.
        // Falls back to whatever was already in context (unchanged behavior
        // for every other caller) when no override is passed.
        projectId: contextOverride?.projectId ?? state.context.projectId,
        folderId: contextOverride?.folderId ?? state.context.folderId,
        noteId: enrichedMeta.id || enrichedMeta.slug || note.id,
        noteTitle: enrichedMeta.title,
        projectName: enrichedMeta.notebook?.split('/')[0] || enrichedMeta.notebook,
        taskId: undefined  // Clear stale task context
      }
    }));
  } catch (e) {
    console.error('[NOTE-SELECT] Failed to read note:', e);
    if (mySeq !== selectSeq) return; // Superseded — don't clobber with error placeholder
    // Set a placeholder to avoid infinite loading state
    const _placeholderNote = {
      meta: note,
      content: '',
      jsonContent: null
    };
    logActiveNoteSet('selectNote[error-placeholder]', get(activeNote), _placeholderNote);
    activeNote.set({
      meta: note,
      content: '',
      jsonContent: null
    });
  }
}

export async function deleteNote(note: NoteMeta): Promise<void> {
  const current = get(activeNote);
  notes.update(n => n.filter(x => x.id !== note.id));
  if (current?.meta.id === note.id) {
    logActiveNoteSet('deleteNote', get(activeNote), null);
    activeNote.set(null);
  }
  try {
    await api.moveToTrash(note.id);
  } catch (e) {
    console.error('[NOTE-SERVICE] moveToTrash FAILED:', e);
  }
}

export async function loadNotes(notebook: string, forceReload = false): Promise<void> {
  // Load notes for this notebook, keeping only just-created notes from the SAME notebook
  const loaded = await api.listNotes(notebook);
  if (forceReload) {
    // Hard replace — no merge. Used after delete to prevent ghost notes.
    notes.set(loaded);
  } else {
    notes.update(current => {
      const loadedIds = new Set(loaded.map(n => n.id));
      // Only keep extras that belong to this notebook (just-created, not yet indexed)
      const extras = current.filter(n => !loadedIds.has(n.id) && n.notebook === notebook);
      return [...extras, ...loaded];
    });
  }
}

export async function loadNotesByProject(projectId: string): Promise<void> {
  const loaded = await api.listNotesByProject(projectId);
  notes.update(current => {
    const loadedIds = new Set(loaded.map(n => n.id));
    const extras = current.filter(n => !loadedIds.has(n.id));
    return [...extras, ...loaded];
  });
}

export async function loadNotesByFolder(folderId: string): Promise<void> {
  const loaded = await api.listNotesByFolder(folderId);
  notes.update(current => {
    const loadedIds = new Set(loaded.map(n => n.id));
    const extras = current.filter(n => !loadedIds.has(n.id));
    return [...extras, ...loaded];
  });
}

export async function saveNote(slug: string, notebook: string, md: string, title?: string): Promise<void> {
  await api.saveNote(notebook, slug, md, title);
}

export async function loadNoteWithJson(notebook: string, slug: string): Promise<NoteContent | null> {
  try {
    return await api.readNote(notebook, slug);
  } catch (error) {
    console.error('[NOTE-SERVICE] Failed to load note:', error);
    return null;
  }
}

// === UNIFIED NOTE/TASK FUNCTIONS (v7.0) ===

export async function promoteToTask(noteId: string, status: TaskStatus = 'todo'): Promise<void> {
  // Use updateTaskStatus — promoteNoteToTask was removed (it hardcoded status: 'task').
  await api.updateTaskStatus(noteId, status);
  
  // Update local store to reflect task status
  notes.update(current => 
    current.map(n => 
      n.id === noteId 
        ? { ...n, task_status: status, task_sort_order: 0 }
        : n
    )
  );
}

export async function demoteFromTask(noteId: string): Promise<void> {
  await api.demoteNoteFromTask(noteId);
  
  // Update local store to clear task fields
  notes.update(current => 
    current.map(n => 
      n.id === noteId 
        ? { 
            ...n, 
            task_status: undefined,
            task_priority: undefined,
            task_due_date: undefined,
            task_assignee: undefined,
            task_completed_at: undefined,
            task_sort_order: 0
          }
        : n
    )
  );
}

export async function updateTaskFields(
  noteId: string, 
  fields: {
    status?: TaskStatus;
    priority?: TaskPriority;
    dueDate?: string;
    assignee?: string;
    tags?: string;
  }
): Promise<void> {
  // Optimistic update — move the note in the UI immediately
  notes.update(current => 
    current.map(n => 
      n.id === noteId 
        ? { 
            ...n,
            task_status: fields.status !== undefined ? fields.status : n.task_status,
            task_priority: fields.priority !== undefined ? fields.priority : n.task_priority,
            task_due_date: fields.dueDate !== undefined ? fields.dueDate : n.task_due_date,
            task_assignee: fields.assignee !== undefined ? fields.assignee : n.task_assignee,
            task_completed_at: fields.status === 'done' ? new Date().toISOString() : n.task_completed_at,
            updated_at: new Date().toISOString()
          }
        : n
    )
  );
  
  // Persist to DB (non-blocking — UI already updated)
  try {
    await api.updateNoteTaskFields(
      noteId,
      fields.status,
      fields.priority,
      fields.dueDate,
      fields.assignee,
      fields.tags
    );
  } catch (e) {
    console.error('Failed to persist task fields to DB:', e);
  }
  
  // Emit event so views can react to task field changes
  window.dispatchEvent(new CustomEvent('task-fields-updated', { detail: { noteId, fields } }));
}

export async function listTasks(filters?: { 
  status?: string; 
  priority?: string; 
  folderId?: string; 
}): Promise<NoteMeta[]> {
  const taskNotes = await api.listTaskNotes(
    filters?.status,
    filters?.priority, 
    filters?.folderId
  );
  return taskNotes;
}

export async function listTasksByFolder(folderId: string): Promise<NoteMeta[]> {
  return api.listTaskNotesByFolder(folderId);
}
