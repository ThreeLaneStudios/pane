/**
 * Trash.confirmDialog.test.ts — #2874 item G: "one confirm dialog, not
 * many." `Trash.svelte`'s hand-rolled Delete Permanently (and Empty Trash,
 * which shared the exact same modal/state machine) confirm modal is
 * replaced with the shared `ConfirmDialog.svelte` component — the same one
 * Sidebar/NotesBrowser/AllTasksView/KanbanBoard/NewProviderModal/Onboarding/
 * ProviderConnectionCard already use.
 *
 * Scope-doc-confirmed root cause (2026-10-16 audit, item 7): the hand-rolled
 * `.confirm-modal` background was `rgba(255,255,255,0.05)` — effectively
 * invisible against its own `rgba(0,0,0,0.6)` backdrop. `ConfirmDialog`'s
 * solid `#1E1E20` fixes that by construction (shared component, not a
 * forked copy that can drift again).
 *
 * These tests assert on OBSERVABLE BEHAVIOR unique to `ConfirmDialog`
 * (its `role="dialog"` markup, its `error` prop surfacing, Escape-to-cancel)
 * rather than on the removed CSS token directly — the point of this PR is
 * that `Trash.svelte` no longer owns its own modal markup/styling at all.
 * Would FAIL against the pre-fix hand-rolled modal: it had no
 * `role="dialog"` on a `.confirm-modal` with ConfirmDialog's structure, no
 * `error` prop support (a failed delete silently closed the dialog via
 * `console.error` only), and used a locally-scoped Escape handler bound to
 * `onClose` (the whole Trash screen), not ConfirmDialog's own `onCancel`.
 */

import { describe, it, expect, vi, afterEach } from 'vitest';
import { render, cleanup, screen, fireEvent } from '@testing-library/svelte/svelte5';
import Trash from './Trash.svelte';
import * as api from '$lib/api';
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
  title: 'Note To Purge',
  notebook: 'Notes',
  slug: 'note-to-purge',
  deleted_at: '2026-10-01T00:00:00Z',
  original_path: 'Notes/note-to-purge.md',
};

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

async function renderTrashSettled(items: TrashItem[] = [TRASH_ITEM]) {
  vi.spyOn(api, 'listTrash').mockResolvedValue(items);
  render(Trash, { props: { onClose: () => {} } });
  if (items.length > 0) {
    await screen.findByText(items[0].title!);
  } else {
    await screen.findByText('Trash is empty');
  }
}

describe('Trash Delete Permanently goes through the shared ConfirmDialog (#2874 item G)', () => {
  it('clicking Delete Permanently opens a real dialog (role="dialog"), not the old hand-rolled modal', async () => {
    await renderTrashSettled();
    await fireEvent.click(screen.getByText('Delete Permanently'));

    const dialog = await screen.findByRole('dialog');
    expect(dialog).toBeTruthy();
    expect(screen.getByText('Delete Permanently?')).toBeTruthy();
    expect(screen.getByText(/Note To Purge.*cannot be undone/)).toBeTruthy();
  });

  it('confirming calls api.deletePermanently and closes the dialog on success', async () => {
    const deleteSpy = vi.spyOn(api, 'deletePermanently').mockResolvedValue(undefined as any);
    vi.spyOn(api, 'listTrash')
      .mockResolvedValueOnce([TRASH_ITEM])
      .mockResolvedValueOnce([]);

    render(Trash, { props: { onClose: () => {} } });
    await screen.findByText('Note To Purge');
    await fireEvent.click(screen.getByText('Delete Permanently'));
    await screen.findByRole('dialog');

    // ConfirmDialog's own confirm button, labeled via confirmLabel="Delete".
    await fireEvent.click(screen.getByRole('button', { name: 'Delete' }));

    await vi.waitFor(() => expect(deleteSpy).toHaveBeenCalledWith('trash-note-1'));
    await vi.waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());
  });

  it('cancelling closes the dialog without calling the delete API', async () => {
    const deleteSpy = vi.spyOn(api, 'deletePermanently');
    await renderTrashSettled();
    await fireEvent.click(screen.getByText('Delete Permanently'));
    await screen.findByRole('dialog');

    await fireEvent.click(screen.getByText('Cancel'));

    expect(screen.queryByRole('dialog')).toBeNull();
    expect(deleteSpy).not.toHaveBeenCalled();
  });

  it('a failed delete surfaces an error IN the dialog (ConfirmDialog\'s error prop) and keeps it open, instead of silently closing', async () => {
    vi.spyOn(api, 'deletePermanently').mockRejectedValue(new Error('disk permission denied'));
    await renderTrashSettled();
    await fireEvent.click(screen.getByText('Delete Permanently'));
    await screen.findByRole('dialog');

    await fireEvent.click(screen.getByRole('button', { name: 'Delete' }));

    // Dialog stays open (this is the behavior the pre-fix code lacked —
    // the old code's catch only console.error'd and the modal closed
    // unconditionally) AND the error text is visible in the UI.
    await vi.waitFor(() => {
      expect(screen.queryByRole('dialog')).toBeTruthy();
      expect(screen.getByText('disk permission denied')).toBeTruthy();
    });
  });

  it('Empty Trash (same shared confirm state machine) also goes through ConfirmDialog', async () => {
    await renderTrashSettled();
    await fireEvent.click(screen.getByText('Empty Trash'));

    const dialog = await screen.findByRole('dialog');
    expect(dialog).toBeTruthy();
    expect(screen.getByText('Empty Trash?')).toBeTruthy();
  });

  it('Escape key cancels the open confirm dialog without triggering delete', async () => {
    const deleteSpy = vi.spyOn(api, 'deletePermanently');
    await renderTrashSettled();
    await fireEvent.click(screen.getByText('Delete Permanently'));
    await screen.findByRole('dialog');

    await fireEvent.keyDown(document, { key: 'Escape' });

    await vi.waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());
    expect(deleteSpy).not.toHaveBeenCalled();
  });
});
