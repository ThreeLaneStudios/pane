/**
 * KanbanBoard.collapse-persist.test.ts — #2874 Kanban item (this PR):
 * "every column collapsible, state persisted per project, Done defaults
 * collapsed."
 *
 * Before this PR: `KanbanBoard.svelte`'s column-header `onclick` was gated —
 * `(column.status === 'done' || getTasksForColumn(column.status).length > 5)
 * && toggleColumnCollapse(column.status)` — so a non-Done column with 5 or
 * fewer cards could not be collapsed at all (no cursor, no click handler, no
 * indicator). `collapsedColumns` was also a plain ephemeral `$state(new
 * Set(['done']))` with no persistence of any kind — it reset to
 * Done-only on every mount, including every project switch and every app
 * restart.
 *
 * These tests exercise the REAL `KanbanBoard.svelte` end-to-end against the
 * real `get_ai_setting`/`set_ai_setting` Tauri commands (mocked in
 * `vitest-setup.ts` by a real in-memory KV store, not a fire-and-forget
 * spy), so "persists across a reload" is proven by actually unmounting and
 * remounting the component against the same backing store — not asserted
 * via a mocked call count.
 *
 * All three would FAIL against the pre-fix code:
 *   - Test 1 fails because the 'To Do' column (2 cards, not Done) had no
 *     click handler at all — clicking its header does nothing.
 *   - Test 2 fails for the same reason (nothing to persist, since it can't
 *     be collapsed), and would additionally fail because `collapsedColumns`
 *     was never written to/read from any persistence layer.
 *   - Test 3 happens to pass at base (Done collapses by default already) —
 *     included anyway, named explicitly, to prove the default survived the
 *     persistence rewrite rather than silently regressing.
 */

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, cleanup, fireEvent, screen, within } from '@testing-library/svelte/svelte5';
import KanbanBoard from './KanbanBoard.svelte';
import { canvasState } from '$lib/stores/canvas';
import { activeNote } from '$lib/stores/app';
import { mockData } from '../../../vitest-setup';

// vitest-setup.ts replaces `globalThis.window` with a plain mock object
// lacking addEventListener/removeEventListener/ResizeObserver — same shim
// set KanbanBoard.sheet-identity.test.ts and KanbanBoard.new-sheet-button-polish.test.ts
// already use.
const win = globalThis.window as any;
if (typeof win.addEventListener !== 'function') {
  win.addEventListener = () => {};
}
if (typeof win.removeEventListener !== 'function') {
  win.removeEventListener = () => {};
}
if (typeof (globalThis as any).ResizeObserver !== 'function') {
  (globalThis as any).ResizeObserver = class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
}

const PROJECT_ID = 'project-kanban-collapse';

function seedTasks() {
  const now = new Date().toISOString();
  mockData.tasks.push(
    {
      id: 'todo-task-1', title: 'Todo Card One', content: '', content_preview: '',
      status: 'todo', priority: 'medium', due_date: null, assignee: null,
      project_id: PROJECT_ID, folder_id: null, parent_id: null, note_type: null,
      file_path: 'x.md', tags: [], pinned: false, word_count: 0, created_at: now, updated_at: now,
    } as any,
    {
      id: 'todo-task-2', title: 'Todo Card Two', content: '', content_preview: '',
      status: 'todo', priority: 'medium', due_date: null, assignee: null,
      project_id: PROJECT_ID, folder_id: null, parent_id: null, note_type: null,
      file_path: 'y.md', tags: [], pinned: false, word_count: 0, created_at: now, updated_at: now,
    } as any,
    {
      id: 'done-task-1', title: 'Done Card One', content: '', content_preview: '',
      status: 'done', priority: 'medium', due_date: null, assignee: null,
      project_id: PROJECT_ID, folder_id: null, parent_id: null, note_type: null,
      file_path: 'z.md', tags: [], pinned: false, word_count: 0, created_at: now, updated_at: now,
    } as any,
  );
}

beforeEach(() => {
  mockData.tasks = [];
  mockData.nextId = 1;
  mockData.noteContents = {};
  mockData.aiSettings = {};
  mockData.projects = [
    { id: PROJECT_ID, name: 'Collapse Project', parent_id: null,
      created_at: new Date().toISOString(), updated_at: new Date().toISOString() },
  ];
  seedTasks();
  canvasState.set({
    view: 'kanban',
    data: {},
    context: { projectId: PROJECT_ID, projectName: 'Collapse Project' },
  });
  activeNote.set(null);
});

afterEach(() => {
  cleanup();
  activeNote.set(null);
});

describe('KanbanBoard column collapse — every column collapsible, persisted per project (#2874)', () => {
  it('a non-Done column with \u22645 cards (To Do, 2 cards) can be collapsed by clicking its header', async () => {
    render(KanbanBoard, { props: {} });

    await screen.findByText('Todo Card One');
    const todoRegion = screen.getByRole('region', { name: 'To Do' });
    expect(within(todoRegion).queryByText('Todo Card One')).toBeTruthy();
    expect(todoRegion.className).not.toMatch(/collapsed/);

    const header = within(todoRegion).getByText('To Do', { selector: '.column-title' }).closest('.column-header') as HTMLElement;
    expect(header).toBeTruthy();
    await fireEvent.click(header);

    await vi.waitFor(() => {
      expect(todoRegion.className).toMatch(/collapsed/);
    });
    expect(within(todoRegion).queryByText('Todo Card One')).toBeNull();
  });

  it('collapsed state for a project survives a reload (real get_ai_setting/set_ai_setting round-trip, not a mocked call count)', async () => {
    const { unmount } = render(KanbanBoard, { props: {} });
    await screen.findByText('Todo Card One');

    const todoRegion = screen.getByRole('region', { name: 'To Do' });
    const header = within(todoRegion).getByText('To Do', { selector: '.column-title' }).closest('.column-header') as HTMLElement;
    await fireEvent.click(header);

    await vi.waitFor(() => {
      expect(todoRegion.className).toMatch(/collapsed/);
    });

    // Confirm the write actually landed in the (real-shaped) backing KV
    // store, not just in-memory component state.
    expect(mockData.aiSettings[`kanban:${PROJECT_ID}:todo:collapsed`]).toBe('true');

    // Simulate a reload: unmount entirely, then remount a fresh component
    // instance against the SAME canvasState/project — nothing left over in
    // the old component's local state can leak into this assertion.
    unmount();
    cleanup();
    activeNote.set(null);
    canvasState.set({
      view: 'kanban',
      data: {},
      context: { projectId: PROJECT_ID, projectName: 'Collapse Project' },
    });

    render(KanbanBoard, { props: {} });
    await screen.findByRole('region', { name: 'To Do' }); // settle initial load

    await vi.waitFor(() => {
      const reloadedTodoRegion = screen.getByRole('region', { name: 'To Do' });
      expect(reloadedTodoRegion.className).toMatch(/collapsed/);
      expect(within(reloadedTodoRegion).queryByText('Todo Card One')).toBeNull();
    });
  });

  it('Done still defaults collapsed with no persisted state for the project', async () => {
    render(KanbanBoard, { props: {} });
    await screen.findByText('Todo Card One');

    const doneRegion = screen.getByRole('region', { name: 'Done' });
    await vi.waitFor(() => {
      expect(doneRegion.className).toMatch(/collapsed/);
    });
    expect(within(doneRegion).queryByText('Done Card One')).toBeNull();
  });
});
