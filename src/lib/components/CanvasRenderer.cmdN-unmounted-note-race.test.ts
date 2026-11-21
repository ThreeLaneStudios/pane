/**
 * CanvasRenderer.cmdN-unmounted-note-race.test.ts — #3044 delta-2 (Reed).
 *
 * Reed's delta-2 audit (`docs/audits/2026-10-18-pr-3044-delta2.md` item 1,
 * hard FAIL) proved by execution that `noteService.createNewNote()` being
 * store-only does NOT make Cmd+N screen-independent end-to-end: its "open"
 * step writes `canvasState.view = 'note_editor'`, which — when fired from
 * a screen where `NotesBrowser.svelte` wasn't already mounted (Calendar,
 * All Tasks, Trash, Home, the project list, project_config) — makes
 * `CanvasRenderer.svelte` mount `NotesBrowser` for the FIRST time.
 * `NotesBrowser`'s own `onMount` ran `$activeNote = null;` unconditionally,
 * with no read of `canvasState.context.noteId` to recover — wiping the
 * note `createNewNote()` had just set, a beat earlier, in the same call.
 *
 * Matt's ruling: keep `$activeNote` on mount when it already is the note
 * THIS navigation targeted (`canvasState.context.noteId`); clear only when
 * stale. One shared check — `noteService.clearStaleActiveNoteOnMount()` —
 * used by both `NotesBrowser.svelte`'s and `KanbanBoard.svelte`'s onMount.
 *
 * This file proves it end-to-end, with the target screen GENUINELY
 * unmounted — not the shortcut ("mount NotesBrowser first, then call the
 * function") that let three fixes in a row ship broken. `CanvasRenderer`
 * is rendered for real, starting on `calendar` (where `NotesBrowser` has
 * never been mounted); `noteService.createNewNote()` is called exactly as
 * `+page.svelte`'s Cmd+N keydown handler calls it; `CanvasRenderer`'s own
 * `{:else if}` branch chain is what actually swaps `NotesBrowser` into
 * existence in response — nothing in this test manually renders
 * `NotesBrowser` itself, or sequences anything by hand.
 */

import './_window-event-shim';
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, cleanup, screen, fireEvent, waitFor } from '@testing-library/svelte/svelte5';
import { tick } from 'svelte';
import { get } from 'svelte/store';
import CanvasRenderer from './CanvasRenderer.svelte';
import * as noteService from '$lib/services/noteService';
import * as api from '$lib/api';
import { canvasState } from '$lib/stores/canvas';
import { activeProject, projects, activeNote } from '$lib/stores/app';
import type { Project, NoteMeta } from '$lib/types';

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

async function flushAsync() {
  await Promise.resolve();
  await tick();
  await tick();
}

const PROJECT: Project = {
  id: 'proj-cmdn-race',
  name: 'Symbiotic',
  parent_id: null,
  created_at: '',
  updated_at: '',
  default_folder_id: 'folder-cmdn-race',
} as any;

const NEW_NOTE_ID = 'cmdn-race-new-note-1';

function makeCreatedMeta(): NoteMeta {
  return {
    id: NEW_NOTE_ID,
    title: 'Untitled',
    slug: NEW_NOTE_ID,
    notebook: 'Symbiotic/General',
    preview: '',
    content_preview: '',
    created_at: '2026-01-01T00:00:00Z',
    updated_at: '2026-01-01T00:00:00Z',
    pinned: false,
    word_count: 0,
    tags: [],
    project_id: PROJECT.id,
    folder_id: 'folder-cmdn-race',
    file_path: undefined,
    note_type: undefined,
  } as any;
}

let savedTitle = 'Untitled';
let savedContent = '';

// CanvasRenderer.svelte's OWN #2335 reload-guard $effect notices
// `canvasState.context.noteId` no longer matches `$activeNote` (true the
// instant NotesBrowser's pre-fix onMount wipes it) and re-fetches the note
// via `api.readNote` as an UNRELATED recovery path — eventually masking
// this exact race in a real render, at the cost of a redundant reload (and,
// in the real app, discarding anything typed in the gap). That recovery is
// real but not this fix, and its timing is not deterministic enough to
// assert against either way. Hanging `api.readNote` (never resolving)
// while `readNoteHang` is true makes the critical assertions in this file
// test ONLY the mount-time behavior `clearStaleActiveNoteOnMount` governs —
// released (`readNoteHang = false`) before the reopen steps below, which
// legitimately need it to resolve.
let readNoteHang = true;

