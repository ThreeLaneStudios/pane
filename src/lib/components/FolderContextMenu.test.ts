/**
 * FolderContextMenu.test.ts — #2874 item 65, Lane 1 PR 3.
 *
 * Matt's ruling 1: the default folder's Delete is SHOWN but DISABLED, with
 * the exact tooltip "This is the default folder. Choose a different default
 * in Project Settings to delete it." — not hidden, the user must see why
 * they can't. A non-default folder's Delete stays enabled.
 *
 * Shown failing before the fix: before this PR, `FolderContextMenu.svelte`
 * had no `isDefaultFolder` prop at all — the Delete button was always a
 * plain, always-enabled `<button>` with no `disabled`/`title` attribute, so
 * `deleteBtn.disabled` below is `false` and `deleteBtn.title` is `""` even
 * when passed `isDefaultFolder: true`.
 */

import { describe, it, expect, vi } from 'vitest';
import { render, fireEvent } from '@testing-library/svelte/svelte5';
import FolderContextMenu from './FolderContextMenu.svelte';

const DEFAULT_FOLDER_DELETE_TOOLTIP =
  'This is the default folder. Choose a different default in Project Settings to delete it.';

function mkFolder(overrides: Partial<any> = {}) {
  return {
    id: 'folder-1',
    name: 'General',
    parent_id: 'project-1',
    sort_order: 0,
    created_at: '',
    updated_at: '',
    ...overrides,
  };
}

describe('FolderContextMenu — default folder delete disabled-with-tooltip (#2874 item 65)', () => {
  it('disables Delete and shows the exact tooltip for the default folder', async () => {
    const onDelete = vi.fn();
    const { container } = render(FolderContextMenu, {
      props: {
        folder: mkFolder(),
        position: { x: 0, y: 0 },
        onClose: vi.fn(),
        onRename: vi.fn(),
        onDelete,
        isDefaultFolder: true,
      },
    });

    const deleteBtn = container.querySelector('.delete-item') as HTMLButtonElement;
    expect(deleteBtn).toBeTruthy();
    expect(deleteBtn.disabled).toBe(true);
    expect(deleteBtn.title).toBe(DEFAULT_FOLDER_DELETE_TOOLTIP);

    await fireEvent.click(deleteBtn);
    expect(onDelete).not.toHaveBeenCalled();
  });

  it('leaves Delete enabled, no tooltip, for a non-default folder', async () => {
    const onDelete = vi.fn();
    const onClose = vi.fn();
    const { container } = render(FolderContextMenu, {
      props: {
        folder: mkFolder({ id: 'folder-2', name: 'Work' }),
        position: { x: 0, y: 0 },
        onClose,
        onRename: vi.fn(),
        onDelete,
        isDefaultFolder: false,
      },
    });

    const deleteBtn = container.querySelector('.delete-item') as HTMLButtonElement;
    expect(deleteBtn.disabled).toBe(false);
    expect(deleteBtn.title).toBe('');

    await fireEvent.click(deleteBtn);
    expect(onDelete).toHaveBeenCalled();
    expect(onClose).toHaveBeenCalled();
  });

  it('defaults isDefaultFolder to false when omitted (back-compat)', () => {
    const { container } = render(FolderContextMenu, {
      props: {
        folder: mkFolder(),
        position: { x: 0, y: 0 },
        onClose: vi.fn(),
        onRename: vi.fn(),
        onDelete: vi.fn(),
      },
    });

    const deleteBtn = container.querySelector('.delete-item') as HTMLButtonElement;
    expect(deleteBtn.disabled).toBe(false);
  });
});
