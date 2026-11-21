/**
 * Tests for listNotesForKanban (api.ts) — fix/1935-kanban-cabinet-regression
 *
 * Verifies that the kanban-only data path:
 *   1. Returns cabinet-status notes (fix for bug 1: get_tasks excludes cabinet)
 *   2. Preserves task_status = 'cabinet' instead of mapping to undefined (fix for bug 2)
 *   3. Does NOT apply the || 'todo' fallback that would misclassify cabinet notes (fix for bug 3)
 *   4. Correctly returns todo/in_progress/done notes alongside cabinet notes
 */

import { describe, it, expect, beforeEach } from 'vitest';
import { listNotesForKanban } from '$lib/api';
import { mockData } from '../../vitest-setup';

const PROJECT_ID = 'project-test';

function makeNote(overrides: Record<string, any>) {
  const id = 'note-kanban-' + Math.random().toString(36).slice(2);
  return {
    id,
    title: 'Test note',
    content_preview: '',
    status: null,
    priority: 'medium',
    due_date: null,
    project_id: PROJECT_ID,
    folder_id: null,
    parent_id: null,
    note_type: null,
    deleted_at: null,
    created_at: new Date().toISOString(),
    updated_at: new Date().toISOString(),
    ...overrides,
  };
}

describe('listNotesForKanban', () => {
  beforeEach(() => {
    mockData.tasks = [];
    mockData.nextId = 1;
  });

  it('returns cabinet-status notes (bug 1 fix: no AND status != cabinet filter)', async () => {
    mockData.tasks = [makeNote({ status: 'cabinet', title: 'Cabinet note' })];
    const result = await listNotesForKanban(PROJECT_ID);
    expect(result).toHaveLength(1);
    expect(result[0].title).toBe('Cabinet note');
  });

  it('maps status=cabinet to task status cabinet, NOT undefined (bug 2 fix: transformNoteMeta)', async () => {
    mockData.tasks = [makeNote({ status: 'cabinet', title: 'Cabinet note' })];
    const result = await listNotesForKanban(PROJECT_ID);
    expect(result[0].status).toBe('cabinet');
  });

  it('does NOT promote cabinet notes to todo via fallback (bug 3 fix: no || todo)', async () => {
    mockData.tasks = [makeNote({ status: 'cabinet', title: 'Cabinet note' })];
    const result = await listNotesForKanban(PROJECT_ID);
    expect(result[0].status).not.toBe('todo');
  });

  it('notes with no status get cabinet as default (new notes default to cabinet)', async () => {
    mockData.tasks = [makeNote({ status: null, title: 'New note' })];
    const result = await listNotesForKanban(PROJECT_ID);
    expect(result[0].status).toBe('cabinet');
  });

  it('returns todo notes correctly', async () => {
    mockData.tasks = [makeNote({ status: 'todo', title: 'Todo note' })];
    const result = await listNotesForKanban(PROJECT_ID);
    expect(result[0].status).toBe('todo');
  });

  it('returns in_progress notes correctly', async () => {
    mockData.tasks = [makeNote({ status: 'in_progress', title: 'WIP note' })];
    const result = await listNotesForKanban(PROJECT_ID);
    expect(result[0].status).toBe('in_progress');
  });

  it('returns done notes correctly', async () => {
    mockData.tasks = [makeNote({ status: 'done', title: 'Done note' })];
    const result = await listNotesForKanban(PROJECT_ID);
    expect(result[0].status).toBe('done');
  });

  it('returns all statuses together — cabinet, todo, in_progress, done', async () => {
    mockData.tasks = [
      makeNote({ status: 'cabinet', title: 'Cabinet' }),
      makeNote({ status: 'todo', title: 'Todo' }),
      makeNote({ status: 'in_progress', title: 'WIP' }),
      makeNote({ status: 'done', title: 'Done' }),
    ];
    const result = await listNotesForKanban(PROJECT_ID);
    expect(result).toHaveLength(4);
    const statuses = result.map(t => t.status).sort();
    expect(statuses).toEqual(['cabinet', 'done', 'in_progress', 'todo']);
  });

  it('filters by projectId — excludes notes from other projects', async () => {
    mockData.tasks = [
      makeNote({ status: 'cabinet', title: 'My project note', project_id: PROJECT_ID }),
      makeNote({ status: 'todo', title: 'Other project note', project_id: 'project-other' }),
    ];
    const result = await listNotesForKanban(PROJECT_ID);
    expect(result).toHaveLength(1);
    expect(result[0].title).toBe('My project note');
  });

  it('excludes deleted notes', async () => {
    mockData.tasks = [
      makeNote({ status: 'cabinet', title: 'Deleted', deleted_at: new Date().toISOString() }),
      makeNote({ status: 'todo', title: 'Active' }),
    ];
    const result = await listNotesForKanban(PROJECT_ID);
    expect(result).toHaveLength(1);
    expect(result[0].title).toBe('Active');
  });

  it('excludes subtasks (parent_id set)', async () => {
    mockData.tasks = [
      makeNote({ status: 'todo', title: 'Subtask', parent_id: 'some-parent' }),
      makeNote({ status: 'cabinet', title: 'Top-level' }),
    ];
    const result = await listNotesForKanban(PROJECT_ID);
    expect(result).toHaveLength(1);
    expect(result[0].title).toBe('Top-level');
  });

  it('excludes notes with a note_type set (daily, etc.)', async () => {
    mockData.tasks = [
      makeNote({ status: null, title: 'Daily log', note_type: 'daily' }),
      makeNote({ status: 'cabinet', title: 'Regular note' }),
    ];
    const result = await listNotesForKanban(PROJECT_ID);
    expect(result).toHaveLength(1);
    expect(result[0].title).toBe('Regular note');
  });

  it('maps priority from backend note', async () => {
    mockData.tasks = [makeNote({ status: 'todo', priority: 'urgent', title: 'Urgent note' })];
    const result = await listNotesForKanban(PROJECT_ID);
    expect(result[0].priority).toBe('urgent');
  });

  it('returns empty array for project with no notes', async () => {
    mockData.tasks = [];
    const result = await listNotesForKanban('project-empty');
    expect(result).toHaveLength(0);
  });
});
