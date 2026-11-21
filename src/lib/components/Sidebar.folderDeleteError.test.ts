/**
 * Sidebar.folderDeleteError.test.ts — #2874 item 65 ruling 2, Lane 1 PR 3.
 *
 * "Folder-delete errors show to the user, not console-only. Today a
 * failure rolls back and displays nothing." Before this PR,
 * `confirmDeleteFolder`'s catch block only did `console.error(...)` and
 * then unconditionally cleared/closed the dialog state — this test's
 * assertion (`screen.getByText(errorMessage)`) fails outright against
 * pre-fix code (`Unable to find an element with the text`), because
 * nothing renders the error and the dialog is already gone by the time
 * the assertion runs.
 */

import { describe, it, expect, vi, afterEach } from 'vitest';
import { render, cleanup, screen, fireEvent } from '@testing-library/svelte/svelte5';
import Sidebar from './Sidebar.svelte';
import * as api from '$lib/api';
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
  id: 'proj-folder-delete-error',
  name: 'Folder Delete Error Project',
  parent_id: undefined,
  category: null,
  sort_order: 0,
  created_at: '2026-01-01T00:00:00Z',
  updated_at: '2026-01-01T00:00:00Z',
  is_system: false,
  is_favorite: true,
  default_folder_id: 'folder-default',
} as any;

const DEFAULT_FOLDER: Project = {
  id: 'folder-default',
  name: 'General',
  parent_id: PROJECT.id,
  category: null,
  sort_order: 0,
  created_at: '2026-01-01T00:00:00Z',
  updated_at: '2026-01-01T00:00:00Z',
  is_system: false,
  is_favorite: false,
} as any;

const WORK_FOLDER: Project = {
  id: 'folder-work',
  name: 'Work',
  parent_id: PROJECT.id,
  category: null,
  sort_order: 1,
  created_at: '2026-01-01T00:00:00Z',
  updated_at: '2026-01-01T00:00:00Z',
  is_system: false,
  is_favorite: false,
} as any;

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe('Sidebar — folder delete error is visible in the UI (#2874 item 65 ruling 2)', () => {
  it('a failed delete_folder keeps the dialog open and shows the error, not just console.error', async () => {
    vi.spyOn(api, 'listTags').mockResolvedValue([]);
    vi.spyOn(api, 'listCategories').mockResolvedValue([]);
    vi.spyOn(api, 'syncProjectsWithNotebooks').mockResolvedValue([PROJECT, DEFAULT_FOLDER, WORK_FOLDER]);
    vi.spyOn(api, 'getFolderDeletePreview').mockResolvedValue({
      note_count: 0,
      sheet_count: 0,
      default_folder_name: 'General',
    });
    const consoleErrorSpy = vi.spyOn(console, 'error').mockImplementation(() => {});
    const errorMessage = 'Failed to delete folder: disk is full';
    vi.spyOn(api, 'deleteFolder').mockRejectedValue(errorMessage);

    render(Sidebar, { props: {} });

    const folderBtn = await screen.findByText('Work');
    const folderButtonEl = folderBtn.closest('button.folder-item') as HTMLElement;
    expect(folderButtonEl).toBeTruthy();

    await fireEvent.contextMenu(folderButtonEl);

    const deleteMenuItem = await screen.findByText('Delete Folder');
    await fireEvent.click(deleteMenuItem);

    // Confirm dialog appears with the fetched preview message.
    await screen.findByText(/Delete "Work"\?/);

    const confirmBtn = await screen.findByText('Delete');
    await fireEvent.click(confirmBtn);

    // The error must render SOMEWHERE in the DOM, not just console.error.
    await screen.findByText(errorMessage);
    expect(consoleErrorSpy).toHaveBeenCalled();

    // The dialog stays open on failure (not silently closed).
    expect(screen.queryByText(/Delete "Work"\?/)).toBeTruthy();
  });

  it('a successful delete_folder closes the dialog with no error shown', async () => {
    vi.spyOn(api, 'listTags').mockResolvedValue([]);
    vi.spyOn(api, 'listCategories').mockResolvedValue([]);
    vi.spyOn(api, 'syncProjectsWithNotebooks')
      .mockResolvedValueOnce([PROJECT, DEFAULT_FOLDER, WORK_FOLDER])
      .mockResolvedValueOnce([PROJECT, DEFAULT_FOLDER]);
    vi.spyOn(api, 'getFolderDeletePreview').mockResolvedValue({
      note_count: 0,
      sheet_count: 0,
      default_folder_name: 'General',
    });
    vi.spyOn(api, 'deleteFolder').mockResolvedValue(undefined);

    render(Sidebar, { props: {} });

    const folderBtn = await screen.findByText('Work');
    const folderButtonEl = folderBtn.closest('button.folder-item') as HTMLElement;
    await fireEvent.contextMenu(folderButtonEl);

    const deleteMenuItem = await screen.findByText('Delete Folder');
    await fireEvent.click(deleteMenuItem);
    await screen.findByText(/Delete "Work"\?/);

    const confirmBtn = await screen.findByText('Delete');
    await fireEvent.click(confirmBtn);

    await new Promise((r) => setTimeout(r, 20));
    expect(screen.queryByText(/Delete "Work"\?/)).toBeNull();
  });
});
