/**
 * NotesBrowser.cmdN-save.test.ts — #3044, revisited (Reed's delta1 audit
 * WARN, `docs/audits/2026-10-17-pr-3044-delta1.md` §2).
 *
 * History: Matt originally reported notes made with Cmd+N or Cmd+T don't
 * save their title or content, while the New Note button works. The first
 * fix (#3044 item 3) routed Cmd+N through `window.__paneNewNote`, a global
 * `NotesBrowser.svelte`'s own `onMount`/`onDestroy` registered/cleared.
 * Reed's audit found that relocated the race into a silent no-op: the
 * global went `undefined` every time `NotesBrowser` unmounted (any
 * non-notes view), so Cmd+N did nothing — no note, no error — from most of
 * the app, untested.
 *
 * The fix this time: `noteService.createNewNote()` is the ONE shared
 * function — no window global, no "is the right component mounted"
 * precondition at all (see `noteService.createNewNote.test.ts` for the
 * zero-components-mounted proof). `+page.svelte`'s Cmd+N handler and this
 * component's own New Note button both call it directly.
 *
 * THIS file covers the "Cmd+N from the notes browser" screen test Matt
 * asked for specifically: the real `NotesBrowser.svelte` + real
 * `Editor.svelte` mounted (same house pattern as
 * `Editor.attachment-onload-dirty.test.ts`), with `noteService.createNewNote()`
 * invoked exactly as `+page.svelte`'s keydown handler now does it, proving
 * the note that results actually saves title AND body and survives a
 * reopen — plus a separate check that the New Note BUTTON (a real DOM
 * click, not a direct function call) still works from here too, and a
 * grep-style guard that no `window.__paneNewNote` reference survives
 * anywhere in this component.
 */

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, cleanup, screen, fireEvent, waitFor } from '@testing-library/svelte/svelte5';
import { tick } from 'svelte';
import { get } from 'svelte/store';
import NotesBrowser from './NotesBrowser.svelte';
import * as noteService from '$lib/services/noteService';
import * as api from '$lib/api';
import { activeProject, activeFolder, projects, activeNote } from '$lib/stores/app';
import type { Project, Folder, NoteMeta } from '$lib/types';

// vitest-setup.ts replaces `globalThis.window` with a plain mock object
// lacking addEventListener/removeEventListener/ResizeObserver/DOMParser/
// setTimeout/clearTimeout/document — same polyfill set
// NotesBrowser.mount-clears-activeNote.test.ts and
// Editor.attachment-onload-dirty.test.ts both already require.
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
if (typeof (globalThis as any).ResizeObserver === 'undefined') {
  (globalThis as any).ResizeObserver = class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
}
if (typeof win.ResizeObserver === 'undefined') {
  win.ResizeObserver = (globalThis as any).ResizeObserver;
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
if (typeof win.document === 'undefined' && typeof document !== 'undefined') {
  win.document = document;
}

// jsdom's Range/Element don't implement getClientRects — ProseMirror's
// `coordsAtPos` (called on every state update, including a paste, to keep
// the caret scrolled into view) calls it unconditionally and throws without
// this polyfill. Same category of gap as the getBoundingClientRect stub
// other component tests in this repo already apply (CabinetDragReorder.test.ts,
// AgentPickerPopover.test.ts) — jsdom performs no real layout, so a zeroed
// rect list is the correct inert stand-in.
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

// Advances real timers in small increments while flushing microtasks/Svelte
// ticks between each step, so Editor.svelte's 500ms save-debounce timer
// gets a chance to actually fire. Same pattern as
// Editor.attachment-onload-dirty.test.ts's advanceAndFlush.
async function advanceAndFlush(totalMs: number, stepMs = 50) {
  let waited = 0;
  while (waited < totalMs) {
    await new Promise((resolve) => setTimeout(resolve, stepMs));
    await flushAsync();
    waited += stepMs;
  }
}

const FOLDER: Folder = {
  id: 'folder-cmdn',
  name: 'General',
  parent_id: 'proj-cmdn',
  is_system: false,
  created_at: '',
  updated_at: '',
} as any;

const PROJECT: Project = {
  id: 'proj-cmdn',
  name: 'Symbiotic',
  parent_id: null,
  created_at: '',
  updated_at: '',
  // #3044 revisited: the shared function reads THIS field directly — not
  // a first-non-system-folder scan — so it must be set here for the
  // "creates in the right folder" assertions below to mean anything.
  default_folder_id: FOLDER.id,
} as any;

const NEW_NOTE_ID = 'cmdn-new-note-1';

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
    folder_id: FOLDER.id,
    file_path: undefined,
    note_type: undefined,
  } as any;
}

