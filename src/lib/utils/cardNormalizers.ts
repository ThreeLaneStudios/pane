/**
 * cardNormalizers.ts
 * Pure mapping functions: Task/NoteMeta → CardData for unified Card.svelte.
 * No UI logic, no side effects, no imports from Svelte.
 */

import type { NoteMeta, Task, TaskStatus, TaskPriority } from '$lib/types';

/**
 * Map a `NoteMeta` (single-entity read shape, e.g. `entityService.getNote()`'s
 * `.meta`) to a `Task` (#2972 stage 2, §34.5) — the shape KanbanBoard's/
 * AllTasksView's/CalendarView's `tasks` arrays use. Shared here (rather than
 * duplicated per-screen) so a note fetched after a `pane-data-changed` event
 * is inserted/patched identically everywhere, matching the existing
 * `listNotesForKanban()` mapping (`api.ts`) field-for-field.
 */
export function noteMetaToTask(meta: NoteMeta): Task {
  return {
    id: meta.id,
    title: meta.title || 'Untitled',
    description: meta.content_preview || meta.preview || '',
    status: (meta.task_status || 'cabinet') as TaskStatus,
    priority: (meta.task_priority || 'medium') as TaskPriority,
    due_date: meta.task_due_date,
    assignee: meta.task_assignee,
    tags: meta.tags || [],
    sort_order: meta.task_sort_order || 0,
    created_at: meta.created_at,
    updated_at: meta.updated_at,
    project_id: meta.project_id,
    folder_id: meta.folder_id,
    note_type: meta.note_type,
  };
}

/** Unified shape consumed by Card.svelte */
export interface CardData {
  id: string;
  title: string;
  bodyText?: string;        // Pre-processed: markdown stripped for notes, plain text for tasks
  status?: TaskStatus;      // undefined = non-task note
  priority?: TaskPriority;
  dueDate?: string;         // ISO date string
  assignee?: string;
  projectName?: string;
  folderName?: string;
  subtaskCount?: number;    // default 0

  // Sheet card face fields (#2339 PR 15a). All undefined for non-sheet
  // cards — Card.svelte branches on `noteType === 'sheet'` to decide
  // whether to read these at all, so a non-sheet CardData is unaffected
  // by their presence/absence here.
  noteType?: string;         // 'sheet' selects the micro-grid card face; anything else (including undefined) is the note-card path, byte-identical to pre-PR-15a behavior
  colCount?: number;         // sheet_columns count, from get_sheet_card_summary
  rowCount?: number;         // sheet_rows count, from get_sheet_card_summary
  formulaCount?: number;     // '='-prefixed sheet_cells count, from get_sheet_card_summary
  colTypes?: string[];       // ordered column type list (drives the micro-grid header row's TypeGlyph icons), from get_sheet_card_summary
}

/**
 * Strip markdown from a preview string.
 * Mirrors the logic from NoteCard.svelte (previously getPreviewText).
 */
export function getPreviewText(preview: string | undefined): string {
  if (!preview) return '';
  return preview
    .split('\n')
    .map(l => l.trim())
    .filter(l => l.length > 0)
    .map(l => {
      // Strip heading markers but keep text
      l = l.replace(/^#+\s+/, '');
      // Strip bold/italic/strikethrough
      l = l.replace(/\*\*(.+?)\*\*/g, '$1');
      l = l.replace(/\*(.+?)\*/g, '$1');
      l = l.replace(/__(.+?)__/g, '$1');
      l = l.replace(/_(.+?)_/g, '$1');
      l = l.replace(/~~(.+?)~~/g, '$1');
      // Strip list markers
      l = l.replace(/^[-*+]\s+/, '');
      l = l.replace(/^\d+\.\s+/, '');
      return l;
    })
    .filter(l => l.length > 0)
    .join(' ');
}

/**
 * Map a NoteMeta to CardData.
 * @param note       Source NoteMeta record
 * @param opts       Optional subtask count
 */
export function normalizeNoteMeta(
  note: NoteMeta,
  opts: {
    subtaskCount?: number;
    /** Fallback project name when note.notebook is empty (e.g. listNotesByFolder views) */
    projectNameOverride?: string;
    /** Fallback folder name when note.notebook has no subfolder segment */
    folderNameOverride?: string;
    /** Sheet card summary (#2339 PR 15a) — caller fetches this separately
     *  (get_sheet_card_summary/get_sheet_card_summaries) and passes it in
     *  here for sheet notes only. Omitted entirely for non-sheet notes,
     *  which keeps this function's non-sheet return value byte-identical
     *  to its pre-PR-15a shape. */
    sheetSummary?: { colCount: number; rowCount: number; formulaCount: number; colTypes: string[] };
  } = {}
): CardData {
  const parts = (note.notebook || '').split('/');
  const projectName = parts[0] || undefined;
  const folderName = parts.length > 1 ? parts.slice(1).join('/') : undefined;

  const base: CardData = {
    id: note.id,
    title: note.title || 'Untitled',
    bodyText: getPreviewText(note.preview || note.content_preview || ''),
    status: note.task_status,
    priority: note.task_priority,
    dueDate: note.task_due_date,
    assignee: note.task_assignee,
    // Use override when the notebook-derived value is missing
    projectName: projectName || opts.projectNameOverride,
    folderName: folderName || opts.folderNameOverride,
    subtaskCount: opts.subtaskCount ?? 0,
  };

  // Sheet fields are only ever attached when note_type === 'sheet' — the
  // non-sheet return value above is otherwise unchanged from pre-PR-15a.
  if (note.note_type === 'sheet') {
    base.noteType = 'sheet';
    if (opts.sheetSummary) {
      base.colCount = opts.sheetSummary.colCount;
      base.rowCount = opts.sheetSummary.rowCount;
      base.formulaCount = opts.sheetSummary.formulaCount;
      base.colTypes = opts.sheetSummary.colTypes;
    }
  }

  return base;
}

/**
 * Map a Task to CardData.
 * @param task          Source Task record
 * @param projectName   Project display name (from canvasState or parent context)
 * @param folderNameMap Map<folder_id, folder_name> built by KanbanBoard from api.listFolders()
 */
export function normalizeTask(
  task: Task,
  projectName: string,
  folderNameMap: Map<string, string>,
  /** Sheet card summary (#2339 PR 15a) — see normalizeNoteMeta's doc for
   *  why this is a separate opt-in param rather than baked into `Task`. */
  sheetSummary?: { colCount: number; rowCount: number; formulaCount: number; colTypes: string[] }
): CardData {
  const base: CardData = {
    id: task.id,
    title: task.title,
    bodyText: task.description || '',
    status: task.status,
    priority: task.priority,
    dueDate: task.due_date,
    assignee: task.assignee,
    projectName,
    folderName: task.folder_id ? folderNameMap.get(task.folder_id) : undefined,
    subtaskCount: 0,
  };

  if (task.note_type === 'sheet') {
    base.noteType = 'sheet';
    if (sheetSummary) {
      base.colCount = sheetSummary.colCount;
      base.rowCount = sheetSummary.rowCount;
      base.formulaCount = sheetSummary.formulaCount;
      base.colTypes = sheetSummary.colTypes;
    }
  }

  return base;
}
