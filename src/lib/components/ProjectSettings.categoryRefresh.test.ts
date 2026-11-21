/**
 * ProjectSettings.categoryRefresh.test.ts — #2972 Stage 4 (`docs/spec/
 * data-refresh.md` §34, inventory + cross-screen verification pass).
 *
 * Finding (inventory, §34 Stage 4): `ProjectSettings.svelte`'s Category
 * dropdown (`categories`, loaded via `loadCategories()` → raw
 * `invoke('list_categories')`) goes stale if a category is renamed/
 * recolored/deleted elsewhere while this form is open. Not one of
 * §34.12's nine screens, never migrated by any prior #2972 stage.
 *
 * Deliberately scoped to ONLY the category dropdown — NOT the form's own
 * `category`/`projectName`/`projectBrief` fields, which track
 * `hasChanges`/`originalValues` dirty-form state; silently overwriting
 * those from an incoming `project` event mid-edit would clobber unsaved
 * input, a worse regression than the staleness bug being fixed (see the
 * code comment at the fix site for the full rationale).
 *
 * Measured directly against pre-fix `ProjectSettings.svelte` (mount
 * settles, spy cleared, event dispatched, `vi.waitFor`):
 *
 *   category event -> list_categories invoke calls: 0 additional
 *
 * This test fails (expect >=1, get 0 additional) against that code — see
 * the sibling fix commit's PR description for the exact `pnpm vitest run`
 * before/after output.
 *
 * Fix: a `subscribeDataChanged` listener scoped to `entity_type ===
 * 'category'` re-calls `loadCategories()`.
 */

import { describe, it, expect, vi, afterEach } from 'vitest';
import { render, cleanup, screen } from '@testing-library/svelte/svelte5';
import ProjectSettings from './ProjectSettings.svelte';
import { simulateTauriEvent } from '../../../vitest-setup';
import { EVENT_DATA_CHANGED, ENTITY_CATEGORY, REASON_EDITED } from '$lib/refreshEvent';

const { invokeCallsLog } = vi.hoisted(() => ({ invokeCallsLog: [] as string[] }));

vi.mock('@tauri-apps/api/core', async (importOriginal) => {
  const original = (await importOriginal()) as any;
  return {
    ...original,
    invoke: async (cmd: string, args?: any) => {
      invokeCallsLog.push(cmd);
      if (cmd === 'list_categories') {
        return [{ category: 'Work', color: '#AABBCC' }];
      }
      return original.invoke(cmd, args);
    },
  };
});

vi.mock('$lib/api', async (importOriginal) => {
  const original = (await importOriginal()) as any;
  return {
    ...original,
    getAgents: vi.fn().mockResolvedValue([]),
    listRuntimeConnections: vi.fn().mockResolvedValue([]),
    listProjects: vi.fn().mockResolvedValue([]),
  };
});

const win = globalThis.window as any;
if (typeof win.addEventListener !== 'function') {
  win.addEventListener = globalThis.addEventListener?.bind(globalThis) ?? (() => {});
}
if (typeof win.removeEventListener !== 'function') {
  win.removeEventListener = globalThis.removeEventListener?.bind(globalThis) ?? (() => {});
}

afterEach(() => {
  cleanup();
  invokeCallsLog.length = 0;
});

describe('ProjectSettings pane-data-changed handler — category dropdown refresh (#2972 stage 4)', () => {
  it('a category event re-reads the category list (list_categories invoked again)', async () => {
    render(ProjectSettings, { props: {} });
    await vi.waitFor(() => expect(invokeCallsLog.filter((c) => c === 'list_categories').length).toBeGreaterThanOrEqual(1));
    invokeCallsLog.length = 0;

    simulateTauriEvent(EVENT_DATA_CHANGED, {
      entity_id: 'Office',
      entity_type: ENTITY_CATEGORY,
      reason: REASON_EDITED,
      origin_tag: 'ps-test-category',
    });

    await vi.waitFor(() => expect(invokeCallsLog.filter((c) => c === 'list_categories').length).toBeGreaterThanOrEqual(1));
  });
});
