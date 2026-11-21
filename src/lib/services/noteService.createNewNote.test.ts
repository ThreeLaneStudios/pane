/**
 * noteService.createNewNote.test.ts — #3044 (revisited).
 *
 * Reed's delta1 audit on #3044 (`docs/audits/2026-10-17-pr-3044-delta1.md`,
 * §2 WARN) found the previous fix "relocated, not removed" the Cmd+N race:
 * routing creation through `window.__paneNewNote` meant Cmd+N silently did
 * NOTHING (no note, no error, no UI feedback) from every screen where
 * `NotesBrowser.svelte` isn't mounted — which is most of the app.
 *
 * Matt's ruling this time: move the New Note button's creation code into
 * ONE shared function that BOTH the button and Cmd+N call directly, with
 * no global in between. That function is `noteService.createNewNote()`.
 * It must not depend on any component being mounted — these tests prove
 * exactly that by calling it directly, with ZERO Svelte components
 * rendered anywhere in this file.
 *
 * Coverage:
 *   1. Inside a project: creates the note via api.createNoteInProject using
 *      the PROJECT'S OWN default_folder_id — never a "first non-system
 *      folder" scan, never a hardcoded 'AI'/'Pane System' name check (the
 *      leftover island #3044 already removed once from the old
 *      NotesBrowser-local handleNewNote).
 *   2. Inside a project: opens the result — sets $activeNote AND navigates
 *      canvasState to 'note_editor' with the right context — regardless of
 *      which view was active beforehand (proving "whichever screen you're
 *      on").
 *   3. Outside a project ($activeProject is null): creates NOTHING and
 *      dispatches 'pane-new-note-blocked' on window instead — the signal
 *      +page.svelte turns into the "Open a project to make a note."
 *      message. This is the exact scenario Cmd+N from Home/Calendar/All
 *      Tasks/Trash/the project list hits.
 *   4. Works with zero components mounted — the core structural claim.
 *   5. RED-ON-REVERT: source-text guard against the old is_system/'AI'
 *      folder-scan pattern creeping back into this function.
 */

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { get } from 'svelte/store';
import * as api from '$lib/api';
import * as noteService from './noteService';
import { activeProject, activeNote } from '$lib/stores/app';
import { canvasState } from '$lib/stores/canvas';
import type { Project, NoteMeta } from '$lib/types';

// vitest-setup.ts replaces `globalThis.window` with a plain mock object
// lacking addEventListener/removeEventListener/dispatchEvent —
// createNewNote() dispatches a real 'pane-new-note-blocked' CustomEvent on
// `window` when called outside a project, so these need rebinding to the
// real globalThis EventTarget (same shim several other component tests in
// this repo already apply).
const win = globalThis.window as any;
if (typeof win.addEventListener !== 'function') {
  win.addEventListener = globalThis.addEventListener.bind(globalThis);
}
if (typeof win.removeEventListener !== 'function') {
  win.removeEventListener = globalThis.removeEventListener.bind(globalThis);
}
// vitest-setup.ts's mock `window` already HAS a `dispatchEvent` — a
// hardcoded no-op that always returns `true` without touching any real
// listener registry (see vitest-setup.ts's own comment on that mock). The
// `typeof ... !== 'function'` guard other test files use for this exact
// shim therefore never fires for `dispatchEvent` specifically — it has to
// be force-replaced, unconditionally, for a real `window.addEventListener`
// listener (bound to real globalThis below) to ever actually receive it.
win.dispatchEvent = globalThis.dispatchEvent.bind(globalThis);

const PROJECT: Project = {
  id: 'proj-shared-fn',
  name: 'Symbiotic',
  parent_id: undefined,
  sort_order: 0,
  created_at: '',
  updated_at: '',
  is_system: false,
  default_folder_id: 'folder-default-general',
} as any;

const NEW_NOTE_ID = 'shared-fn-new-note-1';

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
    folder_id: PROJECT.default_folder_id!,
    file_path: undefined,
    note_type: undefined,
  } as any;
}

beforeEach(() => {
  activeProject.set(null);
  activeNote.set(null);
  canvasState.set({ view: 'home', data: {}, context: {} });
  vi.spyOn(api, 'createNoteInProject').mockResolvedValue(makeCreatedMeta());
});

