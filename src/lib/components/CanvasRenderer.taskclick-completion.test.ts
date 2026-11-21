/**
 * CanvasRenderer.taskclick-completion.test.ts — #2874 item 18 fix 1
 * regression coverage, UPDATED for #2874 item F (this PR).
 *
 * Before item F: `handleTaskClick` navigated to `view: 'notes_browser'`
 * (the full per-project note/file browser — Matt's "every note" complaint)
 * and this file tested the completion-signal fix against that destination
 * (NotesBrowser's `pendingSelectNoteId` queue).
 *
 * Item F changes the destination: clicking a Calendar task now opens that
 * project's **board** (`view: 'kanban'`, same view `Sidebar.svelte`'s
 * `selectProject` uses) with the task's **card open** — not the notes
 * browser. The underlying completion-signal mechanism this file was
 * written to guard (await navigateTo() -> tick() -> dispatch
 * `pane-select-note`, no fixed-delay setTimeout) is unchanged in shape; it
 * now resolves against `KanbanBoard.svelte`'s own `pendingSelectTaskId`
 * queue instead of `NotesBrowser.svelte`'s `pendingSelectNoteId` — same
 * pattern, new listener, no new dispatch path (KanbanBoard reuses the
 * existing `pane-select-note` CustomEvent `NotesBrowser`/`ChatSidebar`
 * already use).
 *
 * `api.listNotesForKanban` (the call KanbanBoard's `loadTasks()` makes) is
 * deliberately delayed 250ms so this test FAILS if the completion-signal
 * queue regresses to a fixed-delay guess: the click must not open the card
 * before the slow load resolves, and must open it once it does.
 */

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { readFileSync } from 'fs';
import { resolve } from 'path';
// Must be the FIRST non-type import in this file: aliases
// window.addEventListener/removeEventListener/dispatchEvent to the real
// globals before CanvasRenderer's transitive import chain
// (CanvasRenderer -> Settings -> authStore/syncStore) calls
// window.addEventListener at MODULE LOAD TIME. ES module semantics
// evaluate all `import` declarations (hoisted, in source order) before any
// of a module's own top-level non-import code runs — a plain top-level
// statement placed textually above an `import` line would still run too
// late. Importing the shim here, as literally the first import, guarantees
// its side effects complete before `import CanvasRenderer from
// './CanvasRenderer.svelte'` below begins evaluating its own transitive
// chain. See _window-event-shim.ts for the full rationale.
import './_window-event-shim';
import { render, cleanup, screen, fireEvent } from '@testing-library/svelte/svelte5';
import { get } from 'svelte/store';
import CanvasRenderer from './CanvasRenderer.svelte';
import * as api from '$lib/api';
import * as entityService from '$lib/services/entityService';
import { canvasState } from '$lib/stores/canvas';
import { activeProject, projects, activeNote } from '$lib/stores/app';
import type { NoteMeta, Task, Project } from '$lib/types';

// The shim import above covers add/removeEventListener/dispatchEvent.
// TipTap (mounted once the card opens, via KanbanBoard.svelte -> Editor.svelte)
// additionally needs ResizeObserver/DOMParser/setTimeout/clearTimeout/document
// off the window object — same shim set CanvasRenderer.taskclick-completion
// (pre-item-F) and KanbanBoard.sheet-identity.test.ts already use.
const win = globalThis.window as any;
if (typeof (globalThis as any).ResizeObserver !== 'function') {
  (globalThis as any).ResizeObserver = class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
}
if (!win.DOMParser && typeof DOMParser !== 'undefined') {
  win.DOMParser = DOMParser;
}
if (typeof win.setTimeout !== 'function') {
  win.setTimeout = globalThis.setTimeout.bind(globalThis);
}
if (typeof win.clearTimeout !== 'function') {
  win.clearTimeout = globalThis.clearTimeout.bind(globalThis);
}
if (!win.document && typeof document !== 'undefined') {
  win.document = document;
}

vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(() => Promise.resolve(() => {})),
}));

const PROJECT_ID = 'project-taskclick-1';
const TASK_NOTE_ID = 'task-note-slow-1';
const SLOW_LOAD_MS = 250; // deliberately > the old 200ms setTimeout

function makeProject(): Project {
  return {
    id: PROJECT_ID,
    name: 'Project One',
    parent_id: null,
    created_at: '',
    updated_at: '',
  } as any;
}

function makeKanbanTask(): Task {
  return {
    id: TASK_NOTE_ID,
    title: 'Slow-Load Task Note',
    status: 'todo',
    priority: 'medium',
    due_date: null,
    project_id: PROJECT_ID,
    folder_id: null,
    tags: [],
    sort_order: 0,
    created_at: '',
    updated_at: '',
  } as any;
}

function makeCalendarTask(): Task {
  const now = new Date();
  const dueDate = `${now.getFullYear()}-${String(now.getMonth() + 1).padStart(2, '0')}-15`;
  return {
    id: TASK_NOTE_ID,
    title: 'Slow-Load Task Note',
    status: 'todo',
    priority: 'medium',
    due_date: dueDate,
    project_id: PROJECT_ID,
    tags: [],
    sort_order: 0,
    created_at: '',
    updated_at: '',
  } as any;
}

