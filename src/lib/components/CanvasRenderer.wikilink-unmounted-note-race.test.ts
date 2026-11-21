/**
 * CanvasRenderer.wikilink-unmounted-note-race.test.ts — #3044 delta-2
 * (Reed), item 4: "a wiki link from All Tasks" + an additional instance of
 * the same mechanism found while auditing every `navigateWikiLink` caller
 * — "a wiki link from Kanban's own note editor panel".
 *
 * `navigateWikiLink()` (`wikiLinkNavigation.ts`) resolves an exact title
 * match and calls `selectNoteFn` — every real caller passes
 * `noteService.selectNote` directly. `selectNote()` does `activeNote.set
 * (enriched)` immediately followed by `canvasState.update(... view:
 * 'note_editor' ...)`, no `await` between the two — exactly
 * `createNewNote()`'s shape. Clicking a wiki link from `AllTasksView` or
 * from `KanbanBoard`'s note-editor panel calls this while `NotesBrowser`
 * is NOT mounted (both are separate, mutually-exclusive branches of
 * `CanvasRenderer.svelte`'s `{:else if}` chain) — so the exact same
 * mount-wipes-the-note race applies.
 *
 * These tests call `navigateWikiLink` for real (not a mocked
 * `selectNoteFn`) with `CanvasRenderer` genuinely mounted on `all_tasks`/
 * `kanban` — not `notes_browser` — so `NotesBrowser` mounts for the first
 * time in response, exactly reproducing what a real DOM click on a
 * `[[Title]]` wiki link inside either screen's embedded `Editor.svelte`
 * would trigger.
 */

import './_window-event-shim';
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, cleanup, screen } from '@testing-library/svelte/svelte5';
import { tick } from 'svelte';
import { get } from 'svelte/store';
import CanvasRenderer from './CanvasRenderer.svelte';
import * as noteService from '$lib/services/noteService';
import * as api from '$lib/api';
import { navigateWikiLink } from '$lib/utils/wikiLinkNavigation';
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
  id: 'proj-wikilink-race',
  name: 'Symbiotic',
  parent_id: null,
  created_at: '',
  updated_at: '',
} as any;

const TARGET_NOTE_ID = 'wikilink-race-target-note';

function makeTargetMeta(): NoteMeta {
  return {
    id: TARGET_NOTE_ID,
    title: 'Existing Target Note',
    slug: TARGET_NOTE_ID,
    notebook: 'Symbiotic',
    preview: '',
    content_preview: '',
    created_at: '2026-01-01T00:00:00Z',
    updated_at: '2026-01-01T00:00:00Z',
    pinned: false,
    word_count: 0,
    tags: [],
    project_id: PROJECT.id,
    folder_id: undefined,
    note_type: undefined,
  } as any;
}

// CanvasRenderer.svelte's OWN #2335 reload-guard $effect notices
// `canvasState.context.noteId` no longer matches `$activeNote` (true the
// instant NotesBrowser's pre-fix onMount wipes it) and re-fetches the note
// via `api.readNote` as an UNRELATED recovery path — eventually masking
// this exact race in a real render, at the cost of a redundant reload (and,
// in the real app, discarding anything typed in the gap). That recovery is
// real but not this fix, and its timing is not deterministic enough to
// assert against either way. `readNote`'s FIRST call is `selectNote()`'s
// own, legitimate read (must resolve normally); any call AFTER that is the
// self-heal effect's own separate re-fetch attempt — made to hang forever,
// so the critical assertions below test ONLY the mount-time behavior
// `clearStaleActiveNoteOnMount` governs, not whether an unrelated recovery
// path happened to also run fast enough.
let readNoteCalls = 0;

function wireWikiLinkMocks() {
  readNoteCalls = 0;
  vi.spyOn(api, 'searchNotes').mockResolvedValue([makeTargetMeta()]);
  vi.spyOn(api, 'getNoteRaw').mockResolvedValue({ meta: makeTargetMeta() } as any);
  vi.spyOn(api, 'readNote').mockImplementation(async () => {
    readNoteCalls += 1;
    if (readNoteCalls > 1) return new Promise<any>(() => {}); // never resolves
    return {
      meta: makeTargetMeta(),
      content: 'Existing saved body text',
    } as any;
  });
  vi.spyOn(api, 'getNoteTaskFields').mockResolvedValue({
    title: null, task_status: null, task_priority: null, task_due_date: null, task_assignee: null,
  } as any);
}