// In-memory "backend" so saveNote/readNote round-trip for real, proving the
// reopen half of the contract (not just that saveNote was called once).
let savedTitle = 'Untitled';
let savedContent = '';

beforeEach(() => {
  savedTitle = 'Untitled';
  savedContent = '';

  activeProject.set(PROJECT);
  activeFolder.set(null);
  projects.set([PROJECT]);
  activeNote.set(null);

  vi.spyOn(api, 'listFolders').mockResolvedValue([FOLDER]);
  vi.spyOn(api, 'listNotesByProject').mockResolvedValue([]);
  vi.spyOn(api, 'createNoteInProject').mockResolvedValue(makeCreatedMeta());
  vi.spyOn(api, 'getSheetCardSummaries').mockResolvedValue([]);
  vi.spyOn(api, 'listTasksByNote').mockResolvedValue([]);
  vi.spyOn(api, 'getBacklinks' as any).mockResolvedValue([]);
  vi.spyOn(api, 'rebuildNoteLinks').mockResolvedValue(undefined as any);

  vi.spyOn(api, 'saveNote').mockImplementation(async (_notebook, _slug, content, title) => {
    savedContent = content;
    if (title) savedTitle = title;
    return { ...makeCreatedMeta(), title: savedTitle } as any;
  });

  // Reopen path: noteService.selectNote() -> api.readNote() returns
  // whatever was actually saved, proving a genuine round-trip.
  vi.spyOn(api, 'readNote').mockImplementation(async () => ({
    meta: { ...makeCreatedMeta(), title: savedTitle },
    content: savedContent,
  } as any));
  vi.spyOn(api, 'getNoteTaskFields').mockResolvedValue({
    title: null, task_status: null, task_priority: null, task_due_date: null, task_assignee: null,
  } as any);
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  activeProject.set(null);
  activeFolder.set(null);
  projects.set([]);
  activeNote.set(null);
});

describe('#3044 (revisited) — no window.__paneNewNote reference survives anywhere', () => {
  it('window.__paneNewNote is never set by mounting (or unmounting) NotesBrowser', async () => {
    const { unmount } = render(NotesBrowser, { props: {} });
    await flushAsync();
    expect((window as any).__paneNewNote).toBeUndefined();
    unmount();
    await flushAsync();
    expect((window as any).__paneNewNote).toBeUndefined();
  });

  it('NotesBrowser.svelte\'s own source contains no __paneNewNote reference at all', async () => {
    const { readFileSync } = await import('fs');
    const { resolve } = await import('path');
    const source = readFileSync(resolve(__dirname, 'NotesBrowser.svelte'), 'utf-8');
    expect(source).not.toMatch(/__paneNewNote/);
  });
});

