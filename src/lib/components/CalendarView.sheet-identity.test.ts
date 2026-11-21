/**
 * CalendarView.sheet-identity.test.ts — #2874 item 15 / checklist #22
 * regression coverage.
 *
 * The click-through chain from a calendar task card to the opened note spans
 * THREE components via a window CustomEvent, not a direct function call:
 *
 *   CalendarView.svelte (task card onclick -> onTaskSelect prop)
 *     -> CanvasRenderer.svelte's handleTaskClick (navigates to notes_browser,
 *        then `window.dispatchEvent(new CustomEvent('pane-select-note', ...))`
 *        after a 200ms delay for the view to render)
 *     -> NotesBrowser.svelte's `pane-select-note` window listener
 *        (NotesBrowser.svelte ~line 673) -> selectProjectNote(note)
 *        -> noteService.selectNote(note)
 *
 * Per the item-15 scope audit (§1 inventory, row 7), this chain was traced
 * and found ALREADY SAFE: no hand-built meta object exists anywhere in it —
 * `noteService.selectNote` calls `api.readNote()` (itself a spread) and then
 * spreads `{ ...noteWithContent.meta, ... }` when enriching with task
 * fields. This file is REGRESSION insurance for that finding (and doubles as
 * the requested evidence Matt can use to re-litigate checklist item 22 if
 * needed) — it exercises the REAL `NotesBrowser` window listener and the
 * REAL `noteService.selectNote`, with only `$lib/api` mocked at the
 * boundary, dispatching the actual `pane-select-note` CustomEvent the same
 * way CanvasRenderer's `handleTaskClick` does.
 */

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, cleanup, screen } from '@testing-library/svelte/svelte5';
import { get } from 'svelte/store';
import NotesBrowser from './NotesBrowser.svelte';
import * as api from '$lib/api';
import { activeProject, projects, activeNote } from '$lib/stores/app';
import type { NoteMeta } from '$lib/types';

const win = globalThis.window as any;
if (typeof win.addEventListener !== 'function') {
  win.addEventListener = globalThis.addEventListener?.bind(globalThis) ?? (() => {});
}
if (typeof win.removeEventListener !== 'function') {
  win.removeEventListener = globalThis.removeEventListener?.bind(globalThis) ?? (() => {});
}
// vitest-setup.ts's `window` mock stubs `dispatchEvent` as a hardcoded no-op
// (`return true` without actually firing anything) — see
// ChatSidebar.onboardingAgentName.test.ts's own note on this. Since
// `window.addEventListener` above is aliased to the REAL
// `globalThis.addEventListener`, listeners registered via
// `window.addEventListener` (as NotesBrowser's `pane-select-note` effect
// does) are only reachable by dispatching through `globalThis.dispatchEvent`,
// not `window.dispatchEvent`.
if (typeof (globalThis as any).ResizeObserver !== 'function') {
  (globalThis as any).ResizeObserver = class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
}

vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(() => Promise.resolve(() => {})),
}));

const PROJECT_ID = 'project-calendar-1';
const SHEET_NOTE_ID = 'calendar-sheet-note-1';

function makeSheetNoteMeta(): NoteMeta {
  return {
    id: SHEET_NOTE_ID,
    title: 'Calendar-linked Sheet',
    slug: SHEET_NOTE_ID,
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
    note_type: 'sheet',
  } as any;
}

beforeEach(() => {
  activeProject.set({
    id: PROJECT_ID, name: 'Project One', parent_id: null,
    created_at: '', updated_at: '',
  } as any);
  projects.set([
    { id: PROJECT_ID, name: 'Project One', parent_id: null, created_at: '', updated_at: '' } as any,
  ]);
  activeNote.set(null);

  vi.spyOn(api, 'listProjects').mockResolvedValue([
    { id: PROJECT_ID, name: 'Project One', parent_id: null, created_at: '', updated_at: '' } as any,
  ]);
  vi.spyOn(api, 'listFolders').mockResolvedValue([]);
  vi.spyOn(api, 'listNotesByProject').mockResolvedValue([makeSheetNoteMeta()]);
  vi.spyOn(api, 'getSheetCardSummaries').mockResolvedValue([]);
  // noteService.selectNote's real path: readNote (spread-based, api.ts:79-115)
  // followed by getNoteTaskFields enrichment (also spread-based).
  vi.spyOn(api, 'readNote').mockResolvedValue({
    meta: { ...makeSheetNoteMeta() },
    content: '',
  } as any);
  vi.spyOn(api, 'getNoteTaskFields').mockResolvedValue({
    title: null, task_status: null, task_priority: null, task_due_date: null, task_assignee: null,
  } as any);
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  activeProject.set(null);
  projects.set([]);
  activeNote.set(null);
});

describe('CalendarView -> CanvasRenderer -> NotesBrowser click-through preserves sheet identity (item 15 / checklist #22)', () => {
  it('dispatching pane-select-note (the exact event CanvasRenderer.handleTaskClick fires) resolves $activeNote with note_type still "sheet"', async () => {
    render(NotesBrowser, { props: {} });

    // Let taskNotes populate (loadTaskNotes -> api.listNotesByProject) before
    // the listener's `taskNotes.length > 0` guard can find the note.
    await vi.waitFor(() => {
      expect(api.listNotesByProject).toHaveBeenCalled();
    });
    // Let the $effect that registers the `pane-select-note` listener settle
    // (registered inside an `$effect`, which runs on a microtask/tick after
    // the taskNotes state update above).
    await new Promise((r) => setTimeout(r, 30));

    // The exact CustomEvent CanvasRenderer.svelte:381 dispatches.
    globalThis.dispatchEvent(new CustomEvent('pane-select-note', { detail: { noteId: SHEET_NOTE_ID } }));

    await vi.waitFor(() => {
      const note = get(activeNote);
      expect(note).toBeTruthy();
      expect(note!.meta.id).toBe(SHEET_NOTE_ID);
    });

    const note = get(activeNote);
    expect(note!.meta.note_type).toBe('sheet');
  });

  it('mounts SheetGrid, not a TipTap editor, when a sheet task is opened via calendar click-through', async () => {
    vi.spyOn(api, 'getSheetData').mockResolvedValue({
      note_meta: makeSheetNoteMeta() as any,
      columns: [{ id: 'col-a', note_id: SHEET_NOTE_ID, name: 'Col', col_type: 'text', options_json: '{}', width: 148, sort_order: 0, created_at: '', updated_at: '' }],
      rows: [{ id: 'row-1', note_id: SHEET_NOTE_ID, sort_order: 0, height: 34, created_at: '', updated_at: '' }],
      cells: [],
      formula_count: 0,
      formulas_paused: false,
    } as any);

    render(NotesBrowser, { props: {} });

    await vi.waitFor(() => {
      expect(api.listNotesByProject).toHaveBeenCalled();
    });

    await new Promise((r) => setTimeout(r, 30));
    globalThis.dispatchEvent(new CustomEvent('pane-select-note', { detail: { noteId: SHEET_NOTE_ID } }));

    await vi.waitFor(() => {
      expect(screen.queryByTestId('sheet-grid')).toBeTruthy();
    });
    expect(document.querySelector('.ProseMirror')).toBeNull();
  });
});
