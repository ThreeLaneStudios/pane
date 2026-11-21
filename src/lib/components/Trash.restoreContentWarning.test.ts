/**
 * Trash.restoreContentWarning.test.ts — #2874 item 1, Matt's ruling 5 ("no
 * recovery tool ... tell the user plainly the content was lost before this
 * fix" / "rebuild from note_versions when a snapshot exists").
 *
 * The backend's `restore_note` command now returns `{ meta, content_warning }`
 * instead of a bare `NoteMeta` — `content_warning` is set whenever the
 * note's file was missing at restore time (rebuilt from a saved
 * `note_versions` snapshot, or lost entirely with no snapshot). That signal
 * must reach the user, never a silent, normal-looking restore.
 * `api.restoreFromTrash` (api.ts) already maps `content_warning` ->
 * `contentWarning`; this test proves `Trash.svelte`'s `handleRestore`
 * actually surfaces it (via `alert`, the same mechanism already used for
 * this screen's other user-facing failure message) and does NOT show
 * anything when no warning is present.
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
  title: 'Missing File Note',
  notebook: 'Notes',
  slug: 'missing-file-note',
  deleted_at: '2026-10-01T00:00:00Z',
  original_path: 'Notes/missing-file-note.md',
};

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

async function renderTrashSettled() {
  vi.spyOn(api, 'listTrash').mockResolvedValue([TRASH_ITEM]);
  render(Trash, { props: { onClose: () => {} } });
  await screen.findByText('Missing File Note');
  return;
}

describe('Trash restore content_warning surfacing (#2874 item 1, ruling 5)', () => {
  it('shows the backend content_warning via alert when restore rebuilt from a snapshot', async () => {
    const alertSpy = vi.fn();
    vi.stubGlobal('alert', alertSpy);
    vi.spyOn(api, 'restoreFromTrash').mockResolvedValue({
      contentWarning:
        "This note's file was missing when it was restored. It has been rebuilt from a saved version from 2026-10-10T00:00:00Z — any edits made after that version are not included.",
    });

    await renderTrashSettled();
    const restoreButton = screen.getByText('Restore');
    await fireEvent.click(restoreButton);

    await vi.waitFor(() => expect(alertSpy).toHaveBeenCalledTimes(1));
    expect(alertSpy).toHaveBeenCalledWith(expect.stringContaining('rebuilt from a saved version'));
  });

  it('shows the backend content_warning via alert when content was lost with no snapshot', async () => {
    const alertSpy = vi.fn();
    vi.stubGlobal('alert', alertSpy);
    vi.spyOn(api, 'restoreFromTrash').mockResolvedValue({
      contentWarning:
        "This note's file was missing when it was restored, and no saved version exists to recover it from. The content was lost before this fix.",
    });

    await renderTrashSettled();
    const restoreButton = screen.getByText('Restore');
    await fireEvent.click(restoreButton);

    await vi.waitFor(() => expect(alertSpy).toHaveBeenCalledTimes(1));
    expect(alertSpy).toHaveBeenCalledWith(expect.stringContaining('content was lost'));
  });

  it('does NOT alert when restore succeeds with no content_warning (file was present)', async () => {
    const alertSpy = vi.fn();
    vi.stubGlobal('alert', alertSpy);
    vi.spyOn(api, 'restoreFromTrash').mockResolvedValue({ contentWarning: undefined });

    await renderTrashSettled();
    const restoreButton = screen.getByText('Restore');
    await fireEvent.click(restoreButton);

    await new Promise((r) => setTimeout(r, 50));
    expect(alertSpy).not.toHaveBeenCalled();
  });
});
