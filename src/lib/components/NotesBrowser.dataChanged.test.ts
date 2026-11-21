/**
 * NotesBrowser.dataChanged.test.ts — #2972 stage 2 (`docs/spec/
 * data-refresh.md` §34.5): NotesBrowser's `dataHandler` (`loadProjectData()`,
 * DOM `pane-data-changed`/`pane-note-created` full-reload listeners) and
 * its Tauri `listen('pane-note-changed', ...)` registration (dead since
 * #2972 stage 1 migrated that emit site onto the single `pane-data-changed`
 * event) both collapse into one handler on the new event — patching the
 * affected row in the cabinet/folder list in place, never a full
 * `loadProjectData()` reload.
 */

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, cleanup, screen } from '@testing-library/svelte/svelte5';
import NotesBrowser from './NotesBrowser.svelte';
import * as api from '$lib/api';
import { activeProject, projects, activeNote, activeFolder } from '$lib/stores/app';
import { simulateTauriEvent } from '../../../vitest-setup';
import { EVENT_DATA_CHANGED, ENTITY_NOTE, ENTITY_FOLDER, REASON_CREATED, REASON_DELETED, dispatchDataChanged } from '$lib/refreshEvent';
import type { NoteMeta, NoteContent } from '$lib/types';

const win = globalThis.window as any;
// #2972 stage 3 (item 4): these three are now REAL bindings, not no-ops —
// Stage 2 only needed addEventListener/removeEventListener as harmless
// no-ops (every Stage 2 listener here used the Tauri `listen()` channel,
// simulated via `simulateTauriEvent`, never the DOM channel). Stage 3's
// new app-channel test below needs a dispatch on `window` to actually
// reach a `window.addEventListener` registration, so all three must be
// the real jsdom EventTarget methods.
win.addEventListener = globalThis.addEventListener.bind(globalThis);
win.removeEventListener = globalThis.removeEventListener.bind(globalThis);
win.dispatchEvent = globalThis.dispatchEvent.bind(globalThis);
if (typeof (globalThis as any).ResizeObserver !== 'function') {
  (globalThis as any).ResizeObserver = class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
}

const PROJECT = { id: 'project-nb-datachanged-1', name: 'Project One', parent_id: null, created_at: '', updated_at: '' } as any;

function makeNote(overrides: Partial<NoteMeta> = {}): NoteMeta {
  return {
    id: 'note-x',
    title: 'Note X',
    notebook: '',
    slug: 'note-x',
    preview: '',
    created_at: '2026-01-01T00:00:00Z',
    updated_at: '2026-01-01T00:00:00Z',
    project_id: PROJECT.id,
    ...overrides,
  } as NoteMeta;
}

beforeEach(() => {
  activeProject.set(PROJECT);
  activeFolder.set(null as any);
  projects.set([PROJECT]);
  activeNote.set(null);
  vi.spyOn(api, 'listProjects').mockResolvedValue([PROJECT]);
  vi.spyOn(api, 'listFolders').mockResolvedValue([]);
  vi.spyOn(api, 'listNotesByProject').mockResolvedValue([]);
  vi.spyOn(api, 'getSheetCardSummaries').mockResolvedValue([]);
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  activeProject.set(null as any);
  activeFolder.set(null as any);
});

