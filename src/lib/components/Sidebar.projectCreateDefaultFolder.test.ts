/**
 * Sidebar.projectCreateDefaultFolder.test.ts — #2874 item 4 (docs/audits/
 * 2026-10-16-scope-1060-test-failures.md item 4, "New project's General
 * folder missing until 2nd folder exists").
 *
 * Root cause (backend-only, confirmed by the audit): `create_project_internal`
 * creates BOTH the project row and its auto-created default folder row in
 * the same transaction — both exist in the DB immediately — but the caller
 * (the Sidebar's `create_project` Tauri command, and the agent's
 * `create_project` tool) used to emit only ONE `pane-data-changed` event,
 * for the project's own id. `Sidebar.svelte`'s `handleDataChangedEvent` (the
 * same single-entity patch handler `Sidebar.dataChanged.test.ts` already
 * covers for the plain "create a folder" case) only learns a row exists
 * when THAT row's own id is announced — so the default folder stayed
 * invisible until something else happened to trigger a full
 * `syncProjectsWithNotebooks()` reload (e.g. creating a SECOND folder).
 *
 * This is a backend-only fix (`projects::emit_project_created_events`, used
 * by both the Tauri command and the agent tool) — `Sidebar.svelte`'s
 * handler already does the right thing for ANY `ENTITY_PROJECT`/
 * `ENTITY_FOLDER` `created` event (proven by `Sidebar.dataChanged.test.ts`'s
 * Path 1); this test exists to lock in the ACTUAL two-event sequence
 * `create_project` now emits, and to prove the default folder appears from
 * a SINGLE project creation — no second folder, no full reload.
 */

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, cleanup, screen } from '@testing-library/svelte/svelte5';
import Sidebar from './Sidebar.svelte';
import * as api from '$lib/api';
import { simulateTauriEvent } from '../../../vitest-setup';
import { EVENT_DATA_CHANGED, ENTITY_PROJECT, ENTITY_FOLDER, REASON_CREATED } from '$lib/refreshEvent';

const win = globalThis.window as any;
if (typeof win.addEventListener !== 'function') {
  win.addEventListener = globalThis.addEventListener?.bind(globalThis) ?? (() => {});
}
if (typeof win.removeEventListener !== 'function') {
  win.removeEventListener = globalThis.removeEventListener?.bind(globalThis) ?? (() => {});
}
win.dispatchEvent = globalThis.dispatchEvent.bind(globalThis);

beforeEach(() => {
  vi.spyOn(api, 'listTags').mockResolvedValue([]);
  vi.spyOn(api, 'listCategories').mockResolvedValue([]);
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe('Sidebar — a newly created project announces its default folder too (#2874 item 4)', () => {
  it('both the project-created and folder-created events patch in, from ONE project creation — no second folder ever created, no full reload', async () => {
    // Mount with an EMPTY project list — nothing exists yet, mirroring a
    // fresh app state right before the user creates their first project.
    const syncSpy = vi.spyOn(api, 'syncProjectsWithNotebooks').mockResolvedValueOnce([]);
    const getOneSpy = vi.spyOn(api, 'getProjectOrFolder').mockImplementation(async (id: string) => {
      if (id === 'proj-new-1') {
        return {
          id: 'proj-new-1',
          name: 'Brand New Project',
          parent_id: undefined,
          category: null,
          sort_order: 0,
          created_at: '2026-10-17T00:00:00Z',
          updated_at: '2026-10-17T00:00:00Z',
          is_system: false,
          is_favorite: true,
        } as any;
      }
      if (id === 'folder-default-1') {
        return {
          id: 'folder-default-1',
          name: 'General',
          parent_id: 'proj-new-1',
          category: null,
          sort_order: 0,
          created_at: '2026-10-17T00:00:00Z',
          updated_at: '2026-10-17T00:00:00Z',
          is_system: false,
          is_favorite: false,
        } as any;
      }
      throw new Error(`unexpected id: ${id}`);
    });

    render(Sidebar, { props: {} });

    // Nothing exists yet.
    await vi.waitFor(() => expect(syncSpy).toHaveBeenCalledTimes(1));
    expect(screen.queryByText('Brand New Project')).toBeNull();
    expect(screen.queryByText('General')).toBeNull();

    // The real `create_project_internal` creates the project AND its
    // default folder in one DB transaction (already true at base) —
    // `emit_project_created_events` (#2874 item 4's fix) now emits BOTH
    // events for that one call, project first, folder second, exactly as
    // the Tauri command and the agent tool both do.
    simulateTauriEvent(EVENT_DATA_CHANGED, {
      entity_id: 'proj-new-1',
      entity_type: ENTITY_PROJECT,
      reason: REASON_CREATED,
      origin_tag: 'test-origin-project',
    });
    await screen.findByText('Brand New Project');

    simulateTauriEvent(EVENT_DATA_CHANGED, {
      entity_id: 'folder-default-1',
      entity_type: ENTITY_FOLDER,
      reason: REASON_CREATED,
      origin_tag: 'test-origin-folder',
    });

    // The default folder is visible from THIS ONE project creation — no
    // second folder was ever created, and no full-list reload happened.
    await screen.findByText('General');
    expect(getOneSpy).toHaveBeenCalledTimes(2);
    expect(getOneSpy).toHaveBeenCalledWith('proj-new-1');
    expect(getOneSpy).toHaveBeenCalledWith('folder-default-1');
    // The full-list refetch only ever ran once, at mount — proving the
    // folder's visibility came from the single-entity patch handler, not
    // from some other reload incidentally picking it up too (the exact
    // "disappears until a 2nd folder exists" mechanism the audit traced).
    expect(syncSpy).toHaveBeenCalledTimes(1);
  });
});
