/**
 * KanbanBoard.dataChangedHandler.test.ts — #2972 stage 2 (`docs/spec/
 * data-refresh.md` §34, specifically §34.5's literal example and §34.10's
 * Stage 2 acceptance criteria: "Insert and remove branches tested
 * separately").
 *
 * KanbanBoard.svelte's `handleDataChangedEvent` (wired via
 * `listen(EVENT_DATA_CHANGED, ...)` — Tauri IPC, since backend emits are
 * invisible to window.addEventListener, #2215) is the board's one handler
 * for the single data-change event: it patches, inserts, or removes
 * exactly one card from `tasks`, never calling `loadTasks()` (a full
 * reload is banned by ruling 5/§34.5). This file is the sibling-parity
 * anchor §34.10 asks for — a regression test for that handler which names
 * both #2972 and §34 directly (this docstring, and each test below) — plus
 * explicit coverage of the insert and remove branches as separate tests
 * (the patch branch, and the "same Editor DOM node" guarantee, are covered
 * by `KanbanBoard.editor-survives-reload.test.ts`; plain removal is also
 * covered by `KanbanBoard.sheet-delete-refresh.test.ts` — this file is the
 * one place all three branches are asserted side by side against the real
 * component).
 */

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, cleanup, screen } from '@testing-library/svelte/svelte5';
import KanbanBoard from './KanbanBoard.svelte';
import * as api from '$lib/api';
import { canvasState } from '$lib/stores/canvas';
import { mockData, simulateTauriEvent } from '../../../vitest-setup';
import {
  EVENT_DATA_CHANGED,
  ENTITY_NOTE,
  ENTITY_SHEET,
  REASON_CREATED,
  REASON_EDITED,
  REASON_DELETED,
  dispatchDataChanged,
} from '$lib/refreshEvent';

const win = globalThis.window as any;
win.addEventListener = globalThis.addEventListener.bind(globalThis);
win.removeEventListener = globalThis.removeEventListener.bind(globalThis);
// #2972 stage 3 (item 4): unconditional, unlike the two above —
// vitest-setup.ts's fake `window` already defines a `dispatchEvent`
// property (a no-op stub). A conditional "only patch if missing" guard
// would see that stub as already present and never wire it to the real
// jsdom EventTarget, silently making every app-channel dispatch below a
// no-op that reaches no listener at all.
win.dispatchEvent = globalThis.dispatchEvent.bind(globalThis);

if (typeof (globalThis as any).ResizeObserver !== 'function') {
  (globalThis as any).ResizeObserver = class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
}

const PROJECT_ID = 'project-2972-stage2-board';

function mountKanban() {
  canvasState.set({
    view: 'kanban',
    data: {},
    context: { projectId: PROJECT_ID, projectName: '#2972 Stage 2 Board Project' },
  } as any);
  return render(KanbanBoard, { props: {} });
}

function emit(entityId: string, entityType: string, reason: string) {
  simulateTauriEvent(EVENT_DATA_CHANGED, {
    entity_id: entityId,
    entity_type: entityType,
    reason,
    origin_tag: 'test-origin-tag',
  });
}

// #2972 stage 3 (item 4, §34.10: "each screen updates on an app-channel
// event, not just backend"): the OTHER sender — a DOM CustomEvent via the
// shared `dispatchDataChanged` helper, exactly what a migrated app.ts/
// component sender now calls, instead of the Tauri `listen()` channel
// `emit()` above simulates.
function emitViaAppChannel(entityId: string, entityType: string, reason: string) {
  dispatchDataChanged(entityId, entityType as any, reason as any, 'test-origin-tag-app');
}

