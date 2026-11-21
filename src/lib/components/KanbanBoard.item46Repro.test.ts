/**
 * KanbanBoard.item46Repro.test.ts — #2972 Stage 4 (`docs/spec/
 * data-refresh.md` §34.10 Stage 4 acceptance criteria, item 46's literal
 * repro): "an agent creates/edits a sheet → the board updates without
 * navigating away."
 *
 * The underlying mechanism (`KanbanBoard.svelte`'s `handleDataChangedEvent`,
 * entity_type === 'note' | 'sheet', patch/insert/remove via a single
 * `api.readNote()` read, never a full `loadTasks()`) already landed in
 * Stage 2/3 and is covered generically (mostly via `ENTITY_NOTE`) by
 * `KanbanBoard.dataChangedHandler.test.ts`. This file is Stage 4's
 * explicit, named verification of item 46's own scenario — SHEETS
 * specifically, both halves (create AND edit) in one place, run against
 * `main` at Stage 4's base — not a new code path.
 *
 * Honest result (per the Stage 4 task's own instruction: "the
 * verification repros... may pass at base — say so honestly rather than
 * manufacturing a failure"): both tests below PASS against `main` as-is.
 * Stage 2/3 already closed this gap; this file exists to pin it as a
 * named, permanent regression guard referencing item 46 directly, not to
 * demonstrate a fix. (Confirmed: these tests were run once with
 * `loadTasks` call-count assertions added, and loadTasks is called
 * exactly once — the mount-time load — for either scenario, never a
 * second time.)
 */

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, cleanup, screen } from '@testing-library/svelte/svelte5';
import KanbanBoard from './KanbanBoard.svelte';
import * as api from '$lib/api';
import { canvasState } from '$lib/stores/canvas';
import { mockData, simulateTauriEvent } from '../../../vitest-setup';
import { EVENT_DATA_CHANGED, ENTITY_SHEET, REASON_CREATED, REASON_EDITED } from '$lib/refreshEvent';

const win = globalThis.window as any;
win.addEventListener = globalThis.addEventListener.bind(globalThis);
win.removeEventListener = globalThis.removeEventListener.bind(globalThis);
win.dispatchEvent = globalThis.dispatchEvent.bind(globalThis);

if (typeof (globalThis as any).ResizeObserver !== 'function') {
  (globalThis as any).ResizeObserver = class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
}

const PROJECT_ID = 'project-item46-board';

function mountKanban() {
  canvasState.set({
    view: 'kanban',
    data: {},
    context: { projectId: PROJECT_ID, projectName: 'Item 46 Repro Project' },
  } as any);
  return render(KanbanBoard, { props: {} });
}

function emit(entityId: string, entityType: string, reason: string) {
  simulateTauriEvent(EVENT_DATA_CHANGED, {
    entity_id: entityId,
    entity_type: entityType,
    reason,
    origin_tag: 'item46-origin-tag',
  });
}

beforeEach(() => {
  mockData.tasks = [];
  mockData.noteContents = {};
  mockData.nextId = 1;
  mockData.projects = [
    { id: PROJECT_ID, name: 'Item 46 Repro Project', parent_id: null,
      created_at: new Date().toISOString(), updated_at: new Date().toISOString() },
  ];
  vi.spyOn(api, 'listFolders').mockResolvedValue([]);
  vi.spyOn(api, 'getSheetCardSummaries').mockResolvedValue([]);
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe('item 46 repro — agent creates/edits a sheet, board updates without navigating away (#2972 stage 4)', () => {
  it('agent-created sheet appears on the board without a full loadTasks() reload or leaving the view', async () => {
    const listSpy = vi.spyOn(api, 'listNotesForKanban').mockResolvedValue([]);

    mountKanban();
    await vi.waitFor(() => expect(listSpy).toHaveBeenCalledTimes(1));
    expect(screen.queryByText('Agent-Created Sheet')).toBeNull();

    const now = new Date().toISOString();
    mockData.tasks.push({
      id: 'sheet-item46-created',
      title: 'Agent-Created Sheet',
      content_preview: '',
      status: 'cabinet',
      priority: 'medium',
      due_date: null,
      project_id: PROJECT_ID,
      folder_id: null,
      note_type: 'sheet',
      tags: [],
      created_at: now,
      updated_at: now,
    } as any);

    // The agent's create_sheet tool call -> backend emit (Stage 1) ->
    // this frontend listener (Stage 2) — no navigation, no remount.
    emit('sheet-item46-created', ENTITY_SHEET, REASON_CREATED);

    await vi.waitFor(() => {
      expect(screen.getByText('Agent-Created Sheet')).toBeTruthy();
    });

    // §34.5: inserted via the single-entity read, never a second
    // loadTasks() reload — proving "without navigating away" (no
    // route/view change ever happened, and no board-wide refetch either).
    expect(listSpy).toHaveBeenCalledTimes(1);
  });

  it('agent-edited sheet (already on the board) patches in place without a full loadTasks() reload', async () => {
    const now = new Date().toISOString();
    mockData.tasks = [{
      id: 'sheet-item46-edited',
      title: 'Original Sheet Title',
      content_preview: '',
      status: 'cabinet',
      priority: 'medium',
      due_date: null,
      project_id: PROJECT_ID,
      folder_id: null,
      note_type: 'sheet',
      tags: [],
      created_at: now,
      updated_at: now,
    } as any];
    const listSpy = vi.spyOn(api, 'listNotesForKanban').mockResolvedValue([...mockData.tasks] as any);

    mountKanban();
    await vi.waitFor(() => expect(screen.getByText('Original Sheet Title')).toBeTruthy());
    listSpy.mockClear();

    // Agent edits the sheet's title via a sheet tool call elsewhere.
    mockData.tasks[0] = { ...mockData.tasks[0], title: 'Agent-Edited Sheet Title', updated_at: new Date().toISOString() };
    emit('sheet-item46-edited', ENTITY_SHEET, REASON_EDITED);

    await vi.waitFor(() => {
      expect(screen.getByText('Agent-Edited Sheet Title')).toBeTruthy();
    });
    expect(screen.queryByText('Original Sheet Title')).toBeNull();

    // No second full-board reload for the edit either.
    expect(listSpy).toHaveBeenCalledTimes(0);
  });
});
