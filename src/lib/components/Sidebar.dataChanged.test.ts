/**
 * Sidebar.dataChanged.test.ts — #2972 stage 2 (`docs/spec/data-refresh.md`
 * §34.5/§34.10's "Folder-creation-visibility (Sidebar)" acceptance
 * criterion):
 *
 * "create a folder via each of the two remaining live paths (explicit
 * create_folder tool, human Sidebar path) and assert it appears in
 * $projects-derived tree without manual reload/restart."
 *
 * Path 1 (agent `create_folder` tool): the ONLY thing #2972 stage 1 (#3014)
 * + stage 2 (this PR) change — Stage 1 gave the backend's `create_folder`
 * paths their first-ever `pane-data-changed` emit; Stage 2 gives
 * Sidebar.svelte a listener for it (`listen(EVENT_DATA_CHANGED, ...)`,
 * since backend emits are invisible to window.addEventListener, #2215),
 * replacing the now-dead `listen('pane-note-changed', ...)` registration
 * (dead since Stage 1 migrated that emit site onto the new event name).
 *
 * #2972 stage 2 WARN closure (PR #3016 / Reed's 2026-10-11 audit, Matt's
 * sign-off "fix before merge"): Path 1's handler used to re-fetch the
 * WHOLE project/folder list (`api.syncProjectsWithNotebooks()` ->
 * `get_projects()`) on every folder/project event — Reed's WARN. It now
 * patches the ONE changed folder in via a new single-entity read
 * (`api.getProjectOrFolder()`), so `syncSpy`'s count stays at 1 (the
 * mount-time load only) instead of climbing to 2 on the event.
 *
 * Path 2 (human Sidebar "New Folder"): unchanged by #2972 — `submitNewFolder()`
 * (Sidebar.svelte) already worked today by manually re-fetching `$projects`
 * right after `api.createFolder()`, with no event dependency at all
 * (§34.1.4). Included here as an explicit regression guard, driven through
 * the real context-menu -> input -> Enter UI flow, not re-implemented.
 */

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, cleanup, screen, fireEvent } from '@testing-library/svelte/svelte5';
import Sidebar from './Sidebar.svelte';
import * as api from '$lib/api';
import { simulateTauriEvent } from '../../../vitest-setup';
import { EVENT_DATA_CHANGED, ENTITY_FOLDER, REASON_CREATED, dispatchDataChanged } from '$lib/refreshEvent';
import type { Project } from '$lib/types';

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

const PROJECT: Project = {
  id: 'proj-sidebar-datachanged-1',
  name: 'Test Project',
  parent_id: undefined,
  category: null,
  sort_order: 0,
  created_at: '2026-01-01T00:00:00Z',
  updated_at: '2026-01-01T00:00:00Z',
  is_system: false,
  is_favorite: true,
} as any;

