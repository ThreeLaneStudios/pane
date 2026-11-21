/**
 * KanbanBoard.mount-clears-activeNote.test.ts — #3044 delta-2 (Reed).
 *
 * `KanbanBoard.svelte`'s `onMount` used to run an unconditional
 * `$activeNote = null; // Clear active note on kanban mount` as its first
 * statement — the same protection `NotesBrowser.svelte`'s onMount has
 * (dropping a note carried over from whatever view was active before
 * Kanban mounted), and the same bug: it ALSO wiped a note THIS SAME mount
 * was deliberately opening (Calendar opening a board card; a wiki-link
 * click from Kanban's own editor panel navigating elsewhere and back —
 * see `CanvasRenderer.wikilink-unmounted-note-race.test.ts`).
 *
 * Both onMounts now call the one shared `noteService.
 * clearStaleActiveNoteOnMount()` — see its own doc comment in
 * `noteService.ts` and `NotesBrowser.mount-clears-activeNote.test.ts`
 * (NotesBrowser's equivalent, pre-existing, coverage for the stale-clear
 * side of this).
 *
 * This file is KanbanBoard's own version of that same matrix: a genuinely
 * stale carry-over is still cleared (the original protection), AND a
 * deliberately-opened note (context.noteId already matching) survives
 * (the fix) — both via a real `render(KanbanBoard, ...)`, not a
 * source-regex.
 */

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, cleanup } from '@testing-library/svelte/svelte5';
import { get } from 'svelte/store';
import KanbanBoard from './KanbanBoard.svelte';
import * as api from '$lib/api';
import { activeNote } from '$lib/stores/app';
import { canvasState } from '$lib/stores/canvas';

const win = globalThis.window as any;
if (typeof win.addEventListener !== 'function') {
  win.addEventListener = globalThis.addEventListener?.bind(globalThis) ?? (() => {});
}
if (typeof win.removeEventListener !== 'function') {
  win.removeEventListener = globalThis.removeEventListener?.bind(globalThis) ?? (() => {});
}
if (typeof win.dispatchEvent !== 'function') {
  win.dispatchEvent = globalThis.dispatchEvent?.bind(globalThis) ?? (() => true);
}
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
if (typeof (globalThis as any).Range !== 'undefined' && !(globalThis as any).Range.prototype.getClientRects) {
  (globalThis as any).Range.prototype.getClientRects = function () { return []; };
  (globalThis as any).Range.prototype.getBoundingClientRect = function () {
    return { top: 0, bottom: 0, left: 0, right: 0, width: 0, height: 0, x: 0, y: 0, toJSON() { return this; } };
  };
}
if (typeof Element !== 'undefined' && !Element.prototype.getClientRects) {
  Element.prototype.getClientRects = function () { return [] as any; };
}

vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(() => Promise.resolve(() => {})),
}));

const PROJECT_ID = 'project-kanban-mount-clear';

function makeNote(id: string, overrides: Record<string, any> = {}) {
  return {
    meta: {
      id,
      title: 'Carried-Over Note',
      slug: id,
      notebook: 'Some Other Project',
      preview: '',
      created_at: '',
      updated_at: '',
      note_type: undefined,
      ...overrides,
    },
    content: '',
    jsonContent: null,
  };
}

beforeEach(() => {
  vi.spyOn(api, 'listNotesForKanban').mockResolvedValue([]);
  vi.spyOn(api, 'listFolders').mockResolvedValue([]);
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  activeNote.set(null);
  canvasState.set({ view: 'home', data: {}, context: {} } as any);
});

describe('KanbanBoard — clears a genuinely stale $activeNote carry-over on mount', () => {
  it('clears $activeNote on mount when it does not match canvasState.context.noteId (no target at all)', () => {
    activeNote.set(makeNote('stale-note-1') as any);
    canvasState.set({ view: 'kanban', data: {}, context: { projectId: PROJECT_ID } } as any);

    render(KanbanBoard, { props: {} });

    // RED-ON-REVERT: removing the onMount guard entirely (or hardcoding it
    // to never clear) leaves the stale note in place here.
    expect(get(activeNote)).toBeNull();
  });

  it('clears $activeNote on mount when context.noteId points at a DIFFERENT note', () => {
    activeNote.set(makeNote('stale-note-1') as any);
    canvasState.set({
      view: 'kanban',
      data: {},
      context: { projectId: PROJECT_ID, noteId: 'some-other-note' },
    } as any);

    render(KanbanBoard, { props: {} });

    expect(get(activeNote)).toBeNull();
  });
});

describe('KanbanBoard — keeps a deliberately-opened note on mount (#3044 delta-2 fix)', () => {
  it('does NOT clear $activeNote on mount when it already matches canvasState.context.noteId', () => {
    activeNote.set(makeNote('deliberately-opened-note', { title: 'Opened By This Navigation' }) as any);
    canvasState.set({
      view: 'kanban',
      data: {},
      context: { projectId: PROJECT_ID, noteId: 'deliberately-opened-note' },
    } as any);

    render(KanbanBoard, { props: {} });

    // RED pre-#3044-delta-2: the old unconditional
    // `$activeNote = null; // Clear active note on kanban mount` wiped
    // this regardless of context.noteId.
    expect(get(activeNote)?.meta?.id).toBe('deliberately-opened-note');
  });
});
