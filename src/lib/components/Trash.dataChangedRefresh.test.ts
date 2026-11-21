/**
 * Trash.dataChangedRefresh.test.ts — #2972 Stage 4 (`docs/spec/
 * data-refresh.md` §34, inventory + cross-screen verification pass).
 *
 * Finding (inventory, §34 Stage 4): `Trash.svelte` is a live, reachable
 * screen (navigated to via Sidebar's "Trash" item, `ChatSidebar`'s
 * `navigateTo({view: 'trash'})`, and `+page.svelte`'s keyboard-shortcut
 * handler) — not one of §34.12's original nine, and never migrated by any
 * prior #2972 stage. It had NO `pane-data-changed` listener of any kind
 * before this fix: a note moved to trash by an agent/background tool call
 * while this screen was already open never appeared until the user closed
 * and reopened it.
 *
 * Measured directly against pre-fix `Trash.svelte` (mount settles, spy
 * cleared, event dispatched, 100ms wait — the same harness as
 * `ProjectsView.categoryRefresh.test.ts`/`AllTasksView`'s own handler
 * tests):
 *
 *   note/deleted event -> listTrash calls: 0
 *
 * Zero calls — confirms the gap. This test fails (expect 1, get 0) against
 * that code.
 *
 * Fix: a listener scoped to `entity_type === 'note' && reason ===
 * 'deleted'` re-reads the trash list via the existing `loadTrash()`
 * (`api.listTrash()`) — there is no single-item trash read to patch one
 * row with, and the trash list is small/bounded (30-day retention), so a
 * full re-read is the same "tiny, no per-item read, deliberate" shape
 * Sidebar/ProjectsView already use for their `category` branch. Measured
 * against the fixed code with the identical harness:
 *
 *   note/deleted event -> listTrash calls: 1
 *
 * Negative case: a note/edited event (not a delete) must NOT trigger a
 * reload — confirms the handler is reason-scoped, not entity-type-only.
 */

import { describe, it, expect, vi, afterEach } from 'vitest';
import { tick } from 'svelte';
import { render, cleanup, screen } from '@testing-library/svelte/svelte5';
import Trash from './Trash.svelte';
import * as api from '$lib/api';
import { simulateTauriEvent } from '../../../vitest-setup';
import {
  EVENT_DATA_CHANGED,
  ENTITY_NOTE,
  ENTITY_PROJECT,
  REASON_DELETED,
  REASON_EDITED,
} from '$lib/refreshEvent';
import type { TrashItem } from '$lib/types';

const win = globalThis.window as any;
if (typeof win.addEventListener !== 'function') {
  win.addEventListener = globalThis.addEventListener?.bind(globalThis) ?? (() => {});
}
if (typeof win.removeEventListener !== 'function') {
  win.removeEventListener = globalThis.removeEventListener?.bind(globalThis) ?? (() => {});
}
win.dispatchEvent = globalThis.dispatchEvent.bind(globalThis);

const TRASH_ITEM: TrashItem = {
  id: 'trash-note-1',
  title: 'Already Trashed Note',
  notebook: 'Notes',
  slug: 'already-trashed-note',
  deleted_at: '2026-10-01T00:00:00Z',
  original_path: 'Notes/already-trashed-note.md',
};

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

async function renderTrashSettled() {
  const listTrashSpy = vi.spyOn(api, 'listTrash').mockResolvedValue([TRASH_ITEM]);

  render(Trash, { props: { onClose: () => {} } });
  await screen.findByText('Already Trashed Note');
  await new Promise((r) => setTimeout(r, 100));
  await tick();
  listTrashSpy.mockClear();

  return { listTrashSpy };
}

describe('Trash pane-data-changed handler (#2972 stage 4)', () => {
  it('a note/deleted event re-reads the trash list exactly once', async () => {
    const { listTrashSpy } = await renderTrashSettled();

    simulateTauriEvent(EVENT_DATA_CHANGED, {
      entity_id: 'note-newly-trashed',
      entity_type: ENTITY_NOTE,
      reason: REASON_DELETED,
      origin_tag: 'trash-test-note-deleted',
    });

    await vi.waitFor(() => expect(listTrashSpy).toHaveBeenCalledTimes(1));
  });

  it('a note/edited event does NOT reload the trash list', async () => {
    const { listTrashSpy } = await renderTrashSettled();

    simulateTauriEvent(EVENT_DATA_CHANGED, {
      entity_id: 'note-edited-elsewhere',
      entity_type: ENTITY_NOTE,
      reason: REASON_EDITED,
      origin_tag: 'trash-test-note-edited',
    });

    await new Promise((r) => setTimeout(r, 100));
    expect(listTrashSpy).toHaveBeenCalledTimes(0);
  });

  it('a project event does NOT reload the trash list', async () => {
    const { listTrashSpy } = await renderTrashSettled();

    simulateTauriEvent(EVENT_DATA_CHANGED, {
      entity_id: 'proj-1',
      entity_type: ENTITY_PROJECT,
      reason: REASON_DELETED,
      origin_tag: 'trash-test-project-deleted',
    });

    await new Promise((r) => setTimeout(r, 100));
    expect(listTrashSpy).toHaveBeenCalledTimes(0);
  });
});
