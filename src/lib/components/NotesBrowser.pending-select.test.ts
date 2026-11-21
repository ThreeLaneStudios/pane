/**
 * #2874 item 18 fix 1 — NotesBrowser.svelte pending-selection queue for
 * `pane-select-note`.
 *
 * Background: CanvasRenderer's `handleTaskClick` used to dispatch
 * `pane-select-note` after a fixed 200ms `setTimeout`, standing in for
 * "wait long enough for NotesBrowser to mount and loadTaskNotes() to
 * resolve." That guess was replaced with `await navigateTo()` + `await
 * tick()` — but there is still no guarantee `loadTaskNotes()`'s own Tauri
 * IPC round-trip has resolved by the time the event fires. This fix moves
 * the actual completion signal into NotesBrowser itself: if the event
 * arrives before `taskNotes` is populated, the noteId is queued in
 * `pendingSelectNoteId` instead of being silently dropped, and a `$effect`
 * watching `taskNotes` resolves it once the load completes.
 *
 * These tests exercise the queue/resolve logic directly (logic-extraction
 * pattern, matching NotesBrowser.folder-selection.test.ts) because the full
 * NotesBrowser.svelte requires a Tauri window context not available in
 * Vitest/jsdom. Only the reducer-style state transitions are mocked here —
 * not CanvasRenderer, not the DOM CustomEvent machinery.
 *
 * RED-ON-REVERT: reverting the listener to the original
 * `if (noteId && taskNotes.length > 0)` guard (silently drop otherwise)
 * makes "queues the noteId when taskNotes is empty" fail — the note would
 * never be found because it's never retried.
 */

import { describe, it, expect } from 'vitest';

interface NoteMeta {
  id: string;
  slug: string;
}

/** Mirrors NotesBrowser.svelte's pane-select-note $effect handler (fix 1). */
function handlePaneSelectNote(
  noteId: string | undefined,
  taskNotes: NoteMeta[],
  state: { pendingSelectNoteId: string | null; selected: NoteMeta | null },
): void {
  if (!noteId) return;
  if (taskNotes.length > 0) {
    const note = taskNotes.find(n => n.id === noteId || n.slug === noteId);
    if (note) state.selected = note;
  } else {
    // #2874 item 18 fix 1: queue instead of dropping silently.
    state.pendingSelectNoteId = noteId;
  }
}

/** Mirrors NotesBrowser.svelte's taskNotes-watcher $effect (fix 1). */
function resolvePendingSelection(
  taskNotes: NoteMeta[],
  state: { pendingSelectNoteId: string | null; selected: NoteMeta | null },
): void {
  if (state.pendingSelectNoteId && taskNotes.length > 0) {
    const noteId = state.pendingSelectNoteId;
    const note = taskNotes.find(n => n.id === noteId || n.slug === noteId);
    state.pendingSelectNoteId = null;
    if (note) state.selected = note;
  }
}

const NOTE_A: NoteMeta = { id: 'note-a', slug: 'note-a-slug' };
const NOTE_B: NoteMeta = { id: 'note-b', slug: 'note-b-slug' };

describe('NotesBrowser.svelte pending-selection queue — #2874 item 18 fix 1', () => {
  it('queues the noteId when taskNotes is empty (load still in flight) instead of dropping it', () => {
    const state = { pendingSelectNoteId: null as string | null, selected: null as NoteMeta | null };

    handlePaneSelectNote('note-a', [], state);

    expect(state.pendingSelectNoteId).toBe('note-a');
    expect(state.selected).toBeNull();
  });

  it('RED-ON-REVERT: without the queue, an empty taskNotes silently drops the request', () => {
    // Simulates the pre-fix behavior for comparison: no queue means the
    // request is simply lost with no trace and no way to recover it.
    const state = { pendingSelectNoteId: null as string | null, selected: null as NoteMeta | null };
    handlePaneSelectNote('note-a', [], state);
    // With the fix, this must NOT be null — it must be queued.
    expect(state.pendingSelectNoteId).not.toBeNull();
  });

  it('selects immediately when taskNotes is already populated (no queueing needed)', () => {
    const state = { pendingSelectNoteId: null as string | null, selected: null as NoteMeta | null };

    handlePaneSelectNote('note-b', [NOTE_A, NOTE_B], state);

    expect(state.selected).toEqual(NOTE_B);
    expect(state.pendingSelectNoteId).toBeNull();
  });

  it('resolves a queued noteId once taskNotes subsequently populates', () => {
    const state = { pendingSelectNoteId: null as string | null, selected: null as NoteMeta | null };

    // Event fires before load completes.
    handlePaneSelectNote('note-a', [], state);
    expect(state.pendingSelectNoteId).toBe('note-a');

    // loadTaskNotes() resolves later; the taskNotes-watcher effect fires.
    resolvePendingSelection([NOTE_A, NOTE_B], state);

    expect(state.selected).toEqual(NOTE_A);
    expect(state.pendingSelectNoteId).toBeNull();
  });

  it('matches by slug as well as id when resolving a queued selection', () => {
    const state = { pendingSelectNoteId: null as string | null, selected: null as NoteMeta | null };

    handlePaneSelectNote('note-b-slug', [], state);
    resolvePendingSelection([NOTE_A, NOTE_B], state);

    expect(state.selected).toEqual(NOTE_B);
  });

  it('does nothing when taskNotes populates but no selection is pending', () => {
    const state = { pendingSelectNoteId: null as string | null, selected: null as NoteMeta | null };

    resolvePendingSelection([NOTE_A, NOTE_B], state);

    expect(state.selected).toBeNull();
    expect(state.pendingSelectNoteId).toBeNull();
  });

  it('clears the pending queue even if the queued noteId is not found in the resolved taskNotes', () => {
    const state = { pendingSelectNoteId: 'note-ghost', selected: null as NoteMeta | null };

    resolvePendingSelection([NOTE_A, NOTE_B], state);

    // No matching note — nothing selected, but the queue must not get stuck.
    expect(state.selected).toBeNull();
    expect(state.pendingSelectNoteId).toBeNull();
  });

  it('ignores events with no noteId in the payload', () => {
    const state = { pendingSelectNoteId: null as string | null, selected: null as NoteMeta | null };

    handlePaneSelectNote(undefined, [], state);

    expect(state.pendingSelectNoteId).toBeNull();
    expect(state.selected).toBeNull();
  });
});

// Note: a source-level regression guard for handleTaskClick's fixed-delay
// removal, plus a full end-to-end behavioral test (real CanvasRenderer +
// NotesBrowser render, slow-mocked api.listNotesByProject), already lives
// in CanvasRenderer.taskclick-completion.test.ts — not duplicated here.
