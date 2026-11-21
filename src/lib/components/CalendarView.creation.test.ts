/**
 * CalendarView.creation.test.ts
 *
 * Tests for the CalendarView task-creation guard introduced in fix/2405-calendar-creation-picker.
 *
 * Scope: calendarCreation.ts — the module that CalendarView.svelte calls from confirmCreateTask.
 *
 * Red-on-revert contract:
 *   - Test 1 (creation call carries picker IDs): reverts red if CalendarView reverts to
 *     `api.createNoteInProject('', undefined, ...)` — the empty-string call never passes
 *     the non-empty projectId / folderId assertions.
 *   - Test 2 (no selection → no creation): reverts red if the `if (!projectId || !folderId) return`
 *     guard is removed — the API call would fire and `not.toHaveBeenCalled()` would fail.
 *
 * Relates to #2405 / spec ruling 2026-08-04.
 */

import { describe, it, expect, vi, beforeEach } from 'vitest';

// ─── Mock $lib/api before importing the module under test ────────────────────
// Note: vi.mock is hoisted — factory must use only vi.fn() calls, no outer-scope variables.

vi.mock('$lib/api', () => ({
  createNoteInProject: vi.fn(),
  noteMetaToTask: vi.fn(),
  listProjects: vi.fn().mockResolvedValue([]),
  listTasks: vi.fn().mockResolvedValue([]),
}));

// Import AFTER mock registration — order matters for hoisting
import { createCalendarTask } from './calendarCreation';
import * as api from '$lib/api';

// ─── Test fixtures ────────────────────────────────────────────────────────────

const MOCK_NOTE_META = {
  id: 'note-abc123',
  title: 'New Task',
  status: 'todo',
  project_id: 'proj-abc',
  folder_id: 'folder-xyz',
  notebook: 'MyProject/General',
  due_date: '2026-08-10',
  created_at: new Date().toISOString(),
  updated_at: new Date().toISOString(),
};

const MOCK_TASK = {
  id: 'note-abc123',
  title: 'New Task',
  status: 'todo' as const,
  project_id: 'proj-abc',
  folder_id: 'folder-xyz',
  due_date: '2026-08-10',
};

// ─── Tests ────────────────────────────────────────────────────────────────────

describe('createCalendarTask — CalendarView creation guard (#2405)', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(api.createNoteInProject).mockResolvedValue(MOCK_NOTE_META as any);
    vi.mocked(api.noteMetaToTask).mockReturnValue(MOCK_TASK as any);
  });

  // ─── Test 1: Creation call carries the picked project_id and folder_id ────

  it('calls createNoteInProject with the selected projectId and folderId (not empty strings)', async () => {
    const PROJECT_ID = 'proj-abc';
    const FOLDER_ID = 'folder-xyz';
    const DATE = '2026-08-10';

    await createCalendarTask(PROJECT_ID, FOLDER_ID, DATE);

    expect(api.createNoteInProject).toHaveBeenCalledTimes(1);
    expect(api.createNoteInProject).toHaveBeenCalledWith(
      PROJECT_ID,          // must be the selected project, NOT ''
      FOLDER_ID,           // must be the selected folder, NOT undefined
      'New Task',
      'todo',
      { dueDate: DATE }
    );

    // Sanity: ensure the returned task is forwarded
    expect(api.noteMetaToTask).toHaveBeenCalledWith(MOCK_NOTE_META);
  });

  it('returns the task returned by noteMetaToTask', async () => {
    const result = await createCalendarTask('proj-abc', 'folder-xyz', '2026-08-10');
    expect(result).toBe(MOCK_TASK);
  });

  // ─── Test 2: No selection → no creation ───────────────────────────────────

  it('does NOT call createNoteInProject when no project is selected (empty projectId)', async () => {
    await createCalendarTask('', 'folder-xyz', '2026-08-10');
    expect(api.createNoteInProject).not.toHaveBeenCalled();
  });

  it('does NOT call createNoteInProject when no folder is selected (empty folderId)', async () => {
    await createCalendarTask('proj-abc', '', '2026-08-10');
    expect(api.createNoteInProject).not.toHaveBeenCalled();
  });

  it('returns null when guard fires (no project selected)', async () => {
    const result = await createCalendarTask('', 'folder-xyz', '2026-08-10');
    expect(result).toBeNull();
  });

  it('returns null when guard fires (no folder selected)', async () => {
    const result = await createCalendarTask('proj-abc', '', '2026-08-10');
    expect(result).toBeNull();
  });

  it('returns null when both projectId and folderId are empty', async () => {
    const result = await createCalendarTask('', '', '2026-08-10');
    expect(result).toBeNull();
    expect(api.createNoteInProject).not.toHaveBeenCalled();
  });
});
