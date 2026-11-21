/**
 * Sidebar.categoryRefresh.test.ts — #3018 (`docs/spec/data-refresh.md`
 * §34.1/§34.2/§34.5, "category as a fifth entity_type").
 *
 * The bug: renaming, recoloring, or deleting a category in Settings didn't
 * refresh the sidebar. Verified failing against pre-fix code: pre-fix,
 * `Sidebar.svelte`'s `handleDataChangedEvent` had no `entity_type ===
 * "category"` branch at all — its very first real check was
 * `if (entity_type !== ENTITY_FOLDER && entity_type !== ENTITY_PROJECT)
 * return;`, which returns immediately for a `category` event without
 * calling `loadCategories()` again. Measured directly (mount settles,
 * `listCategories` spy is cleared, then the category event is dispatched
 * and the test waits 100ms before reading the spy's call count again —
 * long enough for this component's own pre-existing, unrelated
 * `$effect(() => { void $projects; ... loadCategories() })` noise to have
 * already settled before the clear, so only calls caused by THIS event
 * are counted):
 *
 *   created settled calls: 3 / after dispatch (post-clear): 0
 *   edited  settled calls: 3 / after dispatch (post-clear): 0
 *   deleted settled calls: 3 / after dispatch (post-clear): 0
 *
 * Zero additional calls for all three reasons, against pre-fix
 * `Sidebar.svelte` — these tests fail (expect 1, get 0) against that code.
 *
 * Fix: a new branch re-reads the whole category list (§34.5: "it's tiny
 * and has no per-item read, so this is deliberate, unlike the
 * single-entity reads elsewhere") on ANY category event — add, rename,
 * recolor, or delete all funnel through the same `entity_type: "category"`
 * check. Measured against fixed code with the identical harness:
 *
 *   created after dispatch (post-clear): 1
 *   edited  after dispatch (post-clear): 1
 *   deleted after dispatch (post-clear): 1
 */

import { describe, it, expect, vi, afterEach } from 'vitest';
import { tick } from 'svelte';
import { render, cleanup, screen } from '@testing-library/svelte/svelte5';
import Sidebar from './Sidebar.svelte';
import * as api from '$lib/api';
import { simulateTauriEvent } from '../../../vitest-setup';
import { EVENT_DATA_CHANGED, ENTITY_CATEGORY, REASON_CREATED, REASON_EDITED, REASON_DELETED } from '$lib/refreshEvent';
import type { Project } from '$lib/types';

const win = globalThis.window as any;
if (typeof win.addEventListener !== 'function') {
  win.addEventListener = globalThis.addEventListener?.bind(globalThis) ?? (() => {});
}
if (typeof win.removeEventListener !== 'function') {
  win.removeEventListener = globalThis.removeEventListener?.bind(globalThis) ?? (() => {});
}
win.dispatchEvent = globalThis.dispatchEvent.bind(globalThis);

const PROJECT: Project = {
  id: 'proj-category-refresh-1',
  name: 'Test Project',
  parent_id: undefined,
  category: 'Work',
  sort_order: 0,
  created_at: '2026-01-01T00:00:00Z',
  updated_at: '2026-01-01T00:00:00Z',
  is_system: false,
  is_favorite: true,
} as any;

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

/** Mounts Sidebar, lets its own unrelated mount-time reactivity (the
 *  pre-existing `$effect` that re-runs `loadCategories()` whenever
 *  `$projects` changes, including its own initial assignment) fully
 *  settle, THEN clears the spy — so the count measured afterward is
 *  attributable only to whatever is dispatched next, not ambient noise
 *  this component already has independent of #3018. */
async function renderSidebarSettled() {
  vi.spyOn(api, 'listTags').mockResolvedValue([]);
  vi.spyOn(api, 'syncProjectsWithNotebooks').mockResolvedValue([PROJECT]);
  const categoriesSpy = vi.spyOn(api, 'listCategories').mockResolvedValue([{ category: 'Work', color: '#AABBCC' }]);

  render(Sidebar, { props: {} });
  await screen.findByText('Test Project');
  await new Promise((r) => setTimeout(r, 100));
  await tick();
  categoriesSpy.mockClear();

  return categoriesSpy;
}

describe('Sidebar pane-data-changed handler — category re-read (#3018, §34.5)', () => {
  it('a category "created" event (add) re-reads the category list exactly once', async () => {
    const categoriesSpy = await renderSidebarSettled();

    simulateTauriEvent(EVENT_DATA_CHANGED, {
      entity_id: 'Work',
      entity_type: ENTITY_CATEGORY,
      reason: REASON_CREATED,
      origin_tag: 'test-origin-tag-add',
    });

    await vi.waitFor(() => expect(categoriesSpy).toHaveBeenCalledTimes(1));
  });

  it('a category "edited" event (rename or recolor) re-reads the category list exactly once', async () => {
    const categoriesSpy = await renderSidebarSettled();

    simulateTauriEvent(EVENT_DATA_CHANGED, {
      entity_id: 'Office',
      entity_type: ENTITY_CATEGORY,
      reason: REASON_EDITED,
      origin_tag: 'test-origin-tag-rename',
    });

    await vi.waitFor(() => expect(categoriesSpy).toHaveBeenCalledTimes(1));
  });

  it('a category "deleted" event re-reads the category list exactly once', async () => {
    const categoriesSpy = await renderSidebarSettled();

    simulateTauriEvent(EVENT_DATA_CHANGED, {
      entity_id: 'Work',
      entity_type: ENTITY_CATEGORY,
      reason: REASON_DELETED,
      origin_tag: 'test-origin-tag-delete',
    });

    await vi.waitFor(() => expect(categoriesSpy).toHaveBeenCalledTimes(1));
  });
});