describe('#3044 (revisited) — Cmd+N "from the notes browser" creates a note that actually saves title and body', () => {
  it('noteService.createNewNote() — exactly what +page.svelte\'s Cmd+N handler calls — creates via the project default_folder_id while NotesBrowser is mounted', async () => {
    render(NotesBrowser, { props: {} });
    await flushAsync();

    await noteService.createNewNote();
    await flushAsync();

    expect(api.createNoteInProject).toHaveBeenCalledWith(PROJECT.id, FOLDER.id);
    expect(get(activeNote)?.meta?.id).toBe(NEW_NOTE_ID);
  });

  it('Cmd+N, type a title and a body — both reach api.saveNote; reopening shows both saved (not blank)', async () => {
    render(NotesBrowser, { props: {} });
    await flushAsync();

    // Cmd+N — called exactly as +page.svelte's handleKeydown now does.
    await noteService.createNewNote();
    await flushAsync();

    // The real Editor must now be showing the brand-new note.
    const titleInput = await screen.findByTestId('note-title-input') as HTMLInputElement;
    expect(titleInput.value).toBe('Untitled');

    // Type a title (real DOM input event -> Editor.svelte's handleTitleChange
    // sets dirty=true), then blur to trigger the immediate title-save path.
    await fireEvent.input(titleInput, { target: { value: 'My Cmd+N Note' } });
    await fireEvent.blur(titleInput);
    await flushAsync();

    await waitFor(() => {
      expect(api.saveNote).toHaveBeenCalled();
    });
    const titleCall = (api.saveNote as any).mock.calls.at(-1);
    expect(titleCall[1]).toBe(NEW_NOTE_ID); // slug/noteId
    expect(titleCall[3]).toBe('My Cmd+N Note'); // title

    // Type a body via a real ProseMirror paste event — the deterministic,
    // DOM-diffing-free way ProseMirror ingests external text (handlePaste
    // reads event.clipboardData directly), landing a real transaction in
    // the real mounted TipTap doc.
    const pmEl = document.querySelector('.ProseMirror') as HTMLElement;
    expect(pmEl).not.toBeNull();
    pmEl.focus();
    const clipboardData = {
      getData: (fmt: string) => (fmt === 'text/plain' ? 'Hello from Cmd+N body text' : ''),
      types: ['text/plain'],
      files: [],
    };
    await fireEvent.paste(pmEl, { clipboardData });
    await flushAsync();

    // Let the 500ms content-save debounce fire.
    await advanceAndFlush(700);

    await waitFor(() => {
      const bodyCall = (api.saveNote as any).mock.calls.at(-1);
      expect(bodyCall[2]).toContain('Hello from Cmd+N body text');
    });

    // ── Reopen ──────────────────────────────────────────────────────────
    // Simulate closing and reopening the note: null activeNote, then
    // re-select it exactly like clicking its card would — through the real
    // noteService.selectNote() -> api.readNote() round-trip, which now
    // returns whatever was actually persisted above.
    activeNote.set(null);
    await flushAsync();
    await noteService.selectNote({ ...makeCreatedMeta(), title: savedTitle });
    await flushAsync();

    const reopened = get(activeNote);
    expect(reopened?.meta?.title).toBe('My Cmd+N Note');
    expect(reopened?.content).toContain('Hello from Cmd+N body text');

    const reopenedTitleInput = await screen.findByTestId('note-title-input') as HTMLInputElement;
    expect(reopenedTitleInput.value).toBe('My Cmd+N Note');
    expect(document.querySelector('.ProseMirror')?.textContent).toContain('Hello from Cmd+N body text');
  });
});

describe('#3044 (revisited) — the New Note BUTTON still works from the notes browser (a real DOM click, not a direct function call)', () => {
  it('clicking "New Note" creates via createNoteInProject(project.id, default_folder_id) and opens the new note in the Editor', async () => {
    render(NotesBrowser, { props: {} });
    await flushAsync();

    const button = await screen.findByTitle('New Note');
    await fireEvent.click(button);
    // createNewNote() awaits a real 20ms DB-flush buffer (setTimeout) before
    // calling api.createNoteInProject — give it real wall-clock time, not
    // just microtask flushes.
    await waitFor(() => {
      expect(api.createNoteInProject).toHaveBeenCalled();
    });
    await flushAsync();

    expect(api.createNoteInProject).toHaveBeenCalledWith(PROJECT.id, FOLDER.id);
    expect(get(activeNote)?.meta?.id).toBe(NEW_NOTE_ID);

    const titleInput = await screen.findByTestId('note-title-input') as HTMLInputElement;
    expect(titleInput.value).toBe('Untitled');
  });
});
