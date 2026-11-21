/**
 * NotesBrowser.mount-clears-activeNote.test.ts — #2874 item 15, fix #3.
 * UPDATED for #3044 delta-2 (Reed).
 *
 * NotesBrowser.svelte's `onMount` clears a STALE `$activeNote` left over
 * from whatever view was active before folder view mounted (possibly with
 * a corrupted/missing `note_type`, per the item 15 diagnosis's "$activeNote
 * carry-over" transport mechanism), which would otherwise get picked up by
 * the `fvNoteOpen` effect (NotesBrowser.svelte ~line 191:
 * `$effect(() => { if ($activeNote !== null) fvNoteOpen = true; })`) and
 * reopened in folder view's own note panel — even though the user never
 * clicked anything in folder view.
 *
 * #3044 delta-2: that clear used to be unconditional (mirroring
 * KanbanBoard.svelte's own onMount clear) — which ALSO wiped a note THIS
 * SAME mount was deliberately opening (Cmd+N from a non-Notes screen, a
 * wiki-link click from AllTasksView/Kanban — see
 * `CanvasRenderer.cmdN-unmounted-note-race.test.ts` /
 * `CanvasRenderer.wikilink-unmounted-note-race.test.ts` for the end-to-end
 * proof). It now calls the shared `noteService.clearStaleActiveNoteOnMount()`
 * — keep $activeNote when it already matches `canvasState.context.noteId`
 * (the note THIS navigation targeted), clear in every other case. The
 * "clears a genuinely stale carry-over" tests below pin that the original
 * protection survives; the new "keeps a deliberately-opened note" describe
 * block pins the fix.
 *
 * CONFIRMED SAFE precondition (amendments doc Q1): NotesBrowser mounts
 * exactly ONCE per app session (single non-keyed mount site in
 * CanvasRenderer.svelte) — it does NOT remount on folder switch, it reacts
 * to `$activeFolder` via `$effect`. So a bare, unconditional `onMount` clear
 * cannot clobber an intentionally-just-opened note on folder switch within
 * folder view itself.
 *
 * Real render (not source-regex) — this is exactly the class of
 * mount-lifecycle behavior a regex over source text cannot verify.
 */

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, cleanup, screen } from '@testing-library/svelte/svelte5';
import { get } from 'svelte/store';
import NotesBrowser from './NotesBrowser.svelte';
import * as api from '$lib/api';
import { activeProject, projects, activeNote } from '$lib/stores/app';
import { canvasState } from '$lib/stores/canvas';

// vitest-setup.ts replaces `globalThis.window` with a plain mock object
// lacking addEventListener/removeEventListener/ResizeObserver — NotesBrowser
// registers window listeners and a ResizeObserver in onMount (same pattern
// as NotesBrowser.new-sheet-button-polish.test.ts).
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
// #3044 delta-2's new "keeps a deliberately-opened note" describe block
// below is the first test in this file where $activeNote survives mount —
// which means the real Editor.svelte (TipTap) mounts for it, needing the
// same DOMParser/setTimeout/clearTimeout/document/Range shims the other
// Editor-mounting test files in this repo already apply (e.g.
// NotesBrowser.cmdN-save.test.ts).
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

function makeStaleActiveNote(overrides: Record<string, any> = {}) {
  return {
    meta: {
      id: 'stale-note-1',
      title: 'Stale Sheet Left Over From Kanban',
      slug: 'stale-note-1',
      notebook: 'Some Other Project',
      preview: '',
      created_at: '',
      updated_at: '',
      // Simulates the exact corruption item 15 fixes: note_type dropped by
      // the KanbanBoard.svelte hand-built meta bug (pre-fix #1).
      note_type: undefined,
      ...overrides,
    },
    content: '',
    jsonContent: null,
  };
}

beforeEach(() => {
  activeProject.set({
    id: 'project-1',
    name: 'Project One',
    parent_id: null,
    created_at: '',
    updated_at: '',
  } as any);
  projects.set([
    { id: 'project-1', name: 'Project One', parent_id: null, created_at: '', updated_at: '' } as any,
  ]);
  vi.spyOn(api, 'listProjects').mockResolvedValue([
    { id: 'project-1', name: 'Project One', parent_id: null, created_at: '', updated_at: '' } as any,
  ]);
  vi.spyOn(api, 'listFolders').mockResolvedValue([]);
  vi.spyOn(api, 'listNotesByProject').mockResolvedValue([]);
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  activeProject.set(null);
  projects.set([]);
  activeNote.set(null);
  canvasState.set({ view: 'home', data: {}, context: {} } as any);
});

describe('NotesBrowser — clears stale $activeNote on mount (item 15 §3)', () => {
  it('sets $activeNote to null on mount when a stale note was left over from another view', async () => {
    activeNote.set(makeStaleActiveNote() as any);
    expect(get(activeNote)).not.toBeNull();

    render(NotesBrowser, { props: {} });

    // RED-ON-REVERT: removing the `$activeNote = null;` first statement in
    // NotesBrowser's onMount leaves the stale note in place here.
    expect(get(activeNote)).toBeNull();
  });

  it('does not reopen a stale sheet note\'s Editor after mount, even if the fv-note panel shell briefly latches open', async () => {
    // NotesBrowser.svelte's own #2192 comment documents that `.fv-note`'s
    // outer guard is `fvNoteOpen` (not `$activeNote` directly) so the panel
    // shell can stay mounted during noteService.selectNote's mandatory null
    // gap without flashing. That means fvNoteOpen may still latch true if
    // the `$activeNote !== null` effect observes the stale value before
    // onMount's clear runs. What actually matters — and what this test
    // pins — is the INNER guard: `{#if $activeNote}` wraps the real
    // <Editor>/<SheetGrid> mount, and $activeNote is null once onMount has
    // run. So no Editor, and specifically no stale SheetGrid, ever mounts
    // for the carried-over note.
    activeNote.set(makeStaleActiveNote({ note_type: 'sheet' }) as any);

    render(NotesBrowser, { props: {} });

    expect(get(activeNote)).toBeNull();
    expect(screen.queryByTestId('sheet-grid')).toBeNull();
    expect(document.querySelector('.ProseMirror')).toBeNull();
  });

  it('a genuinely null $activeNote at mount time (the common case) is unaffected — still null, no error', async () => {
    activeNote.set(null);

    expect(() => render(NotesBrowser, { props: {} })).not.toThrow();
    expect(get(activeNote)).toBeNull();
  });
});

describe('NotesBrowser — keeps a deliberately-opened note on mount (#3044 delta-2 fix)', () => {
  it('does NOT clear $activeNote on mount when it already matches canvasState.context.noteId', async () => {
    activeNote.set(makeStaleActiveNote({ id: 'deliberately-opened-note', title: 'Just Created' }) as any);
    canvasState.set({
      view: 'note_editor',
      data: {},
      context: { noteId: 'deliberately-opened-note', projectId: 'project-1' },
    } as any);

    render(NotesBrowser, { props: {} });

    // RED pre-#3044-delta-2: the old unconditional `$activeNote = null;`
    // wiped this regardless of context.noteId.
    expect(get(activeNote)?.meta?.id).toBe('deliberately-opened-note');
  });
});
