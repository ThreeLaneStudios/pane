/**
 * api.categoryRefresh.test.ts — #3018 (`docs/spec/data-refresh.md` §34,
 * "category as a fifth entity_type").
 *
 * The bug: renaming, recoloring, or deleting a category in Settings didn't
 * refresh the sidebar. Rename/delete REGRESSED in #2972 Stage 2: both kept
 * their old bare `window.dispatchEvent(new CustomEvent('pane-data-changed'))`
 * dispatch (no `detail` at all) from before the single-event migration, and
 * Stage 3's `subscribeDataChanged()` DOM handler no-ops on a missing
 * `detail` — so once Stage 3 landed, these two calls stopped refreshing
 * ANYTHING, anywhere, silently. Recolor (`setCategoryColor` for the
 * color-only-update path) never dispatched anything at all, at any stage —
 * it never refreshed, period.
 *
 * Fix: `category` is a fifth `entity_type` (`entity_id` = the category
 * name). `setCategoryColor()` now dispatches one `category` event itself
 * (reason supplied by the caller — `created` for add, `edited` for
 * recolor). `renameCategory()`/`deleteCategory()` each dispatch one
 * `category` event (`edited`/`deleted`) PLUS one `project`/`edited` event
 * per project ID the backend command returns as affected by its own bulk
 * `UPDATE projects.category` — zero project events when zero projects were
 * affected, not skipped/omitted.
 *
 * Verified failing against pre-fix code (manually, since the pre-fix
 * functions returned `Promise<void>` not `Promise<string[]>` and never
 * called `dispatchDataChanged` at all):
 *   - `setCategoryColor` pre-fix: `invoke('set_category_color', ...)` only,
 *     no dispatch of any kind → `categoryEvents` below would see 0 calls,
 *     not 1 — these tests fail against that code.
 *   - `renameCategory`/`deleteCategory` pre-fix: one bare
 *     `window.dispatchEvent(new CustomEvent('pane-data-changed'))` (no
 *     `detail`) → `subscribeDataChanged`'s DOM handler
 *     (`refreshEvent.ts`) no-ops on missing `detail`, so `categoryEvents`
 *     and `projectEvents` below would BOTH see 0 calls — these tests fail
 *     against that code too.
 */

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { setCategoryColor, renameCategory, deleteCategory } from '$lib/api';
import * as tauriCore from '@tauri-apps/api/core';
import {
  subscribeDataChanged,
  REASON_CREATED,
  type DataChangedPayload,
} from '$lib/refreshEvent';

const win = globalThis.window as any;
win.addEventListener = globalThis.addEventListener.bind(globalThis);
win.removeEventListener = globalThis.removeEventListener.bind(globalThis);
win.dispatchEvent = globalThis.dispatchEvent.bind(globalThis);

