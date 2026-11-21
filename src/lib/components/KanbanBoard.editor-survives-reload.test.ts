/**
 * KanbanBoard.editor-survives-reload.test.ts — #2951 (kanban editor flicker:
 * pane-data-changed -> loadTasks -> loading=true destroys/remounts Editor).
 *
 * Root cause (see docs/audits/2026-09-17-kanban-flicker-diagnosis.md, PR #2929
 * / commit d828977b): the entire `kanban-split-view` — including the
 * `{#if $activeNote}` block that renders `<Editor>` inside `{#key
 * note.meta.id}` — used to live inside the `{:else}` branch of `{#if
 * loading}`. `loadTasks()` sets `loading = true` at its top and `false` in
 * its `finally`, so every `pane-data-changed`-triggered reload (added by
 * PR #2929 for #2874 item 34's sheet-delete-refresh fix) unmounted and
 * remounted the whole split view, destroying and recreating the `<Editor>`
 * component even though `note.meta.id` never changed.
 *
 * The fix (#2951) moved the editor panel — and the `{#if loading}/{:else}`
 * gate itself — so `loading` only gates the kanban columns/card list inside
 * `.kanban-left-panel`, never the `.kanban-editor-panel` sibling.
 *
 * UPDATED FOR #2972 STAGE 2 (§34.10's "payload-shape migration" list names
 * this file explicitly): there is no `loadTasks()` reload left to survive
 * at all — ruling 5/§34.5 replaced it with per-card patch/insert/remove via
 * the Tauri-emitted channel. This test now proves the STRONGER,
 * post-migration invariant the #2951 fix was protecting in the first
 * place: patching the open note's own card (via a `pane-data-changed`
 * event for that note, simulated on the Tauri channel since backend emits
 * are invisible to window.addEventListener, #2215) neither reloads the
 * task list NOR touches the mounted `<Editor>` DOM node — while still
 * actually updating the card's data, proving the patch really ran.
 *
 * RED-ON-REVERT: reverting KanbanBoard.svelte's `handleDataChangedEvent` to
 * call `loadTasks()` (a full reload) instead of patching `tasks` in place
 * makes this test's "same Editor DOM node" assertion fail again, the same
 * way #2951's original regression did — `loading = true -> false` would
 * unmount/remount the `{#key note.meta.id}` block even though the note id
 * is unchanged.
 */

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, fireEvent, screen } from '@testing-library/svelte/svelte5';
import { get } from 'svelte/store';
import KanbanBoard from './KanbanBoard.svelte';
import { activeNote } from '$lib/stores/app';
import { canvasState } from '$lib/stores/canvas';
import { mockData, simulateTauriEvent } from '../../../vitest-setup';
import * as api from '$lib/api';
import { EVENT_DATA_CHANGED, ENTITY_NOTE, REASON_EDITED } from '$lib/refreshEvent';

// See KanbanBoard.sheet-identity.test.ts for the full rationale on each of
// these shims — vitest-setup.ts's `window` mock lacks all of them, but real
// TipTap/ProseMirror mounting (for a non-sheet note's Editor) needs them.
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
// vitest-setup.ts's `window` mock stubs `addEventListener`/`dispatchEvent`
// as no-ops — KanbanBoard still registers several unrelated window-level
// DOM listeners (`pane-new-task`, `pane-task-meta-changed`, `keydown`), so
// these must be rebound to the real globalThis EventTarget for mount to
// succeed (the `pane-data-changed` listener itself is now Tauri-channel-
// only, simulated below via `simulateTauriEvent`, not this rebinding).
win.addEventListener = globalThis.addEventListener.bind(globalThis);
win.removeEventListener = globalThis.removeEventListener.bind(globalThis);

const PROJECT_ID = 'project-editor-survives-reload';
const NOTE_ID = 'note-editor-survives-reload-1';

function seedPlainNote() {
  const now = new Date().toISOString();
  mockData.tasks.push({
    id: NOTE_ID,
    title: 'Survives Reload',
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
  mockData.noteContents[NOTE_ID] = 'hello world';
}

beforeEach(() => {
  mockData.tasks = [];
  mockData.nextId = 1;
  mockData.noteContents = {};
  mockData.projects = [
    { id: PROJECT_ID, name: 'Editor Survives Reload Project', parent_id: null,
      created_at: new Date().toISOString(), updated_at: new Date().toISOString() },
  ];
  canvasState.set({
    view: 'kanban',
    data: {},
    context: { projectId: PROJECT_ID, projectName: 'Editor Survives Reload Project' },
  } as any);
  activeNote.set(null);
});

afterEach(() => {
  vi.restoreAllMocks();
  activeNote.set(null);
});

describe('KanbanBoard — editor survives a pane-data-changed card patch (#2951, #2972 stage 2)', () => {
  it('patches the card title AND keeps the same Editor DOM node, with no listNotesForKanban() reload', async () => {
    seedPlainNote();
    const listSpy = vi.spyOn(api, 'listNotesForKanban');

    render(KanbanBoard, { props: {} });

    // Open the note from its Kanban card — mirrors handleNoteSelect's real
    // path (entityService.getNote -> $activeNote = transformedNote).
    const card = await screen.findByText('Survives Reload');
    await fireEvent.click(card);

    await vi.waitFor(() => {
      const note = get(activeNote);
      expect(note).toBeTruthy();
      expect(note!.meta.id).toBe(NOTE_ID);
    });

    // Editor mounted for this (non-sheet) note.
    const editorNodeBefore = await screen.findByTestId('note-editor');
    expect(editorNodeBefore.isConnected).toBe(true);

    const callsBefore = listSpy.mock.calls.length;

    // Simulate the backend emitting the new, entity-identified event for
    // this exact note (e.g. an agent edit while the note happens to be
    // open in this board's editor panel) — the Tauri channel, since
    // backend emits are invisible to window.addEventListener (#2215).
    // Mutate the mock backend's record first, same as a real agent edit
    // would, so the single-entity re-read KanbanBoard's handler does picks
    // up a REAL change (not a no-op fetch of unchanged data).
    const task = mockData.tasks.find((t: any) => t.id === NOTE_ID);
    if (task) task.title = 'Survives Reload — Patched';

    simulateTauriEvent(EVENT_DATA_CHANGED, {
      entity_id: NOTE_ID,
      entity_type: ENTITY_NOTE,
      reason: REASON_EDITED,
      origin_tag: 'test-origin-tag',
    });

    // Prove the patch actually ran — the card's title updates in the DOM.
    await vi.waitFor(() => {
      expect(screen.getByText('Survives Reload — Patched')).toBeTruthy();
    });

    // §34.5: patch only, never a second listNotesForKanban() reload.
    expect(listSpy.mock.calls.length).toBe(callsBefore);

    // Give any (bugged) destroy/remount cycle a tick to complete.
    await new Promise(r => setTimeout(r, 30));

    // The note is still the active one...
    const noteAfter = get(activeNote);
    expect(noteAfter?.meta.id).toBe(NOTE_ID);

    // ...and critically, the SAME Editor DOM node instance is still mounted
    // — not a new one created by a destroy/remount of the editor panel.
    const editorNodeAfter = screen.getByTestId('note-editor');
    expect(editorNodeAfter).toBe(editorNodeBefore);
    expect(editorNodeAfter.isConnected).toBe(true);
  });
});
