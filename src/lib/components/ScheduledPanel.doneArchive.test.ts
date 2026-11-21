/**
 * ScheduledPanel.doneArchive.test.ts — §3151 PR2/C24: the Done tab's own
 * "Show deleted" toggle and "Clear done" button.
 *
 * Same render/mock shape as `ScheduledPanel.doneTabNoActions.test.ts`
 * (`@testing-library/svelte/svelte5`, mock `@tauri-apps/api/event`/
 * `$lib/services/navigation`/`$lib/stores/app`).
 */

import { describe, it, expect, vi, beforeAll, beforeEach } from 'vitest';
import { render, fireEvent, waitFor } from '@testing-library/svelte/svelte5';
import ScheduledPanel from './ScheduledPanel.svelte';

beforeAll(() => {
  globalThis.ResizeObserver = class ResizeObserver {
    observe() {}
    unobserve() {}
    disconnect() {}
  };

  const win = globalThis.window as any;
  if (!win.addEventListener) {
    win.addEventListener = globalThis.addEventListener?.bind(globalThis) ?? (() => {});
  }
  if (!win.removeEventListener) {
    win.removeEventListener = globalThis.removeEventListener?.bind(globalThis) ?? (() => {});
  }
  if (!win.document) {
    win.document = globalThis.document;
  }
});

vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn().mockResolvedValue(() => {}),
}));

vi.mock('$lib/services/navigation', () => ({
  navigateTo: vi.fn(),
}));

vi.mock('$lib/stores/app', () => {
  const { writable } = require('svelte/store');
  return { projects: writable([]) };
});

function doneRow(overrides: Partial<any> = {}) {
  return {
    id: 'job-1',
    title: 'A finished job',
    kind: 'once',
    agent: 'Agent A',
    project: 'Home',
    project_id: 'workspace',
    next_run_at: null,
    status: 'done',
    missed_count: 0,
    started_at: new Date().toISOString(),
    finished_at: new Date().toISOString(),
    last_error: null,
    session_id: 'session-1',
    ...overrides,
  };
}

const { listScheduledJobsMock, clearDoneJobsMock, getAgentsMock } = vi.hoisted(() => ({
  listScheduledJobsMock: vi.fn(),
  clearDoneJobsMock: vi.fn(),
  getAgentsMock: vi.fn(),
}));

vi.mock('$lib/api', () => ({
  listScheduledJobs: listScheduledJobsMock,
  waitForRunningJobToStop: vi.fn().mockResolvedValue(undefined),
  pauseScheduledJob: vi.fn().mockResolvedValue(undefined),
  resumeScheduledJob: vi.fn().mockResolvedValue(undefined),
  deleteScheduledJob: vi.fn().mockResolvedValue(undefined),
  deleteArchivedJob: vi.fn().mockResolvedValue(undefined),
  clearDoneJobs: clearDoneJobsMock,
  getAgents: getAgentsMock,
  createScheduledJob: vi.fn(),
  runJobNow: vi.fn(),
}));

beforeEach(() => {
  listScheduledJobsMock.mockReset();
  getAgentsMock.mockReset().mockResolvedValue([]);
  clearDoneJobsMock.mockReset().mockResolvedValue(2);
});

describe('ScheduledPanel — "Show deleted" toggle (§3151 PR2/C24)', () => {
  /// Mutation: pass `doneAll` (unfiltered) straight to the render loop
  /// instead of `done` (= `visibleDoneRows(doneAll, showDeleted)`) —
  /// the cancelled row would render even with the toggle off, reddening
  /// this test.
  it('hides a cancelled row by default', async () => {
    listScheduledJobsMock.mockResolvedValue([
      doneRow({ id: 'job-done', status: 'done' }),
      doneRow({ id: 'job-cancelled', status: 'cancelled' }),
    ]);
    const { getByText, container } = render(ScheduledPanel, {});
    await waitFor(() => expect(listScheduledJobsMock).toHaveBeenCalled());

    const doneTab = await waitFor(() => getByText('Done').closest('span')!);
    await fireEvent.click(doneTab);

    await waitFor(() => expect(container.querySelectorAll('.sc.done').length).toBeGreaterThan(0));
    expect(container.querySelectorAll('.sc.done')).toHaveLength(1);
    expect(container.querySelector('[data-job-id="job-cancelled"]')).toBeNull();
  });

  /// Mutation: hardcode the checkbox's `checked`/`onchange` to a no-op
  /// (never actually flip `showDeleted`) — the cancelled row would
  /// never reappear after the click, reddening this test.
  it('reveals the cancelled row after checking "Show deleted"', async () => {
    listScheduledJobsMock.mockResolvedValue([
      doneRow({ id: 'job-done', status: 'done' }),
      doneRow({ id: 'job-cancelled', status: 'cancelled' }),
    ]);
    const { getByText, container, getByTestId } = render(ScheduledPanel, {});
    await waitFor(() => expect(listScheduledJobsMock).toHaveBeenCalled());

    const doneTab = await waitFor(() => getByText('Done').closest('span')!);
    await fireEvent.click(doneTab);
    await waitFor(() => expect(container.querySelectorAll('.sc.done').length).toBeGreaterThan(0));

    const toggle = getByTestId('show-deleted-toggle');
    await fireEvent.click(toggle);

    await waitFor(() => expect(container.querySelectorAll('.sc.done')).toHaveLength(2));
    expect(container.querySelector('[data-job-id="job-cancelled"]')).toBeTruthy();
  });
});

describe('ScheduledPanel — "Clear done" (§3151 PR2/C24)', () => {
  /// Mutation: have the button call `api.deleteScheduledJob` per-row in
  /// a loop instead of the one `api.clearDoneJobs()` call — reddens
  /// (the mock below is never invoked, or invoked with the wrong shape).
  it('clicking Clear done calls api.clearDoneJobs and reloads the list', async () => {
    listScheduledJobsMock
      .mockResolvedValueOnce([doneRow({ id: 'job-done', status: 'done' })])
      .mockResolvedValueOnce([]);
    const { getByText, getByTestId } = render(ScheduledPanel, {});
    await waitFor(() => expect(listScheduledJobsMock).toHaveBeenCalledTimes(1));

    const doneTab = await waitFor(() => getByText('Done').closest('span')!);
    await fireEvent.click(doneTab);

    const clearBtn = getByTestId('clear-done-btn');
    await fireEvent.click(clearBtn);

    await waitFor(() => expect(clearDoneJobsMock).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(listScheduledJobsMock).toHaveBeenCalledTimes(2));
  });

  /// Mutation: drop the `doneAll.length === 0` guard from
  /// `handleClearDone`/the button's `disabled` binding — reddens (the
  /// button would be clickable, and clicking it with nothing to clear
  /// would still call the backend command needlessly).
  it('Clear done is disabled when there is nothing on the Done tab', async () => {
    // Empty-jobs state shows the preset cards, not tabs — seed one
    // ACTIVE row so the tabbed view renders with an empty Done tab.
    listScheduledJobsMock.mockResolvedValue([doneRow({ id: 'job-active', status: 'pending', finished_at: null })]);
    const { getByText, getByTestId } = render(ScheduledPanel, {});
    await waitFor(() => expect(listScheduledJobsMock).toHaveBeenCalled());

    const doneTab = await waitFor(() => getByText('Done').closest('span')!);
    await fireEvent.click(doneTab);

    const clearBtn = getByTestId('clear-done-btn');
    expect((clearBtn as HTMLButtonElement).disabled).toBe(true);
  });
});
