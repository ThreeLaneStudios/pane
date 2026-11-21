/**
 * CanvasRenderer.agentOpenNote-unmounted-note-race.test.ts — #3044
 * delta-2 (Reed), item 4: "the agent opening a note".
 *
 * `ChatSidebar.svelte`'s `handleNavigateFromStream` `open_note` case is
 * the backend agent's `navigate({note_id, project_id})` tool landing in
 * the UI. Unlike `createNewNote()`/`selectNote()`, this case does NOT set
 * `$activeNote` and write `canvasState.view` back-to-back with no `await`
 * between them — it calls `navigateTo({view: 'notes_browser', ...})`
 * FIRST (mounting `NotesBrowser`, if it wasn't already), then, 500ms
 * later (`setTimeout`), fetches the note via `invoke('get_note', ...)`
 * and only THEN sets `$activeNote` — by which point `NotesBrowser`'s
 * onMount has long since run.
 *
 * This file reproduces that exact sequence (same two calls, same 500ms
 * gap `ChatSidebar.svelte` itself uses — not a mocked shortcut) against a
 * real `CanvasRenderer`, starting on a screen where `NotesBrowser` has
 * never been mounted, to check whether this path shares the race the
 * other three (Cmd+N, wiki-link-from-AllTasks, wiki-link-from-Kanban) do.
 *
 * Per the task brief ("each shown failing before the change; if one
 * passes at base, say so honestly"): this one passes at base. Recorded
 * here as a regression guard — the sequencing that makes it already safe
 * (navigate-and-mount BEFORE fetch-and-set, not set-then-navigate) must
 * not change, and the shared `clearStaleActiveNoteOnMount` guard must not
 * break it either.
 */

import './_window-event-shim';
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, cleanup, screen } from '@testing-library/svelte/svelte5';
import { tick } from 'svelte';
import { invoke } from '@tauri-apps/api/core';
import { get } from 'svelte/store';
import CanvasRenderer from './CanvasRenderer.svelte';
import * as api from '$lib/api';
import { navigateTo } from '$lib/services/navigation';
import { canvasState } from '$lib/stores/canvas';
import { activeProject, projects, activeNote } from '$lib/stores/app';
import { mockData } from '../../../vitest-setup';
import type { Project } from '$lib/types';

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

const PROJECT_ID = 'project-agent-open-note';
const NOTE_ID = 'agent-open-note-target';

function makeProject(): Project {
  return {
    id: PROJECT_ID,
    name: 'Agent Project',
    parent_id: null,
    created_at: '',
    updated_at: '',
  } as any;
}

/**
 * Mirrors ChatSidebar.svelte's `case 'open_note'` body exactly — same two
 * calls (`navigateTo` then, 500ms later, `invoke('get_note', ...)`), same
 * gap, same order of `$activeNote` write vs. the dispatched event.
 */
async function agentOpenNote() {
  await navigateTo({
    view: 'notes_browser',
    projectId: PROJECT_ID,
    noteId: NOTE_ID,
    folderId: undefined,
  });

  await new Promise<void>((resolve) => {
    setTimeout(async () => {
      const noteData: any = await invoke('get_note', { noteId: NOTE_ID });
      if (noteData) {
        const nd = {
          meta: { ...noteData.meta, slug: noteData.meta.id, notebook: 'Agent Project' },
          content: noteData.content,
        };
        activeNote.set(nd);
        window.dispatchEvent(new CustomEvent('pane-select-note', { detail: { noteId: NOTE_ID } }));
      }
      resolve();
    }, 500);
  });
}

beforeEach(() => {
  mockData.tasks = [
    {
      id: NOTE_ID,
      title: 'Agent-Opened Note',
      content: 'Note body set up by the agent tool call',
      status: 'cabinet',
      priority: 'medium',
      project_id: PROJECT_ID,
      folder_id: null,
      tags: [],
      created_at: new Date().toISOString(),
      updated_at: new Date().toISOString(),
    },
  ];
  mockData.noteContents[NOTE_ID] = 'Note body set up by the agent tool call';
  mockData.projects = [makeProject()];

  // Start on Calendar — NotesBrowser has never been mounted this session.
  canvasState.set({ view: 'calendar', data: {}, context: {} } as any);
  activeProject.set(null);
  projects.set([makeProject()]);
  activeNote.set(null);

  vi.spyOn(api, 'listProjects').mockResolvedValue([makeProject()]);
  vi.spyOn(api, 'listTasks').mockResolvedValue([]);
  vi.spyOn(api, 'listFolders').mockResolvedValue([]);
  vi.spyOn(api, 'listNotesByProject').mockResolvedValue([]);
  vi.spyOn(api, 'getSheetCardSummaries').mockResolvedValue([]);
  vi.spyOn(api, 'getActiveSession').mockResolvedValue({ id: 'sess-agent-open-note' } as any);
  vi.spyOn(api, 'getChatMessages').mockResolvedValue([]);
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  canvasState.set({ view: 'home', data: {}, context: {} } as any);
  activeProject.set(null);
  projects.set([]);
  activeNote.set(null);
});

describe('#3044 delta-2 — the agent opening a note (NotesBrowser genuinely unmounted) — passes at base', () => {
  it('navigates, mounts NotesBrowser, then sets $activeNote 500ms later — the note is open and correct', async () => {
    render(CanvasRenderer, { props: {} });
    await flushAsync();

    expect(screen.queryByTestId('note-title-input')).toBeNull();

    await agentOpenNote();
    await flushAsync();

    expect(get(activeNote)?.meta?.id).toBe(NOTE_ID);

    const titleInput = await screen.findByTestId('note-title-input') as HTMLInputElement;
    expect(titleInput.value).toBe('Agent-Opened Note');
  }, 10000);
});
