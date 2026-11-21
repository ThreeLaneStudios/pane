/**
 * ProjectsView.categoryRefresh.test.ts — #3018 (`docs/spec/data-refresh.md`
 * §34.1/§34.2/§34.5, "category as a fifth entity_type" — "every screen
 * that shows categories must update").
 *
 * Finding: `ProjectsView.svelte` shows categories — each row renders a
 * `.category-indicator` dot colored via `getCategoryColor(project)`, which
 * looks up `project.category` against the `categories` list loaded once in
 * `onMount`'s `loadData()`. Before #3018, this view had **zero**
 * `pane-data-changed` listener of any kind, for ANY entity type — not just
 * uncovered for category (the gap this issue is about), but never
 * migrated by any of #2972's three prior stages either. It loaded once and
 * never refreshed, period.
 *
 * Measured directly against pre-fix `ProjectsView.svelte` (mount settles,
 * spies cleared, then the event is dispatched and the test waits 100ms
 * before reading the spy again):
 *
 *   category event -> listCategories calls: 0
 *   project event  -> getProjectOrFolder calls: 0
 *
 * Zero calls either way — confirms there is no listener at all. These
 * tests fail (expect 1, get 0) against that code.
 *
 * Fix: a listener scoped to what #3018 needs — `category` events re-read
 * the category list (§34.5, same "tiny, no per-item read" rationale as
 * Sidebar's), `project` events patch the one affected project (so a
 * rename/delete's bulk `projects.category` change repaints that project's
 * dot). Measured against the fixed code with the identical harness:
 *
 *   category event -> listCategories calls: 1
 *   project event  -> getProjectOrFolder calls: 1
 *
 * This does NOT retrofit the broader note/sheet/folder listener Stage
 * 2/3 gave every other screen — that remains a gap, out of #3018's scope,
 * flagged in the PR body.
 */

import { describe, it, expect, vi, afterEach } from 'vitest';
import { tick } from 'svelte';
import { render, cleanup, screen } from '@testing-library/svelte/svelte5';
import ProjectsView from './ProjectsView.svelte';
import * as api from '$lib/api';
import { simulateTauriEvent } from '../../../vitest-setup';
import {
  EVENT_DATA_CHANGED,
  ENTITY_CATEGORY,
  ENTITY_PROJECT,
  REASON_EDITED,
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
  id: 'proj-pv-category-1',
  name: 'PV Test Project',
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

/** Mounts ProjectsView, lets mount-time loading fully settle, then clears
 *  both spies — so counts measured afterward are attributable only to
 *  whatever is dispatched next. */
async function renderProjectsViewSettled() {
  vi.spyOn(api, 'listProjects').mockResolvedValue([PROJECT]);
  const categoriesSpy = vi.spyOn(api, 'listCategories').mockResolvedValue([{ category: 'Work', color: '#AABBCC' } as CategoryWithColor]);
  vi.spyOn(api, 'getProjectNoteCounts').mockResolvedValue(new Map());
  const getOneSpy = vi.spyOn(api, 'getProjectOrFolder').mockResolvedValue({ ...PROJECT, category: 'Office' } as any);

  render(ProjectsView, { props: {} });
  await screen.findByText('PV Test Project');
  await new Promise((r) => setTimeout(r, 100));
  await tick();
  categoriesSpy.mockClear();
  getOneSpy.mockClear();

  return { categoriesSpy, getOneSpy };
}

describe('ProjectsView pane-data-changed handler — category + project (#3018)', () => {
  it('a category event re-reads the category list exactly once', async () => {
    const { categoriesSpy } = await renderProjectsViewSettled();

    simulateTauriEvent(EVENT_DATA_CHANGED, {
      entity_id: 'Office',
      entity_type: ENTITY_CATEGORY,
      reason: REASON_EDITED,
      origin_tag: 'pv-test-category',
    });

    await vi.waitFor(() => expect(categoriesSpy).toHaveBeenCalledTimes(1));
  });

  it('a project/edited event (e.g. from a category rename\'s bulk update) patches that one project via a single-entity read, exactly once — not a full listProjects() refetch', async () => {
    const { getOneSpy } = await renderProjectsViewSettled();
    const listProjectsSpy = vi.spyOn(api, 'listProjects');
    listProjectsSpy.mockClear();

    simulateTauriEvent(EVENT_DATA_CHANGED, {
      entity_id: PROJECT.id,
      entity_type: ENTITY_PROJECT,
      reason: REASON_EDITED,
      origin_tag: 'pv-test-project-patch',
    });

    await vi.waitFor(() => expect(getOneSpy).toHaveBeenCalledTimes(1));
    expect(getOneSpy).toHaveBeenCalledWith(PROJECT.id);
    // The single-entity patch must NOT trigger a second full-list refetch.
    expect(listProjectsSpy).toHaveBeenCalledTimes(0);
  });
});