function makeTaskNoteMeta(): NoteMeta {
  return {
    id: TASK_NOTE_ID,
    title: 'Slow-Load Task Note',
    slug: TASK_NOTE_ID,
    notebook: 'Project One',
    preview: '',
    content_preview: '',
    created_at: '2026-01-01T00:00:00Z',
    updated_at: '2026-01-01T00:00:00Z',
    pinned: false,
    word_count: 0,
    tags: [],
    project_id: PROJECT_ID,
    folder_id: undefined,
    note_type: 'task',
  } as any;
}

beforeEach(() => {
  canvasState.set({ view: 'calendar', data: {}, context: {} } as any);
  activeProject.set(null);
  projects.set([makeProject()]);
  activeNote.set(null);

  vi.spyOn(api, 'listProjects').mockResolvedValue([makeProject()]);
  vi.spyOn(api, 'listTasks').mockResolvedValue([makeCalendarTask()]);
  vi.spyOn(api, 'listFolders').mockResolvedValue([]);
  vi.spyOn(api, 'getSheetCardSummaries').mockResolvedValue([]);
  vi.spyOn(api, 'getActiveSession').mockResolvedValue({ id: 'sess-taskclick' } as any);
  vi.spyOn(api, 'getChatMessages').mockResolvedValue([]);

  // The deliberately-slow boundary call: KanbanBoard's loadTasks() awaits
  // this. 250ms > the old 200ms setTimeout window that used to gate the
  // pane-select-note dispatch.
  vi.spyOn(api, 'listNotesForKanban').mockImplementation(
    () => new Promise((resolve) => setTimeout(() => resolve([makeKanbanTask()]), SLOW_LOAD_MS))
  );

  vi.spyOn(api, 'readNote').mockResolvedValue({
    meta: { ...makeTaskNoteMeta() },
    content: '',
  } as any);
  vi.spyOn(api, 'getNoteTaskFields').mockResolvedValue({
    title: null, task_status: null, task_priority: null, task_due_date: null, task_assignee: null,
  } as any);
  // KanbanBoard's handleNoteSelect (the real card-open code path this test
  // now exercises post-item-F) reads through entityService.getNote, not
  // api.readNote (that was NotesBrowser's boundary, pre-item-F).
  vi.spyOn(entityService, 'getNote').mockResolvedValue({
    meta: { ...makeTaskNoteMeta() },
    content: '',
  } as any);
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  canvasState.set({ view: 'home', data: {}, context: {} } as any);
  activeProject.set(null);
  projects.set([]);
  activeNote.set(null);
});

describe('CanvasRenderer.handleTaskClick (#2874 item F): opens the board with the card', () => {
  it('navigates to the project board (view: kanban), not notes_browser', async () => {
    render(CanvasRenderer, { props: {} });

    const taskItem = await screen.findByText('Slow-Load Task Note');
    await fireEvent.click(taskItem);

    await vi.waitFor(() => {
      const state = get(canvasState);
      expect(state.view).toBe('kanban');
      expect(state.context.projectId).toBe(PROJECT_ID);
    });
  });

  it('does not open the card prematurely, then opens it once the slow load resolves (completion-signal, not a fixed delay)', async () => {
    render(CanvasRenderer, { props: {} });

    const taskItem = await screen.findByText('Slow-Load Task Note');
    await fireEvent.click(taskItem);

    // Immediately after the click (well before the 250ms mock resolves),
    // no note should be selected yet — there is no fixed-delay timer to have
    // fired early, and the async chain (navigateTo -> mount -> loadTasks)
    // has not completed.
    expect(get(activeNote)).toBeNull();

    // Sanity: the slow mock really was invoked (proves the click drove
    // navigation into kanban, which mounted KanbanBoard and triggered
    // loadTasks -> api.listNotesForKanban).
    await vi.waitFor(() => {
      expect(api.listNotesForKanban).toHaveBeenCalled();
    });

    // Still nothing selected while the 250ms mock is in flight.
    expect(get(activeNote)).toBeNull();

    // Once the slow mock resolves and KanbanBoard's pending-selection queue
    // (or, if the event arrived post-mount-but-pre-load, the tasks watcher)
    // resolves the request, the correct task's card opens.
    await vi.waitFor(
      () => {
        const note = get(activeNote);
        expect(note).toBeTruthy();
        expect(note!.meta.id).toBe(TASK_NOTE_ID);
      },
      { timeout: SLOW_LOAD_MS + 2000 }
    );
  });

  it('no longer relies on a setTimeout to gate the pane-select-note dispatch', () => {
    // Static guard alongside the behavioral assertion above: the fix's
    // entire premise is that handleTaskClick's dispatch is not gated behind
    // a fixed-delay timer.
    const COMPONENT_PATH = resolve(__dirname, 'CanvasRenderer.svelte');
    const src = readFileSync(COMPONENT_PATH, 'utf-8');
    const fnStart = src.indexOf('async function handleTaskClick');
    expect(fnStart).toBeGreaterThan(-1);
    const fnEnd = src.indexOf('\n  }\n', fnStart);
    const fnBody = src.slice(fnStart, fnEnd === -1 ? undefined : fnEnd);
    expect(fnBody).not.toMatch(/setTimeout/);
    // #2874 item F: the destination view must be 'kanban', not
    // 'notes_browser' — a direct guard against reverting the one-line
    // navigation target change.
    expect(fnBody).toMatch(/view:\s*'kanban'/);
    expect(fnBody).not.toMatch(/view:\s*'notes_browser'/);
  });
});
