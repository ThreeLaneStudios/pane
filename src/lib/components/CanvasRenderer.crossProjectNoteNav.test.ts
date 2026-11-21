/**
 * CanvasRenderer.crossProjectNoteNav.test.ts — #3044 item 2.
 *
 * Matt's report: clicking a wiki link in a note that lives in "Symbiotic"
 * — one link to an existing note, one to a newly created one — opened both
 * under "Pane Development" instead.
 *
 * Proven cause: `CanvasRenderer.svelte`'s `loadDataForView` has a dedicated
 * branch for EVERY view that scopes the screen to a project — 'notes_browser'/
 * 'project_detail', 'folder_detail', 'note_list', 'task_list', and
 * 'project_config' all resolve `context.projectId` into the `$activeProject`
 * store. The `'note_editor'` branch — the one view wiki-link navigation
 * actually lands on (`noteService.selectNote()` sets
 * `canvasState.view = 'note_editor'`) — never did. `NotesBrowser.svelte`
 * (the SAME component CanvasRenderer.svelte:484 renders for 'notes_browser',
 * 'note_editor', AND 'note_list') reads `$activeProject` directly for its
 * folder header, folder list, and task-note filtering. So the note's
 * CONTENT loaded correctly (`get_note` is id-keyed, not project-scoped,
 * confirmed in `api.readNote`), but every surrounding chrome kept showing
 * whichever project was active before the click.
 *
 * This test drives `CanvasRenderer` through exactly the transition a
 * wiki-link click (or any other note_editor navigation) produces — starting
 * on Project A's board, then landing on a note that belongs to Project B —
 * and asserts both the underlying store AND the rendered DOM now reflect
 * Project B, not the stale Project A.
 *
 * RED at base: before the fix, `$activeProject` stays `{ id: 'project-a' }`
 * and the rendered folder header keeps reading "Project A".
 */

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import './_window-event-shim';
import { render, cleanup, screen } from '@testing-library/svelte/svelte5';
import { get } from 'svelte/store';
import CanvasRenderer from './CanvasRenderer.svelte';
import * as api from '$lib/api';
import { canvasState } from '$lib/stores/canvas';
import { activeProject, activeFolder, projects, activeNote } from '$lib/stores/app';
import type { NoteMeta, Project } from '$lib/types';

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

vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(() => Promise.resolve(() => {})),
}));

const PROJECT_A: Project = {
  id: 'project-a',
  name: 'Pane Development',
  parent_id: null,
  created_at: '',
  updated_at: '',
} as any;

const PROJECT_B: Project = {
  id: 'project-b',
  name: 'Symbiotic',
  parent_id: null,
  created_at: '',
  updated_at: '',
} as any;

const TARGET_NOTE_ID = 'note-in-symbiotic';

function makeTargetNoteMeta(): NoteMeta {
  return {
    id: TARGET_NOTE_ID,
    title: 'Existing Symbiotic Note',
    slug: TARGET_NOTE_ID,
    notebook: 'Symbiotic',
    preview: '',
    content_preview: '',
    created_at: '2026-01-01T00:00:00Z',
    updated_at: '2026-01-01T00:00:00Z',
    pinned: false,
    word_count: 0,
    tags: [],
    project_id: PROJECT_B.id,
    folder_id: undefined,
    note_type: undefined,
  } as any;
}

beforeEach(() => {
  canvasState.set({
    view: 'notes_browser',
    data: {},
    context: { projectId: PROJECT_A.id, projectName: PROJECT_A.name },
  } as any);
  activeProject.set(PROJECT_A);
  activeFolder.set(null);
  projects.set([PROJECT_A, PROJECT_B]);
  activeNote.set(null);

  vi.spyOn(api, 'listProjects').mockResolvedValue([PROJECT_A, PROJECT_B]);
  vi.spyOn(api, 'listFolders').mockResolvedValue([]);
  vi.spyOn(api, 'listNotesByProject').mockResolvedValue([]);
  vi.spyOn(api, 'listTasks').mockResolvedValue([]);
  vi.spyOn(api, 'getSheetCardSummaries').mockResolvedValue([]);
  vi.spyOn(api, 'getActiveSession').mockResolvedValue({ id: 'sess-xproj' } as any);
  vi.spyOn(api, 'getChatMessages').mockResolvedValue([]);
  vi.spyOn(api, 'readNote').mockResolvedValue({
    meta: makeTargetNoteMeta(),
    content: '',
  } as any);
  vi.spyOn(api, 'getNoteTaskFields').mockResolvedValue({
    title: null, task_status: null, task_priority: null, task_due_date: null, task_assignee: null,
  } as any);
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  canvasState.set({ view: 'home', data: {}, context: {} } as any);
  activeProject.set(null);
  activeFolder.set(null);
  projects.set([]);
  activeNote.set(null);
});

describe('#3044 item 2 — note_editor navigation to a different project updates $activeProject and the rendered chrome', () => {
  it('starts on Project A\'s board', async () => {
    render(CanvasRenderer, { props: {} });
    await screen.findByText('Pane Development');
    expect(get(activeProject)?.id).toBe(PROJECT_A.id);
  });

  it('navigating to a note_editor view for a note in Project B updates $activeProject to Project B, not Project A', async () => {
    render(CanvasRenderer, { props: {} });
    await screen.findByText('Pane Development');

    // Mirrors exactly what noteService.selectNote()'s canvasState.update()
    // call does for a wiki-link (or any) navigation to a note whose
    // project differs from whatever is currently active — the contextOverride
    // carries the TARGET note's own project_id.
    canvasState.set({
      view: 'note_editor',
      data: {},
      context: {
        projectId: PROJECT_B.id,
        folderId: undefined,
        noteId: TARGET_NOTE_ID,
        projectName: PROJECT_B.name,
      },
    } as any);

    await vi.waitFor(() => {
      expect(get(activeProject)?.id).toBe(PROJECT_B.id);
    });

    // RED-ON-REVERT: before the fix, this stays "Pane Development" — the
    // bug Matt reported ("both opened under Pane Development").
    await screen.findByText('Symbiotic');
    expect(screen.queryByText('Pane Development')).toBeNull();
  });
});
