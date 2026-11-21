/**
 * KanbanBoard.sheet-identity.test.ts — #2874 item 15, fix #1 (confirmed
 * broken site).
 *
 * `handleNoteSelect` (KanbanBoard.svelte, ~line 253) hand-builds the
 * `$activeNote.meta` object field-by-field instead of spreading the backend
 * meta. It was missing `note_type`, so opening an EXISTING sheet card from
 * a Kanban board silently dropped its sheet identity: `$activeNote.meta.note_type`
 * came back `undefined`, `Editor.svelte`'s `isSheet` derived state
 * (`noteMeta?.note_type === 'sheet'`) resolved false, and the sheet rendered
 * as a blank/garbled TipTap doc instead of mounting `<SheetGrid>`.
 *
 * These tests mount the REAL KanbanBoard.svelte (which mounts the REAL
 * Editor.svelte -> SheetGrid.svelte on sheet-note select), following the
 * wholesale-mock-`$lib/api`-only pattern established by
 * Editor.sheet-cell-editing-integration.test.ts and SheetGrid.test.ts —
 * `entityService` (the module `handleNoteSelect` actually calls through) is
 * mocked directly since that's the real module boundary this bug lived at.
 *
 * vitest-setup.ts's `get_project_notes_for_kanban` mock case was ALSO fixed
 * as part of this PR (it excluded any `note_type`-tagged row, diverging from
 * the real backend's `note_type IS NULL OR note_type = 'sheet'` predicate —
 * KANBAN_NOTES_QUERY, notes.rs) so sheet cards actually appear on the mocked
 * board in the first place.
 */

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, fireEvent, screen, within } from '@testing-library/svelte/svelte5';
import { get } from 'svelte/store';
import KanbanBoard from './KanbanBoard.svelte';
import { activeNote } from '$lib/stores/app';
import { canvasState } from '$lib/stores/canvas';
import { mockData } from '../../../vitest-setup';
import * as entityService from '$lib/services/entityService';
import * as api from '$lib/api';
import type { SheetData } from '$lib/types';

// vitest-setup.ts replaces `globalThis.window` with a plain mock object
// lacking addEventListener/removeEventListener/ResizeObserver — KanbanBoard
// registers window listeners and a ResizeObserver in onMount (same pattern
// documented in KanbanBoard.new-sheet-button-polish.test.ts).
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
// TipTap (mounted for non-sheet notes) needs window.DOMParser; the mock
// window object doesn't carry it even though jsdom's real global DOMParser
// is reachable — same fix as Editor.inline-mark-serialization.test.ts.
if (!win.DOMParser && typeof DOMParser !== 'undefined') {
  win.DOMParser = DOMParser;
}
// TipTap's Editor.mount() also calls window.setTimeout directly (not just
// globalThis.setTimeout) — the mock window object has neither.
if (typeof win.setTimeout !== 'function') {
  win.setTimeout = globalThis.setTimeout.bind(globalThis);
}
if (typeof win.clearTimeout !== 'function') {
  win.clearTimeout = globalThis.clearTimeout.bind(globalThis);
}
// ProseMirror's DOMSerializer needs a real `document` reachable off the
// window object (getHTML() -> serializeFragment() -> createDocumentFragment) —
// the mock window has none. jsdom's real global document is still reachable.
if (!win.document && typeof document !== 'undefined') {
  win.document = document;
}

const PROJECT_ID = 'project-sheet-identity';
const SHEET_NOTE_ID = 'note-sheet-1';

function seedSheetNote() {
  const now = new Date().toISOString();
  mockData.tasks.push({
    id: SHEET_NOTE_ID,
    title: 'My Sheet',
    content: '',
    content_preview: '',
    status: 'cabinet',
    priority: 'medium',
    due_date: null,
    assignee: null,
    project_id: PROJECT_ID,
    folder_id: null,
    parent_id: null,
    note_type: 'sheet',
    file_path: '',
    tags: [],
    pinned: false,
    word_count: 0,
    created_at: now,
    updated_at: now,
  } as any);
  mockData.noteContents[SHEET_NOTE_ID] = '';
}

beforeEach(() => {
  mockData.tasks = [];
  mockData.nextId = 1;
  mockData.noteContents = {};
  mockData.projects = [
    { id: PROJECT_ID, name: 'Sheet Identity Project', parent_id: null,
      created_at: new Date().toISOString(), updated_at: new Date().toISOString() },
  ];
  canvasState.set({
    view: 'kanban',
    data: {},
    context: { projectId: PROJECT_ID, projectName: 'Sheet Identity Project' },
  });
  activeNote.set(null);
});

