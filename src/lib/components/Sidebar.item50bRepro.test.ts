/**
 * Sidebar.item50bRepro.test.ts — #2972 Stage 4 (`docs/spec/
 * data-refresh.md` §34.10 Stage 4 acceptance criteria, item 50b's literal
 * repro): "an agent-created sheet's folder appears in the sidebar without
 * a restart."
 *
 * The underlying mechanism (`Sidebar.svelte`'s single `pane-data-changed`
 * handler, `entity_type === 'folder'` + `reason === 'created'`, patches
 * the one new folder into the tree via `api.getProjectOrFolder()`, never
 * a full `syncProjectsWithNotebooks()` refetch) already landed in Stage 2
 * and is covered generically by `Sidebar.dataChanged.test.ts`'s "Path 1"
 * tests (the agent `create_folder` tool path). This file is Stage 4's
 * explicit, named verification of item 50b's own scenario — the folder an
 * agent's `create_sheet` tool call resolves/creates INTO, not a bare
 * `create_folder` call in isolation — run against `main` at Stage 4's
 * base, not a new code path.
 *
 * §34.0 context: folder auto-create-on-miss was deleted outright by #2874
 * item 50b's own backend fix (#3011) — `execute_create_sheet` now either
 * resolves an EXISTING folder or refuses, it never silently creates one.
 * The realistic path an agent takes to end up with a brand-new folder
 * holding a brand-new sheet is: `create_folder` (explicit tool, backend
 * emits `entity_type: 'folder', reason: 'created'` per Stage 1), THEN
 * `create_sheet` into that folder (`entity_type: 'sheet', reason:
 * 'created'`, resolved via the now-existing folder). This test exercises
 * exactly that two-step sequence as one agent turn would produce it, and
 * asserts the folder is visible in the sidebar tree before any reload.
 *
 * Honest result (per the Stage 4 task's own instruction: "the
 * verification repros... may pass at base — say so honestly rather than
 * manufacturing a failure"): this test PASSES against `main` as-is.
 * Stage 1/2 already closed this gap; this file exists to pin it as a
 * named, permanent regression guard referencing item 50b directly.
 */

import { describe, it, expect, vi, afterEach } from 'vitest';
import { render, cleanup, screen } from '@testing-library/svelte/svelte5';
import Sidebar from './Sidebar.svelte';
import * as api from '$lib/api';
import { simulateTauriEvent } from '../../../vitest-setup';
import { EVENT_DATA_CHANGED, ENTITY_FOLDER, ENTITY_SHEET, REASON_CREATED } from '$lib/refreshEvent';
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
  id: 'proj-item50b-sidebar',
  name: 'Item 50b Repro Project',
  parent_id: undefined,
  category: null,
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

describe('item 50b repro — agent-created sheet\'s folder appears in the sidebar without a restart (#2972 stage 4)', () => {
  it('create_folder then create_sheet (one agent turn) — the new folder is visible with no reload/restart', async () => {
    vi.spyOn(api, 'listTags').mockResolvedValue([]);
    vi.spyOn(api, 'listCategories').mockResolvedValue([]);
    const syncSpy = vi.spyOn(api, 'syncProjectsWithNotebooks').mockResolvedValueOnce([PROJECT]);
    const getOneSpy = vi.spyOn(api, 'getProjectOrFolder').mockResolvedValue({
      id: 'folder-for-new-sheet',
      name: 'Sheet Folder',
      parent_id: PROJECT.id,
      category: null,
      sort_order: 0,
      created_at: '2026-01-01T00:00:00Z',
      updated_at: '2026-01-01T00:00:00Z',
      is_system: false,
      is_favorite: false,
    } as any);

    render(Sidebar, { props: {} });

    await screen.findByText('Item 50b Repro Project');
    expect(syncSpy).toHaveBeenCalledTimes(1);
    expect(screen.queryByText('Sheet Folder')).toBeNull();

    // Step 1: agent's explicit create_folder tool call (backend emit,
    // Stage 1) — the folder this turn's sheet will live in.
    simulateTauriEvent(EVENT_DATA_CHANGED, {
      entity_id: 'folder-for-new-sheet',
      entity_type: ENTITY_FOLDER,
      reason: REASON_CREATED,
      origin_tag: 'item50b-origin-folder',
    });

    // No navigation, no restart, no reload — the folder appears in the
    // tree from the live app state alone.
    await screen.findByText('Sheet Folder');
    expect(getOneSpy).toHaveBeenCalledWith('folder-for-new-sheet');
    // Patched in place — not a second full project/folder tree refetch.
    expect(syncSpy).toHaveBeenCalledTimes(1);

    // Step 2 (same agent turn): create_sheet resolves into the
    // now-existing folder. Sidebar doesn't render sheet rows itself, but
    // this confirms the sheet event doesn't regress the folder's
    // visibility or trigger an unwanted full reload either.
    simulateTauriEvent(EVENT_DATA_CHANGED, {
      entity_id: 'sheet-in-new-folder',
      entity_type: ENTITY_SHEET,
      reason: REASON_CREATED,
      origin_tag: 'item50b-origin-sheet',
    });

    await new Promise((r) => setTimeout(r, 50));
    expect(screen.getByText('Sheet Folder')).toBeTruthy();
    expect(syncSpy).toHaveBeenCalledTimes(1);
  });
});
