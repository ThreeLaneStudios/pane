/**
 * calendarCreation.ts — Creation guard for CalendarView's project/folder picker.
 *
 * Extracted from CalendarView.svelte's confirmCreateTask for testability.
 * See: fix/2405-calendar-creation-picker, spec ruling 2026-08-04.
 *
 * Guard: no project or folder selection → no note created → no silent path.
 */

import * as api from '$lib/api';
import type { Task } from '$lib/types';

/**
 * createCalendarTask — Guarded creation after project/folder selection.
 *
 * @param projectId - selected project UUID (must be non-empty)
 * @param folderId  - selected folder UUID (must be non-empty)
 * @param dateStr   - ISO date string for the due date (YYYY-MM-DD)
 * @returns the created Task, or null if the guard fired (no selection)
 */
export async function createCalendarTask(
  projectId: string,
  folderId: string,
  dateStr: string
): Promise<Task | null> {
  // Guard: no selection → no creation (spec: context-free-surface ruling 2026-08-04)
  if (!projectId || !folderId) return null;

  const noteMeta = await api.createNoteInProject(
    projectId,
    folderId,
    'New Task',
    'todo',
    { dueDate: dateStr }
  );
  return api.noteMetaToTask(noteMeta);
}