afterEach(() => {
  vi.restoreAllMocks();
  activeProject.set(null);
  activeNote.set(null);
  canvasState.set({ view: 'home', data: {}, context: {} });
});

describe('noteService.createNewNote() — #3044 shared function, zero components mounted', () => {
  it('inside a project: calls api.createNoteInProject with the PROJECT\'s default_folder_id', async () => {
    activeProject.set(PROJECT);

    await noteService.createNewNote();

    expect(api.createNoteInProject).toHaveBeenCalledWith(PROJECT.id, PROJECT.default_folder_id);
  });

  it('inside a project: sets $activeNote to the newly created note', async () => {
    activeProject.set(PROJECT);

    await noteService.createNewNote();

    const note = get(activeNote);
    expect(note?.meta?.id).toBe(NEW_NOTE_ID);
  });

  it('inside a project: navigates canvasState to note_editor with projectId/folderId/noteId set — "opens it"', async () => {
    activeProject.set(PROJECT);
    // Start somewhere that does NOT render NotesBrowser at all — proves
    // "opens it, whichever screen you're on" rather than relying on a
    // component that happened to already be showing.
    canvasState.set({ view: 'project_config', data: {}, context: { projectId: PROJECT.id } });

    await noteService.createNewNote();

    const state = get(canvasState);
    expect(state.view).toBe('note_editor');
    expect(state.context.projectId).toBe(PROJECT.id);
    expect(state.context.folderId).toBe(PROJECT.default_folder_id);
    expect(state.context.noteId).toBe(NEW_NOTE_ID);
  });

  it('a project with NO default_folder_id creates with folderId=undefined — never falls back to a folder scan', async () => {
    const projectNoDefault = { ...PROJECT, default_folder_id: null };
    activeProject.set(projectNoDefault as any);

    await noteService.createNewNote();

    expect(api.createNoteInProject).toHaveBeenCalledWith(PROJECT.id, undefined);
  });

  it('outside a project ($activeProject is null): creates NOTHING', async () => {
    activeProject.set(null);

    await noteService.createNewNote();

    expect(api.createNoteInProject).not.toHaveBeenCalled();
    expect(get(activeNote)).toBeNull();
  });

  it('outside a project: dispatches pane-new-note-blocked exactly once on window — this is "Cmd+N from Home" end to end', async () => {
    activeProject.set(null);
    const handler = vi.fn();
    window.addEventListener('pane-new-note-blocked', handler);
    try {
      await noteService.createNewNote();
      expect(handler).toHaveBeenCalledTimes(1);
    } finally {
      window.removeEventListener('pane-new-note-blocked', handler);
    }
  });

  it('outside a project: does NOT touch canvasState at all (no partial navigation)', async () => {
    activeProject.set(null);
    canvasState.set({ view: 'home', data: {}, context: {} });

    await noteService.createNewNote();

    const state = get(canvasState);
    expect(state.view).toBe('home');
  });

  it('works correctly called twice in a row with no project in between — no leaked state from a previous call', async () => {
    activeProject.set(null);
    await noteService.createNewNote();
    expect(api.createNoteInProject).not.toHaveBeenCalled();

    activeProject.set(PROJECT);
    await noteService.createNewNote();
    expect(api.createNoteInProject).toHaveBeenCalledTimes(1);
    expect(get(activeNote)?.meta?.id).toBe(NEW_NOTE_ID);
  });
});

describe('RED-ON-REVERT guard — #3044 ruling: default_folder_id, never a folder scan or hardcoded name check', () => {
  it('createNewNote()\'s source does not filter folders by is_system or check for the literal folder names \'AI\'/\'Pane System\'', async () => {
    const { readFileSync } = await import('fs');
    const { resolve } = await import('path');
    const source = readFileSync(resolve(__dirname, 'noteService.ts'), 'utf-8');
    const fnMatch = source.match(/export async function createNewNote\(\)[\s\S]*?\n\}/);
    expect(fnMatch).toBeTruthy();
    const body = fnMatch![0];
    expect(body).not.toMatch(/is_system/);
    expect(body).not.toMatch(/'AI'/);
    expect(body).not.toMatch(/Pane System/);
    expect(body).not.toMatch(/nonSystemFolders/);
    // The one correct source of the folder id — proves it's actually used,
    // not just absent-by-coincidence.
    expect(body).toMatch(/default_folder_id/);
  });
});
