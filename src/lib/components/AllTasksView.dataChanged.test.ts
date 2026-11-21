/**
 * AllTasksView.dataChanged.test.ts — #2972 stage 2 (`docs/spec/
 * data-refresh.md` §34.5): AllTasksView's `dataHandler` (`loadData(true)`,
 * DOM `pane-data-changed`/`pane-note-created` full-reload listeners) and
 * its Tauri `listen('pane-note-changed', ...)` registration (dead since
 * #2972 stage 1 migrated that emit site onto the single `pane-data-changed`
 * event) both collapse into one handler on the new event — patching,
 * inserting, or removing exactly one row in `allTaskNotes`, never a full
 * `loadData()` reload.
 */

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, cleanup, screen } from '@testing-library/svelte/svelte5';
import AllTasksView from './AllTasksView.svelte';
import * as api from '$lib/api';
import * as noteService from '$lib/services/noteService';
import { simulateTauriEvent } from '../../../vitest-setup';
import { EVENT_DATA_CHANGED, ENTITY_NOTE, REASON_CREATED, REASON_DELETED, dispatchDataChanged } from '$lib/refreshEvent';
import type { NoteMeta, NoteContent } from '$lib/types';

const win = globalThis.window as any;
if (typeof win.addEventListener !== 'function') {
  win.addEventListener = globalThis.addEventListener?.bind(globalThis) ?? (() => {});
}
if (typeof win.removeEventListener !== 'function') {
  win.removeEventListener = globalThis.removeEventListener?.bind(globalThis) ?? (() => {});
}
// #2972 stage 3 (item 4): unconditional — vitest-setup.ts's fake `window`
// already defines a `dispatchEvent` property (a no-op stub); a
// conditional guard would see that as "already present" and never wire it
// to the real jsdom EventTarget.
win.dispatchEvent = globalThis.dispatchEvent.bind(globalThis);
if (typeof (globalThis as any).ResizeObserver !== 'function') {
  (globalThis as any).ResizeObserver = class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
}

function makeNote(overrides: Partial<NoteMeta> = {}): NoteMeta {
  return {
    id: 'note-x',
    title: 'Note X',
    notebook: '',
    slug: 'note-x',
    preview: '',
    created_at: '2026-01-01T00:00:00Z',
    updated_at: '2026-01-01T00:00:00Z',
    task_status: 'todo',
    ...overrides,
  } as NoteMeta;
}

beforeEach(() => {
  vi.spyOn(api, 'getSubtaskCounts').mockResolvedValue({});
  vi.spyOn(api, 'getSheetCardSummaries').mockResolvedValue([]);
  vi.spyOn(api, 'listProjects').mockResolvedValue([]);
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe('AllTasksView pane-data-changed handler (#2972 stage 2, §34.5)', () => {
  it('inserts a newly-created task without a loadData() reload', async () => {
    const listTasksSpy = vi.spyOn(noteService, 'listTasks').mockResolvedValue([]);
    render(AllTasksView, { props: {} });

    await vi.waitFor(() => expect(listTasksSpy).toHaveBeenCalledTimes(1));
    expect(screen.queryByText('Agent Task')).toBeNull();

    vi.spyOn(api, 'readNote').mockResolvedValue({
      meta: makeNote({ id: 'note-agent-1', title: 'Agent Task', task_status: 'todo' }),
      content: '',
    } as NoteContent);

    simulateTauriEvent(EVENT_DATA_CHANGED, {
      entity_id: 'note-agent-1',
      entity_type: ENTITY_NOTE,
      reason: REASON_CREATED,
      origin_tag: 'test-origin-tag',
    });

    await screen.findByText('Agent Task');
    expect(listTasksSpy).toHaveBeenCalledTimes(1);
  });

  it('removes a task reported deleted', async () => {
    const existing = makeNote({ id: 'note-doomed-1', title: 'Doomed Task', task_status: 'todo' });
    vi.spyOn(noteService, 'listTasks').mockResolvedValue([existing]);

    render(AllTasksView, { props: {} });
    await screen.findByText('Doomed Task');

    simulateTauriEvent(EVENT_DATA_CHANGED, {
      entity_id: 'note-doomed-1',
      entity_type: ENTITY_NOTE,
      reason: REASON_DELETED,
      origin_tag: 'test-origin-tag',
    });

    await vi.waitFor(() => {
      expect(screen.queryByText('Doomed Task')).toBeNull();
    });
  });

  it('inserts a newly-created task the SAME way via the APP channel (DOM CustomEvent), not just the backend Tauri channel (#2972 stage 3, item 4)', async () => {
    const listTasksSpy = vi.spyOn(noteService, 'listTasks').mockResolvedValue([]);
    render(AllTasksView, { props: {} });

    await vi.waitFor(() => expect(listTasksSpy).toHaveBeenCalledTimes(1));
    expect(screen.queryByText('App Channel Task')).toBeNull();

    vi.spyOn(api, 'readNote').mockResolvedValue({
      meta: makeNote({ id: 'note-app-channel-1', title: 'App Channel Task', task_status: 'todo' }),
      content: '',
    } as NoteContent);

    dispatchDataChanged('note-app-channel-1', ENTITY_NOTE, REASON_CREATED, 'test-origin-tag-app');

    await screen.findByText('App Channel Task');
  });
});