// vitest-setup.ts's Tauri IPC mock has no `get_sheet_data` /
// `get_sheet_card_summaries` cases (sheets.rs commands are out of scope for
// that generic mock) — SheetGrid.svelte and KanbanBoard's loadSheetSummaries
// both call through api.ts wrappers around those two commands, so they're
// spied/stubbed directly at the api module boundary, same approach
// Editor.sheet-cell-editing-integration.test.ts uses for `getSheetData`.
function makeSheetData(noteId: string, title: string): SheetData {
  return {
    note_meta: { id: noteId, title, notebook: '', slug: noteId, created_at: '', updated_at: '', preview: '', note_type: 'sheet' } as any,
    columns: [{ id: 'col-a', note_id: noteId, name: 'Column 1', col_type: 'text', options_json: '{}', width: 148, sort_order: 0, created_at: '', updated_at: '' }],
    rows: [{ id: 'row-1', note_id: noteId, sort_order: 0, height: 34, created_at: '', updated_at: '' }],
    cells: [],
    formula_count: 0,
    formulas_paused: false,
  };
}

afterEach(() => {
  vi.restoreAllMocks();
  activeNote.set(null);
});

describe('KanbanBoard — sheet identity on open (item 15, fix #1)', () => {
  it('handleNoteSelect preserves note_type when opening an existing sheet card', async () => {
    seedSheetNote();
    render(KanbanBoard, { props: {} });

    const card = await screen.findByText('My Sheet');
    await fireEvent.click(card);

    await vi.waitFor(() => {
      const note = get(activeNote);
      expect(note).toBeTruthy();
      expect(note!.meta.id).toBe(SHEET_NOTE_ID);
    });

    const note = get(activeNote);
    // The regression: this field was silently dropped by the hand-built
    // meta object in handleNoteSelect. RED-ON-REVERT — deleting the
    // `note_type: noteData.meta.note_type,` line makes this assertion fail.
    expect(note!.meta.note_type).toBe('sheet');
  });

  it('mounts SheetGrid, not a TipTap ProseMirror editor, when an existing sheet card is opened', async () => {
    seedSheetNote();
    vi.spyOn(api, 'getSheetCardSummaries').mockResolvedValue([]);
    vi.spyOn(api, 'getSheetData').mockResolvedValue(makeSheetData(SHEET_NOTE_ID, 'My Sheet'));
    render(KanbanBoard, { props: {} });

    const card = await screen.findByText('My Sheet');
    await fireEvent.click(card);

    await vi.waitFor(() => {
      expect(screen.queryByTestId('sheet-grid')).toBeTruthy();
    });
    expect(document.querySelector('.ProseMirror')).toBeNull();
  });

  it('entityService.getNote is the real data source handleNoteSelect reads note_type from', async () => {
    seedSheetNote();
    const getNoteSpy = vi.spyOn(entityService, 'getNote');
    render(KanbanBoard, { props: {} });

    const card = await screen.findByText('My Sheet');
    await fireEvent.click(card);

    await vi.waitFor(() => {
      expect(getNoteSpy).toHaveBeenCalledWith(SHEET_NOTE_ID);
    });
    const result = await getNoteSpy.mock.results[0].value;
    expect(result.meta.note_type).toBe('sheet');
  });

  it('opening a PLAIN (non-sheet) note leaves note_type undefined and mounts no SheetGrid', async () => {
    const now = new Date().toISOString();
    mockData.tasks.push({
      id: 'note-plain-1',
      title: 'My Plain Note',
      content: 'hello world',
      content_preview: 'hello world',
      status: 'cabinet',
      priority: 'medium',
      due_date: null,
      assignee: null,
      project_id: PROJECT_ID,
      folder_id: null,
      parent_id: null,
      note_type: null,
      tags: [],
      pinned: false,
      word_count: 2,
      created_at: now,
      updated_at: now,
    } as any);
    mockData.noteContents['note-plain-1'] = 'hello world';

    render(KanbanBoard, { props: {} });

    const card = await screen.findByText('My Plain Note');
    await fireEvent.click(card);

    await vi.waitFor(() => {
      const note = get(activeNote);
      expect(note).toBeTruthy();
      expect(note!.meta.id).toBe('note-plain-1');
    });

    const note = get(activeNote);
    expect(note!.meta.note_type == null).toBe(true);
    expect(screen.queryByTestId('sheet-grid')).toBeNull();
  });
});
