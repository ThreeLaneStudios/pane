/**
 * KanbanBoard.cmdN-preexisting.test.ts — #3044, revisited (screen test
 * matrix: "Cmd+N from the board").
 *
 * Scope note, read this first: Cmd+N while viewing the Kanban board
 * ("the board" — CHANGELOG.md's own term for `KanbanBoard.svelte`, e.g.
 * item F: "clicking a task from the Calendar opens that project's
 * board (`view: 'kanban'`)") is NOT part of #3044's fix. It never was.
 * `KanbanBoard.svelte` has registered its OWN `window` keydown listener
 * for Cmd+N since #424 (`fix(kanban): Cmd+N shortcut`, long before
 * #3044), calling its own `handleCreateNewTask` — a materially different
 * flow (always status `cabinet`, backend auto-assigns the folder, opens
 * inline in the board's own editor panel rather than navigating anywhere)
 * that creates a task card, not a plain note via the New Note button's
 * path. `+page.svelte`'s own Cmd+N handler explicitly returns early when
 * `$canvasState.view === 'kanban'`, both before and after #3044's rework —
 * untouched by this change, confirmed by this PR's diff touching zero
 * lines in that early-return.
 *
 * This test exists ONLY because Matt's screen-test list for this ruling
 * names "the board" as one of three screens to check Cmd+N on, alongside
 * "the notes browser" and "Home". Per the task's own instruction ("Each
 * shown failing before the change where applicable; if one passes at
 * base, say so honestly") — this one PASSES AT BASE. It is not a fix,
 * it's a regression guard proving #3044's rework of the New Note
 * button/Cmd+N path did not accidentally touch or break Kanban's own,
 * separate, pre-existing Cmd+N flow (this PR's diff contains zero changes
 * to KanbanBoard.svelte's keydown handling).
 *
 * Harness: mirrors KanbanBoard.editor-survives-reload.test.ts's house
 * pattern — real KanbanBoard.svelte + real Editor.svelte, backed by
 * vitest-setup.ts's mockData (which implements a real create_note ->
 * update_note -> get_note round trip), rather than NotesBrowser's test's
 * manual per-method api spies.
 */

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, cleanup, fireEvent, screen, waitFor } from '@testing-library/svelte/svelte5';
import { get } from 'svelte/store';
import KanbanBoard from './KanbanBoard.svelte';
import { activeNote } from '$lib/stores/app';
import { canvasState } from '$lib/stores/canvas';
import { mockData } from '../../../vitest-setup';
import * as entityService from '$lib/services/entityService';

// See KanbanBoard.sheet-identity.test.ts / KanbanBoard.editor-survives-reload.test.ts
// for the full rationale on each of these shims.
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
win.addEventListener = globalThis.addEventListener.bind(globalThis);
win.removeEventListener = globalThis.removeEventListener.bind(globalThis);
win.dispatchEvent = globalThis.dispatchEvent.bind(globalThis);

if (typeof (globalThis as any).Range !== 'undefined' && !(globalThis as any).Range.prototype.getClientRects) {
  (globalThis as any).Range.prototype.getClientRects = function () { return []; };
  (globalThis as any).Range.prototype.getBoundingClientRect = function () {
    return { top: 0, bottom: 0, left: 0, right: 0, width: 0, height: 0, x: 0, y: 0, toJSON() { return this; } };
  };
}
if (typeof Element !== 'undefined' && !Element.prototype.getClientRects) {
  Element.prototype.getClientRects = function () { return [] as any; };
}

async function advanceAndFlush(totalMs: number, stepMs = 50) {
  let waited = 0;
  while (waited < totalMs) {
    await new Promise((resolve) => setTimeout(resolve, stepMs));
    await new Promise((resolve) => setTimeout(resolve, 0));
    waited += stepMs;
  }
}

const PROJECT_ID = 'project-kanban-cmdn';

beforeEach(() => {
  mockData.tasks = [];
  mockData.nextId = 1;
  mockData.noteContents = {};
  mockData.projects = [
    { id: PROJECT_ID, name: 'Kanban Cmd+N Project', parent_id: null,
      created_at: new Date().toISOString(), updated_at: new Date().toISOString() },
  ];
  canvasState.set({
    view: 'kanban',
    data: {},
    context: { projectId: PROJECT_ID, projectName: 'Kanban Cmd+N Project' },
  } as any);
  activeNote.set(null);
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  activeNote.set(null);
});

describe('Kanban board\'s own, pre-existing Cmd+N (#424) — unaffected by #3044, passes at base', () => {
  it('a real Cmd+N keydown on the board creates a task card and opens it (no $activeProject dependency — Kanban reads canvasState.context, not $activeProject, by design)', async () => {
    render(KanbanBoard, { props: {} });
    await new Promise((r) => setTimeout(r, 0));

    expect(mockData.tasks.length).toBe(0);

    window.dispatchEvent(new KeyboardEvent('keydown', { key: 'n', metaKey: true, bubbles: true }));
    await new Promise((r) => setTimeout(r, 0));

    await waitFor(() => {
      expect(mockData.tasks.length).toBe(1);
    });
    expect(mockData.tasks[0].project_id).toBe(PROJECT_ID);
    expect(mockData.tasks[0].status).toBe('cabinet');

    await waitFor(() => {
      const note = get(activeNote);
      expect(note?.meta?.id).toBe(mockData.tasks[0].id);
    });
  });

  it('typing a title and body into the Cmd+N-created card actually saves — title and body survive a reopen', async () => {
    render(KanbanBoard, { props: {} });
    await new Promise((r) => setTimeout(r, 0));

    window.dispatchEvent(new KeyboardEvent('keydown', { key: 'n', metaKey: true, bubbles: true }));
    await waitFor(() => {
      expect(mockData.tasks.length).toBe(1);
    });
    const noteId = mockData.tasks[0].id;

    const titleInput = await screen.findByTestId('note-title-input') as HTMLInputElement;
    await fireEvent.input(titleInput, { target: { value: 'Board Cmd+N Note' } });
    await fireEvent.blur(titleInput);
    await new Promise((r) => setTimeout(r, 0));

    await waitFor(() => {
      expect(mockData.tasks.find((t: any) => t.id === noteId)?.title).toBe('Board Cmd+N Note');
    });

    const pmEl = document.querySelector('.ProseMirror') as HTMLElement;
    expect(pmEl).not.toBeNull();
    pmEl.focus();
    const clipboardData = {
      getData: (fmt: string) => (fmt === 'text/plain' ? 'Hello from the board Cmd+N body' : ''),
      types: ['text/plain'],
      files: [],
    };
    await fireEvent.paste(pmEl, { clipboardData });
    await advanceAndFlush(700);

    await waitFor(() => {
      expect(mockData.noteContents[noteId]).toContain('Hello from the board Cmd+N body');
    });

    // ── Reopen ──────────────────────────────────────────────────────────
    activeNote.set(null);
    await new Promise((r) => setTimeout(r, 0));
    const reopened = await entityService.getNote(noteId);
    expect(reopened.meta.title).toBe('Board Cmd+N Note');
    expect(reopened.content).toContain('Hello from the board Cmd+N body');
  });
});