beforeEach(() => {
  savedTitle = 'Untitled';
  savedContent = '';
  readNoteHang = true;

  // Start on Calendar — NotesBrowser has never been mounted this session.
  canvasState.set({ view: 'calendar', data: {}, context: {} } as any);
  activeProject.set(PROJECT); // Cmd+N requires an active project to create in
  projects.set([PROJECT]);
  activeNote.set(null);

  vi.spyOn(api, 'listProjects').mockResolvedValue([PROJECT]);
  vi.spyOn(api, 'listTasks').mockResolvedValue([]);
  vi.spyOn(api, 'listFolders').mockResolvedValue([{ id: 'folder-cmdn-race', name: 'General', parent_id: PROJECT.id } as any]);
  vi.spyOn(api, 'listNotesByProject').mockResolvedValue([]);
  vi.spyOn(api, 'getSheetCardSummaries').mockResolvedValue([]);
  vi.spyOn(api, 'getActiveSession').mockResolvedValue({ id: 'sess-cmdn-race' } as any);
  vi.spyOn(api, 'getChatMessages').mockResolvedValue([]);
  vi.spyOn(api, 'createNoteInProject').mockResolvedValue(makeCreatedMeta());
  vi.spyOn(api, 'listTasksByNote').mockResolvedValue([]);
  vi.spyOn(api, 'getBacklinks' as any).mockResolvedValue([]);
  vi.spyOn(api, 'rebuildNoteLinks').mockResolvedValue(undefined as any);

  vi.spyOn(api, 'saveNote').mockImplementation(async (_notebook, _slug, content, title) => {
    savedContent = content;
    if (title) savedTitle = title;
    return { ...makeCreatedMeta(), title: savedTitle } as any;
  });
  vi.spyOn(api, 'readNote').mockImplementation(async () => {
    if (readNoteHang) return new Promise<any>(() => {}); // never resolves
    return {
      meta: { ...makeCreatedMeta(), title: savedTitle },
      content: savedContent,
    } as any;
  });
  vi.spyOn(api, 'getNoteTaskFields').mockResolvedValue({
    title: null, task_status: null, task_priority: null, task_due_date: null, task_assignee: null,
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

describe('#3044 delta-2 — Cmd+N from Calendar (NotesBrowser genuinely unmounted at the time of the keypress)', () => {
  it('creates and opens the note; $activeNote survives NotesBrowser mounting for the first time', async () => {
    render(CanvasRenderer, { props: {} });
    await flushAsync();

    // Sanity: we really are on Calendar, NotesBrowser's editor is not on screen.
    expect(screen.queryByTestId('note-title-input')).toBeNull();

    // Exactly what +page.svelte's keydown handler calls.
    await noteService.createNewNote();
    await flushAsync();

    // This is the exact assertion Reed's repro #2 failed at base: before the
    // fix, $activeNote was null here (NotesBrowser's onMount had already
    // wiped it in response to the canvasState.view write above).
    expect(get(activeNote)?.meta?.id).toBe(NEW_NOTE_ID);

    const titleInput = await screen.findByTestId('note-title-input') as HTMLInputElement;
    expect(titleInput.value).toBe('Untitled');
  });

  it('typing a title and body into the note created from Calendar actually saves — survives a reopen', async () => {
    render(CanvasRenderer, { props: {} });
    await flushAsync();

    await noteService.createNewNote();
    await flushAsync();

    // Same immediate, pre-self-heal check as the first test in this file —
    // CanvasRenderer's OWN #2335 reload-guard effect will eventually notice
    // the mismatch this race causes and re-fetch the note from the backend
    // (a real, separate recovery path, not this fix), which would otherwise
    // mask the race by the time `findByTestId` below resolves. Asserting
    // here, before that effect's async round-trip can run, is what makes
    // this test red at base for the right reason.
    expect(get(activeNote)?.meta?.id).toBe(NEW_NOTE_ID);

    const titleInput = await screen.findByTestId('note-title-input') as HTMLInputElement;
    await fireEvent.input(titleInput, { target: { value: 'Note From Calendar' } });
    await fireEvent.blur(titleInput);
    await flushAsync();

    await waitFor(() => {
      expect(api.saveNote).toHaveBeenCalled();
    });
    const titleCall = (api.saveNote as any).mock.calls.at(-1);
    expect(titleCall[3]).toBe('Note From Calendar');

    // Reopen. Release the readNote hang now — this step legitimately needs
    // it to resolve, and the critical mount-race assertion above has already
    // run.
    readNoteHang = false;
    activeNote.set(null);
    await flushAsync();
    await noteService.selectNote({ ...makeCreatedMeta(), title: savedTitle });
    await flushAsync();

    const reopened = get(activeNote);
    expect(reopened?.meta?.title).toBe('Note From Calendar');

    const reopenedTitleInput = await screen.findByTestId('note-title-input') as HTMLInputElement;
    expect(reopenedTitleInput.value).toBe('Note From Calendar');
  });
});
