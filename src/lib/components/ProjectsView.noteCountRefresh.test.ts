/**
 * ProjectsView.noteCountRefresh.test.ts — #2972 Stage 4 (`docs/spec/
 * data-refresh.md` §34.10 Stage 4, §34.11 point 4 — cross-screen
 * verification; item 2 of the stage-4 task: "ProjectsView must handle
 * note, sheet and folder events too — so note counts stay right while
 * it's open. #3019 gave it only category + project; this stage completes
 * it.").
 *
 * Finding (inventory, §34 Stage 4): `ProjectsView.svelte` renders a
 * "Notes" column per project (`getNoteCount(project.id)`, backed by
 * `noteCountsMap`, loaded once in `onMount`'s `loadData()` via
 * `api.getProjectNoteCounts()`). #3018's fix (`ProjectsView.
 * categoryRefresh.test.ts`) gave this view a listener for `category` and
 * `project` events only — note/sheet/folder events (the ones that
 * actually change how many notes a project has) were explicitly flagged
 * there as "out of #3018's scope... a future issue," with no separate
 * issue opened; #2972 stage 4 is that issue, per Matt's ruling.
 *
 * Measured directly against pre-fix `ProjectsView.svelte` (mount settles,
 * spy cleared, event dispatched, 100ms wait — same harness as
 * `ProjectsView.categoryRefresh.test.ts`):
 *
 *   note event   -> getProjectNoteCounts calls: 0
 *   sheet event  -> getProjectNoteCounts calls: 0
 *   folder event -> getProjectNoteCounts calls: 0
 *
 * Zero calls for all three — confirms the gap. These tests fail (expect 1,
 * get 0) against that code.
 *
 * Fix: `entity_type === 'note' | 'sheet' | 'folder'` re-reads ONLY the
 * note-counts map (`getProjectNoteCounts()` — one grouped-COUNT query),
 * never `listProjects()`/`listCategories()` — §34.5's "patch, never
 * full-collection refetch" discipline, same "tiny, no per-item read"
 * rationale the category branch already uses. Measured against the fixed
 * code with the identical harness:
 *
 *   note event   -> getProjectNoteCounts calls: 1
 *   sheet event  -> getProjectNoteCounts calls: 1
 *   folder event -> getProjectNoteCounts calls: 1
 *
 * And, for each: `listProjects()`/`listCategories()` calls stay at 0 —
 * proving this is a counts-only patch, not a broader reload.
 */

import { describe, it, expect, vi, afterEach } from 'vitest';
import { tick } from 'svelte';
import { render, cleanup, screen } from '@testing-library/svelte/svelte5';
import ProjectsView from './ProjectsView.svelte';
import * as api from '$lib/api';
import { simulateTauriEvent } from '../../../vitest-setup';
import {
  EVENT_DATA_CHANGED,
  ENTITY_NOTE,
  ENTITY_SHEET,
  ENTITY_FOLDER,
  REASON_CREATED,
  REASON_EDITED,
  REASON_DELETED,
} from '$lib/refreshEvent';
import type { Project, CategoryWithColor } from '$lib/types';

const win = globalThis.window as any;
if (typeof win.addEventListener !== 'function') {
  win.addEventListener = globalThis.addEventListener?.bind(globalThis) ?? (() => {});
}
if (typeof win.removeEventListener !== 'function') {
  win.removeEventListener = globalThis.removeEventListener?.bind(globalThis) ?? (() => {});
}
win.dispatchEvent = globalThis.dispatchEvent.bind(globalThis);

const PROJECT: Project = {
  id: 'proj-pv-notecount-1',
  name: 'PV NoteCount Test Project',
  parent_id: undefined,
  category: 'Work',
  sort_order: 0,
  created_at: '2026-01-01T00:00:00Z',
  updated_at: '2026-01-01T00:00:00Z',
  is_system: false,
  is_favorite: false,
} as any;

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

async function renderProjectsViewSettled() {
  vi.spyOn(api, 'listProjects').mockResolvedValue([PROJECT]);
  vi.spyOn(api, 'listCategories').mockResolvedValue([{ category: 'Work', color: '#AABBCC' } as CategoryWithColor]);
  const countsSpy = vi.spyOn(api, 'getProjectNoteCounts').mockResolvedValue(new Map([[PROJECT.id, 3]]));

  render(ProjectsView, { props: {} });
  await screen.findByText('PV NoteCount Test Project');
  await new Promise((r) => setTimeout(r, 100));
  await tick();
  countsSpy.mockClear();

  return { countsSpy };
}

describe('ProjectsView pane-data-changed handler — note/sheet/folder note-count refresh (#2972 stage 4)', () => {
  it('a note/created event re-reads the note-counts map exactly once', async () => {
    const { countsSpy } = await renderProjectsViewSettled();
    const listProjectsSpy = vi.spyOn(api, 'listProjects');
    listProjectsSpy.mockClear();
    const listCategoriesSpy = vi.spyOn(api, 'listCategories');
    listCategoriesSpy.mockClear();

    simulateTauriEvent(EVENT_DATA_CHANGED, {
      entity_id: 'note-xyz',
      entity_type: ENTITY_NOTE,
      reason: REASON_CREATED,
      origin_tag: 'pv-test-note-created',
    });

    await vi.waitFor(() => expect(countsSpy).toHaveBeenCalledTimes(1));
    // Counts-only patch — no broader reload of the project/category lists.
    expect(listProjectsSpy).toHaveBeenCalledTimes(0);
    expect(listCategoriesSpy).toHaveBeenCalledTimes(0);
  });

  it('a sheet/edited event re-reads the note-counts map exactly once', async () => {
    const { countsSpy } = await renderProjectsViewSettled();

    simulateTauriEvent(EVENT_DATA_CHANGED, {
      entity_id: 'sheet-xyz',
      entity_type: ENTITY_SHEET,
      reason: REASON_EDITED,
      origin_tag: 'pv-test-sheet-edited',
    });

    await vi.waitFor(() => expect(countsSpy).toHaveBeenCalledTimes(1));
  });

  it('a folder/deleted event re-reads the note-counts map exactly once', async () => {
    const { countsSpy } = await renderProjectsViewSettled();

    simulateTauriEvent(EVENT_DATA_CHANGED, {
      entity_id: 'folder-xyz',
      entity_type: ENTITY_FOLDER,
      reason: REASON_DELETED,
      origin_tag: 'pv-test-folder-deleted',
    });

    await vi.waitFor(() => expect(countsSpy).toHaveBeenCalledTimes(1));
  });

  it('the resulting map is reflected in the rendered note count', async () => {
    vi.spyOn(api, 'listProjects').mockResolvedValue([PROJECT]);
    vi.spyOn(api, 'listCategories').mockResolvedValue([{ category: 'Work', color: '#AABBCC' } as CategoryWithColor]);
    const countsSpy = vi
      .spyOn(api, 'getProjectNoteCounts')
      .mockResolvedValueOnce(new Map([[PROJECT.id, 3]]))
      .mockResolvedValueOnce(new Map([[PROJECT.id, 4]]));

    render(ProjectsView, { props: {} });
    await screen.findByText('PV NoteCount Test Project');
    await new Promise((r) => setTimeout(r, 100));
    await tick();
    await screen.findByText('3');

    simulateTauriEvent(EVENT_DATA_CHANGED, {
      entity_id: 'note-xyz',
      entity_type: ENTITY_NOTE,
      reason: REASON_CREATED,
      origin_tag: 'pv-test-note-created-2',
    });

    await vi.waitFor(() => expect(countsSpy).toHaveBeenCalledTimes(2));
    await screen.findByText('4');
  });
});