beforeEach(() => {
  vi.spyOn(api, 'listTags').mockResolvedValue([]);
  vi.spyOn(api, 'listCategories').mockResolvedValue([]);
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe('Sidebar pane-data-changed handler — folder creation visibility (#2972 stage 2, §34.5/§34.10)', () => {
  it('Path 1 — a folder created by the agent create_folder tool appears without a reload, and does NOT trigger a full syncProjectsWithNotebooks() refetch', async () => {
    const syncSpy = vi.spyOn(api, 'syncProjectsWithNotebooks')
      .mockResolvedValueOnce([PROJECT]); // initial mount load only — the event must not call this a second time
    const getOneSpy = vi.spyOn(api, 'getProjectOrFolder').mockResolvedValue({
      id: 'folder-agent-created-1',
      name: 'Agent Created Folder',
      parent_id: PROJECT.id,
      category: null,
      sort_order: 0,
      created_at: '2026-01-01T00:00:00Z',
      updated_at: '2026-01-01T00:00:00Z',
      is_system: false,
      is_favorite: false,
    } as any);

    render(Sidebar, { props: {} });

    await screen.findByText('Test Project');
    expect(syncSpy).toHaveBeenCalledTimes(1);
    expect(screen.queryByText('Agent Created Folder')).toBeNull();

    // Simulate the backend emitting the new event for the agent's
    // create_folder tool call — the Tauri channel (backend emits are
    // invisible to window.addEventListener, #2215).
    simulateTauriEvent(EVENT_DATA_CHANGED, {
      entity_id: 'folder-agent-created-1',
      entity_type: ENTITY_FOLDER,
      reason: REASON_CREATED,
      origin_tag: 'test-origin-tag',
    });

    await screen.findByText('Agent Created Folder');
    // The single-entity read fires exactly once for this event...
    expect(getOneSpy).toHaveBeenCalledTimes(1);
    expect(getOneSpy).toHaveBeenCalledWith('folder-agent-created-1');
    // ...and the full-list refetch (`syncSpy`) does NOT fire again — this is
    // Reed's WARN closing: the count stays at 1, not 2.
    expect(syncSpy).toHaveBeenCalledTimes(1);
  });

  it('Path 2 — a folder created via the sidebar\'s own "New Folder" UI appears without a reload (regression guard, unchanged by #2972)', async () => {
    vi.spyOn(api, 'syncProjectsWithNotebooks').mockResolvedValue([PROJECT]);
    const createFolderSpy = vi.spyOn(api, 'createFolder').mockResolvedValue({
      id: 'folder-human-created-1',
      name: 'Human Created Folder',
      parent_id: PROJECT.id,
    } as any);
    vi.spyOn(api, 'listProjects').mockResolvedValue([
      PROJECT,
      {
        id: 'folder-human-created-1',
        name: 'Human Created Folder',
        parent_id: PROJECT.id,
        category: null,
        sort_order: 0,
        created_at: '2026-01-01T00:00:00Z',
        updated_at: '2026-01-01T00:00:00Z',
        is_system: false,
      } as any,
    ]);

    render(Sidebar, { props: {} });

    const projectRow = (await screen.findByText('Test Project')).closest('.project-item') as HTMLElement;
    expect(projectRow).toBeTruthy();
    await fireEvent.contextMenu(projectRow);

    const newFolderBtn = await screen.findByText('New Folder');
    await fireEvent.click(newFolderBtn);

    const input = await screen.findByPlaceholderText('New folder name');
    await fireEvent.input(input, { target: { value: 'Human Created Folder' } });
    await fireEvent.keyDown(input, { key: 'Enter' });

    await screen.findByText('Human Created Folder');
    expect(createFolderSpy).toHaveBeenCalledWith(PROJECT.id, 'Human Created Folder');
  });

  it('Path 1, via the APP channel instead of the backend Tauri channel (#2972 stage 3, item 4) — same patch-in-place behavior', async () => {
    const syncSpy = vi.spyOn(api, 'syncProjectsWithNotebooks')
      .mockResolvedValueOnce([PROJECT]);
    const getOneSpy = vi.spyOn(api, 'getProjectOrFolder').mockResolvedValue({
      id: 'folder-app-channel-1',
      name: 'App Channel Folder',
      parent_id: PROJECT.id,
      category: null,
      sort_order: 0,
      created_at: '2026-01-01T00:00:00Z',
      updated_at: '2026-01-01T00:00:00Z',
      is_system: false,
      is_favorite: false,
    } as any);

    render(Sidebar, { props: {} });

    await screen.findByText('Test Project');
    expect(syncSpy).toHaveBeenCalledTimes(1);
    expect(screen.queryByText('App Channel Folder')).toBeNull();

    dispatchDataChanged('folder-app-channel-1', ENTITY_FOLDER, REASON_CREATED, 'test-origin-tag-app');

    await screen.findByText('App Channel Folder');
    expect(getOneSpy).toHaveBeenCalledWith('folder-app-channel-1');
    expect(syncSpy).toHaveBeenCalledTimes(1);
  });
});
