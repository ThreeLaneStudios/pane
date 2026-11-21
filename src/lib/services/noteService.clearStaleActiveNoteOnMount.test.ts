/**
 * noteService.clearStaleActiveNoteOnMount.test.ts — #3044 delta-2 (Reed).
 *
 * The shared check. `NotesBrowser.svelte`'s and `KanbanBoard.svelte`'s
 * `onMount` both used to run an unconditional `$activeNote = null` as
 * their first statement — a real protection against a STALE note carried
 * over from whatever view was active before THIS one mounted, but one
 * that ALSO wiped a note a navigation had just deliberately opened
 * (`createNewNote()`/`selectNote()` both do `activeNote.set(note)`
 * immediately followed by `canvasState.update(... view ...)`, no `await`
 * between the two — when that write is this component's FIRST mount, the
 * unconditional clear ran right after and undid it, before anyone could
 * observe the note existed).
 *
 * `clearStaleActiveNoteOnMount()` is the one place both components' onMount
 * now call. These are direct unit tests of the function itself — zero
 * Svelte components mounted anywhere in this file, same "works with
 * nothing on screen" posture as `noteService.createNewNote.test.ts`. The
 * two `onMount` call sites get their own screen-level coverage elsewhere
 * (`NotesBrowser.mount-clears-activeNote.test.ts`,
 * `KanbanBoard.mount-clears-activeNote.test.ts`, and the set-then-navigate
 * screen tests in `*.unmounted-note-race.test.ts`).
 */

import { describe, it, expect, afterEach } from 'vitest';
import { get } from 'svelte/store';
import { clearStaleActiveNoteOnMount } from './noteService';
import { activeNote } from '$lib/stores/app';
import { canvasState } from '$lib/stores/canvas';
import type { NoteContent } from '$lib/types';

function makeNote(id: string): NoteContent {
  return {
    meta: {
      id,
      title: `Note ${id}`,
      slug: id,
      notebook: 'Some Project',
      preview: '',
      created_at: '',
      updated_at: '',
    } as any,
    content: '',
    jsonContent: null,
  } as any;
}

afterEach(() => {
  activeNote.set(null);
  canvasState.set({ view: 'home', data: {}, context: {} } as any);
});

describe('clearStaleActiveNoteOnMount — keeps a deliberately-opened note', () => {
  it('keeps $activeNote when its id matches canvasState.context.noteId (the note THIS mount was navigated to)', () => {
    activeNote.set(makeNote('target-note'));
    canvasState.set({ view: 'note_editor', data: {}, context: { noteId: 'target-note' } } as any);

    clearStaleActiveNoteOnMount();

    expect(get(activeNote)?.meta?.id).toBe('target-note');
  });
});

describe('clearStaleActiveNoteOnMount — clears a genuinely stale carry-over', () => {
  it('clears $activeNote when context.noteId is unset (no navigation target at all — the common stale-carry-over shape)', () => {
    activeNote.set(makeNote('stale-note'));
    canvasState.set({ view: 'kanban', data: {}, context: {} } as any);

    clearStaleActiveNoteOnMount();

    expect(get(activeNote)).toBeNull();
  });

  it('clears $activeNote when context.noteId points at a DIFFERENT note than what is active', () => {
    activeNote.set(makeNote('stale-note'));
    canvasState.set({ view: 'note_editor', data: {}, context: { noteId: 'some-other-note' } } as any);

    clearStaleActiveNoteOnMount();

    expect(get(activeNote)).toBeNull();
  });

  it('is a no-op (stays null, does not throw) when $activeNote is already null', () => {
    activeNote.set(null);
    canvasState.set({ view: 'kanban', data: {}, context: {} } as any);

    expect(() => clearStaleActiveNoteOnMount()).not.toThrow();
    expect(get(activeNote)).toBeNull();
  });
});