describe('NotesBrowser pane-data-changed handler (#2972 stage 2, §34.5)', () => {
  it('inserts a newly-created note into the cabinet list without a loadProjectData() reload', async () => {
    const listNotesSpy = vi.spyOn(api, 'listNotesByProject').mockResolvedValue([]);
    render(NotesBrowser, { props: {} });

    // NotesBrowser mounts with TWO independent $effects that each call
    // loadTaskNotes()/loadProjectData() (unrelated pre-existing behavior,
    // not touched by #2972) — wait for both to settle before capturing a
    // stable "before" call count, so neither races the event below.
    await vi.waitFor(() => expect(listNotesSpy.mock.calls.length).toBeGreaterThanOrEqual(2));
    await new Promise(r => setTimeout(r, 20));
    const callsBefore = listNotesSpy.mock.calls.length;
    expect(screen.queryByText('Agent Note')).toBeNull();

    vi.spyOn(api, 'readNote').mockResolvedValue({
      meta: makeNote({ id: 'note-agent-1', title: 'Agent Note' }),
      content: '',
    } as NoteContent);

    simulateTauriEvent(EVENT_DATA_CHANGED, {
      entity_id: 'note-agent-1',
      entity_type: ENTITY_NOTE,
      reason: REASON_CREATED,
      origin_tag: 'test-origin-tag',
    });

    await screen.findByText('Agent Note');
    expect(listNotesSpy.mock.calls.length).toBe(callsBefore);
  });

  it('removes a note reported deleted', async () => {
    const existing = makeNote({ id: 'note-doomed-1', title: 'Doomed Note' });
    const listNotesSpy = vi.spyOn(api, 'listNotesByProject').mockResolvedValue([existing]);

    render(NotesBrowser, { props: {} });
    await screen.findByText('Doomed Note');
    // Let BOTH of the mount's independent loadTaskNotes() calls settle
    // (see insert test's comment above) before firing the delete event, so
    // a late-resolving second call doesn't re-set `taskNotes` back to the
    // full (pre-delete) list out from under this assertion.
    await vi.waitFor(() => expect(listNotesSpy.mock.calls.length).toBeGreaterThanOrEqual(2));
    await new Promise(r => setTimeout(r, 20));

    simulateTauriEvent(EVENT_DATA_CHANGED, {
      entity_id: 'note-doomed-1',
      entity_type: ENTITY_NOTE,
      reason: REASON_DELETED,
      origin_tag: 'test-origin-tag',
    });

    await vi.waitFor(() => {
      expect(screen.queryByText('Doomed Note')).toBeNull();
    });
  });

  it('patches in the ONE new folder for a folder-created event, without a listFolders() list refetch (#2972 stage 2 WARN closure, PR #3016)', async () => {
    vi.spyOn(api, 'listNotesByProject').mockResolvedValue([]);
    // #2972 stage 2 WARN closure (Reed's 2026-10-11 audit, Matt's sign-off
    // "fix before merge"): this handler used to re-fetch the whole folder
    // list (`api.listFolders($activeProject.id)`) on every folder event.
    // It now patches the ONE changed folder in via a new single-entity read
    // (`api.getProjectOrFolder()`) — `listFoldersSpy` must stay at its
    // mount-time count of 1, never climb to 2.
    const listFoldersSpy = vi.spyOn(api, 'listFolders').mockResolvedValueOnce([]); // initial mount only
    const getOneSpy = vi.spyOn(api, 'getProjectOrFolder').mockResolvedValue({
      id: 'folder-agent-1',
      name: 'Agent Folder',
      parent_id: PROJECT.id,
      created_at: '',
      updated_at: '',
    } as any);

    render(NotesBrowser, { props: {} });
    await vi.waitFor(() => expect(listFoldersSpy).toHaveBeenCalledTimes(1));

    simulateTauriEvent(EVENT_DATA_CHANGED, {
      entity_id: 'folder-agent-1',
      entity_type: ENTITY_FOLDER,
      reason: REASON_CREATED,
      origin_tag: 'test-origin-tag',
    });

    // The single read fires on the event...
    await vi.waitFor(() => {
      expect(getOneSpy).toHaveBeenCalledTimes(1);
    });
    expect(getOneSpy).toHaveBeenCalledWith('folder-agent-1');
    // ...and the list fetch does NOT — stays at its mount-time count.
    expect(listFoldersSpy).toHaveBeenCalledTimes(1);
  });

  it('removes a folder reported deleted, with no read at all', async () => {
    vi.spyOn(api, 'listNotesByProject').mockResolvedValue([]);
    vi.spyOn(api, 'listFolders').mockResolvedValueOnce([
      { id: 'folder-doomed-1', name: 'Doomed Folder', parent_id: PROJECT.id, created_at: '', updated_at: '' } as any,
    ]);
    const getOneSpy = vi.spyOn(api, 'getProjectOrFolder');

    render(NotesBrowser, { props: {} });
    await vi.waitFor(() => expect(vi.mocked(api.listFolders).mock.calls.length).toBeGreaterThanOrEqual(1));

    simulateTauriEvent(EVENT_DATA_CHANGED, {
      entity_id: 'folder-doomed-1',
      entity_type: ENTITY_FOLDER,
      reason: REASON_DELETED,
      origin_tag: 'test-origin-tag',
    });

    // A delete removes it locally with no read at all.
    await new Promise(r => setTimeout(r, 20));
    expect(getOneSpy).not.toHaveBeenCalled();
  });

  it('inserts a newly-created note the SAME way via the APP channel (DOM CustomEvent), not just the backend Tauri channel (#2972 stage 3, item 4)', async () => {
    const listNotesSpy = vi.spyOn(api, 'listNotesByProject').mockResolvedValue([]);
    render(NotesBrowser, { props: {} });

    await vi.waitFor(() => expect(listNotesSpy.mock.calls.length).toBeGreaterThanOrEqual(2));
    await new Promise(r => setTimeout(r, 20));
    expect(screen.queryByText('App Channel Note')).toBeNull();

    vi.spyOn(api, 'readNote').mockResolvedValue({
      meta: makeNote({ id: 'note-app-channel-1', title: 'App Channel Note' }),
      content: '',
    } as NoteContent);

    dispatchDataChanged('note-app-channel-1', ENTITY_NOTE, REASON_CREATED, 'test-origin-tag-app');

    await screen.findByText('App Channel Note');
  });
});
