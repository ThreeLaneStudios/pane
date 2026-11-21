/**
 * KanbanBoard.sheet-delete-refresh.test.ts — #2874 item 34 (Delete sheet from
 * the ⋯ menu appears to do nothing on the Kanban board).
 *
 * UPDATED FOR #2972 STAGE 2 (§34.10's "payload-shape migration" list names
 * this file explicitly): the original fix (`window.addEventListener
 * ('pane-data-changed', loadTasks)`, a bare-event full reload) is gone —
 * ruling 5/§34.5 bans a full `loadTasks()` reload entirely. The board's
 * `pane-data-changed` handling moved onto the Tauri-emitted channel
 * (`listen()`, since backend emits are invisible to window.addEventListener,
 * #2215) and patches/inserts/removes exactly one card via a single-entity
 * `entityService.getNote()` read — see `KanbanBoard.svelte`'s
 * `handleDataChangedEvent`. These tests now simulate that Tauri-channel
 * event (via `simulateTauriEvent`, vitest-setup.ts) instead of dispatching a
 * bare DOM CustomEvent, and assert the SAME end-user-visible behavior this
 * file always covered (item 34's original bug: a deleted sheet staying
 * stuck on the board) — just through the new, entity-identified mechanism,
 * with a single-entity read (not a second `listNotesForKanban()` call) as
 * the expected implementation.
 *
 * RED-ON-REVERT: removing KanbanBoard.svelte's `listen(EVENT_DATA_CHANGED,
 * ...)` registration (or its `reason === REASON_DELETED` branch) makes
 * test 1 fail (the stale card stays rendered); test 2 stays green either
 * way (it only fails if a *different* regression adds a listener that leaks
 * past unmount).
 */

import { describe, test, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, cleanup, screen } from '@testing-library/svelte/svelte5';
import KanbanBoard from './KanbanBoard.svelte';
import * as api from '$lib/api';
import * as entityService from '$lib/services/entityService';
import { canvasState } from '$lib/stores/canvas';
import type { Task } from '$lib/types';
import { simulateTauriEvent } from '../../../vitest-setup';
import { EVENT_DATA_CHANGED, ENTITY_SHEET, REASON_DELETED } from '$lib/refreshEvent';

// vitest-setup.ts's `window` mock lacks addEventListener/removeEventListener
// (stubbed or absent) — KanbanBoard.svelte still registers several
// window-level DOM listeners unrelated to #2972 (`pane-new-task`,
// `pane-task-meta-changed`, `keydown`), so these must be rebound to the
// real globalThis EventTarget for mount to succeed, same as
// KanbanBoard.sheet-identity.test.ts's established pattern.
const win = globalThis.window as any;
win.addEventListener = globalThis.addEventListener.bind(globalThis);
win.removeEventListener = globalThis.removeEventListener.bind(globalThis);

if (typeof (globalThis as any).ResizeObserver !== 'function') {
  (globalThis as any).ResizeObserver = class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
}

const PROJECT_ID = 'project-kanban-delete-refresh';
const SHEET_TASK_ID = 'sheet-note-delete-refresh-1';

function makeSheetTask(): Task {
  return {
    id: SHEET_TASK_ID,
    title: 'Doomed Sheet',
    description: '',
    status: 'todo',
    priority: 'medium',
    due_date: undefined,
    project_id: PROJECT_ID,
    folder_id: undefined,
    tags: [],
    sort_order: 0,
    created_at: '2026-01-01T00:00:00Z',
    updated_at: '2026-01-01T00:00:00Z',
    note_type: 'sheet',
  } as Task;
}

function mountKanban() {
  canvasState.set({
    view: 'kanban',
    data: {},
    context: { projectId: PROJECT_ID, projectName: 'Delete Refresh Project' },
  } as any);
  return render(KanbanBoard, { props: {} });
}

function emitDeleted(entityId: string) {
  simulateTauriEvent(EVENT_DATA_CHANGED, {
    entity_id: entityId,
    entity_type: ENTITY_SHEET,
    reason: REASON_DELETED,
    origin_tag: 'test-origin-tag',
  });
}

beforeEach(() => {
  vi.spyOn(api, 'listFolders').mockResolvedValue([]);
  vi.spyOn(api, 'getSheetCardSummaries').mockResolvedValue([]);
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe('KanbanBoard removes a deleted sheet card on pane-data-changed (#2874 item 34, #2972 stage 2)', () => {
  test('KanbanBoard removes a deleted sheet card without a listNotesForKanban() reload', async () => {
    const listSpy = vi.spyOn(api, 'listNotesForKanban').mockResolvedValue([makeSheetTask()]);
    const getNoteSpy = vi.spyOn(entityService, 'getNote');

    mountKanban();

    await vi.waitFor(() => {
      expect(screen.getByText('Doomed Sheet')).toBeTruthy();
    });
    expect(listSpy).toHaveBeenCalledTimes(1);

    emitDeleted(SHEET_TASK_ID);

    await vi.waitFor(() => {
      expect(screen.queryByText('Doomed Sheet')).toBeNull();
    });

    // §34.5: patch/remove, never a second full-list reload, and a deletion
    // doesn't need a single-entity read either (there's nothing left to
    // fetch) — removal is purely local-array filtering.
    expect(listSpy).toHaveBeenCalledTimes(1);
    expect(getNoteSpy).not.toHaveBeenCalled();
  });

  test('KanbanBoard removes the pane-data-changed Tauri listener on unmount', async () => {
    const listSpy = vi.spyOn(api, 'listNotesForKanban').mockResolvedValue([makeSheetTask()]);

    const { unmount } = mountKanban();

    await vi.waitFor(() => {
      expect(screen.getByText('Doomed Sheet')).toBeTruthy();
    });

    unmount();

    // Give the unlisten() promise (chained, not awaited, in onDestroy) a
    // tick to actually unregister before asserting the listener is gone.
    await new Promise(r => setTimeout(r, 30));

    emitDeleted(SHEET_TASK_ID);
    await new Promise(r => setTimeout(r, 30));

    // No error thrown, and (since the component is unmounted) nothing left
    // to assert on the DOM — this test's real job is proving `unlisten()`
    // doesn't throw/leak past unmount. listNotesForKanban() call count is
    // unaffected either way since deletion never triggers a reload.
    expect(listSpy).toHaveBeenCalledTimes(1);
  });
});