async function clickWikiLinkFor(title: string) {
  // Exactly what AllTasksView.handleAllTasksWikiLinkNavigation /
  // KanbanBoard.handleKanbanWikiLinkNavigation call — the real function,
  // the real noteService.selectNote, nothing mocked in between.
  await navigateWikiLink(title, {
    searchFn: (q) => api.searchNotes(q),
    selectNoteFn: (note, contextOverride) => noteService.selectNote(note, contextOverride),
    getNoteRawFn: async (id) => {
      const raw: any = await api.getNoteRaw(id);
      return raw?.meta ?? null;
    },
  });
}

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  canvasState.set({ view: 'home', data: {}, context: {} } as any);
  activeProject.set(null);
  projects.set([]);
  activeNote.set(null);
});

describe('#3044 delta-2 — wiki-link click from All Tasks (NotesBrowser genuinely unmounted)', () => {
  beforeEach(() => {
    canvasState.set({ view: 'all_tasks', data: {}, context: {} } as any);
    activeProject.set(null);
    projects.set([PROJECT]);
    activeNote.set(null);

    wireWikiLinkMocks();
    vi.spyOn(api, 'listTaskNotes').mockResolvedValue([]);
    vi.spyOn(api, 'getSubtaskCounts').mockResolvedValue({});
    vi.spyOn(api, 'getSheetCardSummaries').mockResolvedValue([]);
    vi.spyOn(api, 'listProjects').mockResolvedValue([PROJECT]);
  });

  it('opens the existing note and $activeNote survives NotesBrowser mounting for the first time', async () => {
    render(CanvasRenderer, { props: {} });
    await flushAsync();

    expect(screen.queryByTestId('note-title-input')).toBeNull();

    await clickWikiLinkFor('Existing Target Note');
    await flushAsync();

    // Red at base: NotesBrowser's onMount, firing for the first time in
    // response to selectNote()'s canvasState write above, used to wipe
    // this synchronously-just-set note before anyone could observe it.
    expect(get(activeNote)?.meta?.id).toBe(TARGET_NOTE_ID);

    const titleInput = await screen.findByTestId('note-title-input') as HTMLInputElement;
    expect(titleInput.value).toBe('Existing Target Note');
    expect(document.querySelector('.ProseMirror')?.textContent).toContain('Existing saved body text');
  });
});

describe('#3044 delta-2 — wiki-link click from Kanban\'s own note editor panel (NotesBrowser genuinely unmounted) — additional instance found during the audit', () => {
  beforeEach(() => {
    canvasState.set({
      view: 'kanban',
      data: {},
      context: { projectId: PROJECT.id, projectName: PROJECT.name },
    } as any);
    activeProject.set(PROJECT);
    projects.set([PROJECT]);
    activeNote.set(null);

    wireWikiLinkMocks();
    vi.spyOn(api, 'listNotesForKanban').mockResolvedValue([]);
    vi.spyOn(api, 'listFolders').mockResolvedValue([]);
    vi.spyOn(api, 'listProjects').mockResolvedValue([PROJECT]);
    vi.spyOn(api, 'getActiveSession').mockResolvedValue({ id: 'sess-wikilink-kanban' } as any);
    vi.spyOn(api, 'getChatMessages').mockResolvedValue([]);
  });

  it('opens the existing note and $activeNote survives NotesBrowser mounting for the first time', async () => {
    render(CanvasRenderer, { props: {} });
    await flushAsync();

    expect(screen.queryByTestId('note-title-input')).toBeNull();

    await clickWikiLinkFor('Existing Target Note');
    await flushAsync();

    expect(get(activeNote)?.meta?.id).toBe(TARGET_NOTE_ID);

    const titleInput = await screen.findByTestId('note-title-input') as HTMLInputElement;
    expect(titleInput.value).toBe('Existing Target Note');
  });
});
