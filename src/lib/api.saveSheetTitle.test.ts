/**
 * Tests for #2874 item 17 — Sheet title rename not persisting
 *
 * The bug: `saveNote()` always bundles a `content` field alongside `title`
 * in its `update_note` request. For sheet notes, the sheet editor has no
 * TipTap instance, so any `content` it would send is stale/regenerated
 * body content — exactly the write the #2901 sheet-body guard
 * (`apply_note_content_update`, notes.rs:1520-1528) exists to reject. That
 * guard's `Err` propagates via `?` and blocks the title write bundled in
 * the same call, as collateral damage.
 *
 * Fix: a dedicated `saveSheetTitle()` function sends a request with ONLY
 * `note_id` and `title` — no `content` key at all (not `content: undefined`,
 * literally absent from the object) — so the guard is never triggered.
 *
 * Red-on-revert: if `saveSheetTitle` is changed to reuse `saveNote` or to
 * otherwise include a `content` key in its request, these tests fail.
 */

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { saveSheetTitle } from '$lib/api';
import * as tauriCore from '@tauri-apps/api/core';

describe('saveSheetTitle — invoke parameter shape (item 17 fix)', () => {
  let invokeSpy: ReturnType<typeof vi.spyOn>;

  beforeEach(() => {
    invokeSpy = vi.spyOn(tauriCore, 'invoke').mockResolvedValue({ id: 'note-1', title: 'New Title' });
  });

  afterEach(() => {
    invokeSpy.mockRestore();
  });

  it('sends title only — request has NO content key', async () => {
    await saveSheetTitle('ProjectA', 'note-1', 'New Title');

    expect(invokeSpy).toHaveBeenCalledTimes(1);
    const callArgs = invokeSpy.mock.calls[0];
    expect(callArgs[0]).toBe('update_note');
    const { request } = callArgs[1] as any;
    expect(request).not.toHaveProperty('content');
    expect(request.title).toBe('New Title');
    expect(request.note_id).toBe('note-1');
  });

  it('returns the result from invoke and dispatches pane-data-changed', async () => {
    const dispatchSpy = vi.spyOn(window, 'dispatchEvent');
    const result = await saveSheetTitle('ProjectA', 'note-2', 'Another Title');

    expect(result).toEqual({ id: 'note-1', title: 'New Title' });
    expect(dispatchSpy).toHaveBeenCalled();
    const dispatchedEvent = dispatchSpy.mock.calls
      .map((c) => c[0])
      .find((e) => e instanceof CustomEvent && (e as CustomEvent).type === 'pane-data-changed');
    expect(dispatchedEvent).toBeTruthy();
    dispatchSpy.mockRestore();
  });

  it('propagates errors from invoke (no silent failure)', async () => {
    invokeSpy.mockRejectedValueOnce(new Error('backend refused'));
    await expect(saveSheetTitle('ProjectA', 'note-3', 'Bad Title')).rejects.toThrow('backend refused');
  });
});