describe('api.ts category functions — exact refresh-event counts (#3018)', () => {
  let invokeSpy: ReturnType<typeof vi.spyOn>;
  let categoryEvents: ReturnType<typeof vi.fn>;
  let projectEvents: ReturnType<typeof vi.fn>;
  let unlisten: () => void;

  beforeEach(async () => {
    categoryEvents = vi.fn();
    projectEvents = vi.fn();
    const unlistenPromise = subscribeDataChanged((payload: DataChangedPayload) => {
      if (payload.entity_type === 'category') categoryEvents(payload);
      if (payload.entity_type === 'project') projectEvents(payload);
    });
    unlisten = await unlistenPromise;
  });

  afterEach(() => {
    unlisten();
    invokeSpy?.mockRestore();
  });

  describe('add (setCategoryColor, reason: "created")', () => {
    beforeEach(() => {
      invokeSpy = vi.spyOn(tauriCore, 'invoke').mockResolvedValue(undefined);
    });

    it('fires exactly one category/created event and zero project events', async () => {
      await setCategoryColor('Work', '#00E4E4', REASON_CREATED);

      expect(categoryEvents).toHaveBeenCalledTimes(1);
      expect(categoryEvents).toHaveBeenCalledWith(
        expect.objectContaining({ entity_id: 'Work', entity_type: 'category', reason: 'created' }),
      );
      expect(projectEvents).toHaveBeenCalledTimes(0);
    });
  });

  describe('recolor (setCategoryColor, default reason: "edited")', () => {
    beforeEach(() => {
      invokeSpy = vi.spyOn(tauriCore, 'invoke').mockResolvedValue(undefined);
    });

    it('fires exactly one category/edited event and zero project events', async () => {
      await setCategoryColor('Work', '#FF0000');

      expect(categoryEvents).toHaveBeenCalledTimes(1);
      expect(categoryEvents).toHaveBeenCalledWith(
        expect.objectContaining({ entity_id: 'Work', entity_type: 'category', reason: 'edited' }),
      );
      expect(projectEvents).toHaveBeenCalledTimes(0);
    });
  });

  describe('rename (renameCategory)', () => {
    it('with affected projects: fires exactly one category/edited event plus one project/edited event PER affected project', async () => {
      invokeSpy = vi.spyOn(tauriCore, 'invoke').mockResolvedValue(['proj-1', 'proj-2']);

      await renameCategory('Work', 'Office', '#00E4E4');

      expect(invokeSpy).toHaveBeenCalledWith('rename_category', {
        oldCategory: 'Work',
        newCategory: 'Office',
        color: '#00E4E4',
      });

      expect(categoryEvents).toHaveBeenCalledTimes(1);
      expect(categoryEvents).toHaveBeenCalledWith(
        expect.objectContaining({ entity_id: 'Office', entity_type: 'category', reason: 'edited' }),
      );

      expect(projectEvents).toHaveBeenCalledTimes(2);
      expect(projectEvents).toHaveBeenCalledWith(
        expect.objectContaining({ entity_id: 'proj-1', entity_type: 'project', reason: 'edited' }),
      );
      expect(projectEvents).toHaveBeenCalledWith(
        expect.objectContaining({ entity_id: 'proj-2', entity_type: 'project', reason: 'edited' }),
      );
    });

    it('with zero affected projects: fires exactly one category/edited event and EXACTLY ZERO project events (not skipped — asserted)', async () => {
      invokeSpy = vi.spyOn(tauriCore, 'invoke').mockResolvedValue([]);

      await renameCategory('Lonely', 'StillLonely', '#AABBCC');

      expect(categoryEvents).toHaveBeenCalledTimes(1);
      expect(projectEvents).toHaveBeenCalledTimes(0);
    });
  });

  describe('delete (deleteCategory)', () => {
    it('with affected projects: fires exactly one category/deleted event plus one project/edited event PER affected project', async () => {
      invokeSpy = vi.spyOn(tauriCore, 'invoke').mockResolvedValue(['proj-9']);

      await deleteCategory('Archive');

      expect(invokeSpy).toHaveBeenCalledWith('delete_category', { category: 'Archive' });

      expect(categoryEvents).toHaveBeenCalledTimes(1);
      expect(categoryEvents).toHaveBeenCalledWith(
        expect.objectContaining({ entity_id: 'Archive', entity_type: 'category', reason: 'deleted' }),
      );

      expect(projectEvents).toHaveBeenCalledTimes(1);
      expect(projectEvents).toHaveBeenCalledWith(
        expect.objectContaining({ entity_id: 'proj-9', entity_type: 'project', reason: 'edited' }),
      );
    });

    it('with zero affected projects: fires exactly one category/deleted event and EXACTLY ZERO project events', async () => {
      invokeSpy = vi.spyOn(tauriCore, 'invoke').mockResolvedValue([]);

      await deleteCategory('Unused');

      expect(categoryEvents).toHaveBeenCalledTimes(1);
      expect(categoryEvents).toHaveBeenCalledWith(
        expect.objectContaining({ entity_id: 'Unused', entity_type: 'category', reason: 'deleted' }),
      );
      expect(projectEvents).toHaveBeenCalledTimes(0);
    });
  });
});