beforeEach(() => {
  mockData.tasks = [];
  mockData.noteContents = {};
  mockData.nextId = 1;
  mockData.projects = [
    { id: PROJECT_ID, name: '#2972 Stage 2 Board Project', parent_id: null,
      created_at: new Date().toISOString(), updated_at: new Date().toISOString() },
  ];
  vi.spyOn(api, 'listFolders').mockResolvedValue([]);
  vi.spyOn(api, 'getSheetCardSummaries').mockResolvedValue([]);
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe('KanbanBoard pane-data-changed handler — insert branch (#2972, §34.5)', () => {
  it('inserts a new card for a note created elsewhere (agent create_note), without a listNotesForKanban() reload', async () => {
    const listSpy = vi.spyOn(api, 'listNotesForKanban').mockResolvedValue([]);

    mountKanban();

    await vi.waitFor(() => {
      expect(listSpy).toHaveBeenCalledTimes(1);
    });
    expect(screen.queryByText('Agent-Created Note')).toBeNull();

    // Simulate the backend emit's single-entity read target: the note now
    // exists in the mock DB (as it would after a real create_note tool
    // call), but the board's local `tasks` array doesn't know about it yet.
    const now = new Date().toISOString();
    mockData.tasks.push({
      id: 'note-inserted-1',
      title: 'Agent-Created Note',
      content_preview: '',
      status: 'cabinet',
      priority: 'medium',
      due_date: null,
      project_id: PROJECT_ID,
      folder_id: null,
      note_type: null,
      tags: [],
      created_at: now,
      updated_at: now,
    } as any);

    emit('note-inserted-1', ENTITY_NOTE, REASON_CREATED);

    await vi.waitFor(() => {
      expect(screen.getByText('Agent-Created Note')).toBeTruthy();
    });

    // §34.5: insert via a single-entity read, never a second full-list
    // reload.
    expect(listSpy).toHaveBeenCalledTimes(1);
  });

  it('inserts a card the SAME way for an event on the APP channel (DOM CustomEvent), not just the backend Tauri channel (#2972 stage 3, item 4)', async () => {
    const listSpy = vi.spyOn(api, 'listNotesForKanban').mockResolvedValue([]);

    mountKanban();
    await vi.waitFor(() => {
      expect(listSpy).toHaveBeenCalledTimes(1);
    });
    expect(screen.queryByText('App-Channel Note')).toBeNull();

    const now = new Date().toISOString();
    mockData.tasks.push({
      id: 'note-app-channel-1',
      title: 'App-Channel Note',
      content_preview: '',
      status: 'cabinet',
      priority: 'medium',
      due_date: null,
      project_id: PROJECT_ID,
      folder_id: null,
      note_type: null,
      tags: [],
      created_at: now,
      updated_at: now,
    } as any);

    // The only difference from the test above: this fires via the app's
    // own DOM CustomEvent sender (what api.ts's migrated create_* wrappers
    // now call), not the simulated backend Tauri emit.
    emitViaAppChannel('note-app-channel-1', ENTITY_NOTE, REASON_CREATED);

    await vi.waitFor(() => {
      expect(screen.getByText('App-Channel Note')).toBeTruthy();
    });
  });

  it('does NOT insert a card for a note belonging to a different project', async () => {
    const listSpy = vi.spyOn(api, 'listNotesForKanban').mockResolvedValue([]);

    mountKanban();
    await vi.waitFor(() => {
      expect(listSpy).toHaveBeenCalledTimes(1);
    });

    const now = new Date().toISOString();
    mockData.tasks.push({
      id: 'note-other-project-1',
      title: 'Belongs Elsewhere',
      content_preview: '',
      status: 'cabinet',
      priority: 'medium',
      due_date: null,
      project_id: 'some-other-project',
      folder_id: null,
      note_type: null,
      tags: [],
      created_at: now,
      updated_at: now,
    } as any);

    emit('note-other-project-1', ENTITY_NOTE, REASON_CREATED);

    // Give the async handler a tick, then confirm nothing was inserted.
    await new Promise(r => setTimeout(r, 30));
    expect(screen.queryByText('Belongs Elsewhere')).toBeNull();
  });
});

describe('KanbanBoard pane-data-changed handler — remove branch (#2972, §34.5)', () => {
  it('removes a card when its note is reported deleted, without a listNotesForKanban() reload', async () => {
    const now = new Date().toISOString();
    mockData.tasks.push({
      id: 'note-to-delete-1',
      title: 'Doomed Note',
      content_preview: '',
      status: 'todo',
      priority: 'medium',
      due_date: null,
      project_id: PROJECT_ID,
      folder_id: null,
      note_type: null,
      tags: [],
      created_at: now,
      updated_at: now,
    } as any);
    const listSpy = vi.spyOn(api, 'listNotesForKanban').mockResolvedValue([
      { id: 'note-to-delete-1', title: 'Doomed Note', description: '', status: 'todo',
        priority: 'medium', project_id: PROJECT_ID, tags: [], sort_order: 0,
        created_at: now, updated_at: now } as any,
    ]);

    mountKanban();
    await vi.waitFor(() => {
      expect(screen.getByText('Doomed Note')).toBeTruthy();
    });

    emit('note-to-delete-1', ENTITY_SHEET, REASON_DELETED);

    await vi.waitFor(() => {
      expect(screen.queryByText('Doomed Note')).toBeNull();
    });
    expect(listSpy).toHaveBeenCalledTimes(1);
  });

  it('removes a card when the note moves out of this board\'s project', async () => {
    const now = new Date().toISOString();
    const listSpy = vi.spyOn(api, 'listNotesForKanban').mockResolvedValue([
      { id: 'note-moved-out-1', title: 'Moving Out', description: '', status: 'todo',
        priority: 'medium', project_id: PROJECT_ID, tags: [], sort_order: 0,
        created_at: now, updated_at: now } as any,
    ]);

    mountKanban();
    await vi.waitFor(() => {
      expect(screen.getByText('Moving Out')).toBeTruthy();
    });

    // The note's project_id has changed in the backend (moved to another
    // project) — the board's single-entity re-read sees it no longer
    // belongs here.
    mockData.tasks.push({
      id: 'note-moved-out-1',
      title: 'Moving Out',
      content_preview: '',
      status: 'todo',
      priority: 'medium',
      due_date: null,
      project_id: 'some-other-project',
      folder_id: null,
      note_type: null,
      tags: [],
      created_at: now,
      updated_at: now,
    } as any);

    emit('note-moved-out-1', ENTITY_NOTE, REASON_EDITED);

    await vi.waitFor(() => {
      expect(screen.queryByText('Moving Out')).toBeNull();
    });
  });
});
