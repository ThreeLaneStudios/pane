/**
 * ScheduledPanel.runNowError.test.ts — C31 UI half (#3128).
 *
 * Reed's audit: `handleRunNow`'s catch block swallowed a rejection from
 * `runJobNow` (e.g. the documented "Run now is disabled while a run is
 * already in flight" refusal) to `console.error` only — nothing in the
 * DOM changed, no toast, no inline message. This pins the fix: a visible,
 * row-level error element appears with the rejection's own message text,
 * reusing the modal's existing `sp-status`/`sp-status--error` visual
 * language rather than inventing new UI.
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

function activeRow(overrides: Partial<any> = {}) {
  return {
    id: 'job-active-1',
    title: 'Morning brief',
    kind: 'recurring',
    agent: 'Brit Freed',
    project: 'Home',
    project_id: 'workspace',
    next_run_at: new Date(Date.now() + 3600_000).toISOString(),
    status: 'pending',
    missed_count: 0,
    started_at: null,
    finished_at: null,
    last_error: null,
    session_id: 'session-active-1',
    ...overrides,
  };
}

const { listScheduledJobsMock, runJobNowMock, getAgentsMock } = vi.hoisted(() => ({
  listScheduledJobsMock: vi.fn(),
  runJobNowMock: vi.fn(),
  getAgentsMock: vi.fn(),
}));

vi.mock('$lib/api', () => ({
  listScheduledJobs: listScheduledJobsMock,
  waitForRunningJobToStop: vi.fn().mockResolvedValue(undefined),
  pauseScheduledJob: vi.fn().mockResolvedValue(undefined),
  resumeScheduledJob: vi.fn().mockResolvedValue(undefined),
  deleteScheduledJob: vi.fn().mockResolvedValue(undefined),
  getAgents: getAgentsMock,
  createScheduledJob: vi.fn(),
  runJobNow: runJobNowMock,
}));

beforeEach(() => {
  listScheduledJobsMock.mockReset().mockResolvedValue([activeRow()]);
  getAgentsMock.mockReset().mockResolvedValue([]);
  runJobNowMock.mockReset();
});

describe('ScheduledPanel — Run now refusal surfaces a visible row error (C31 UI half, #3128)', () => {
  /// Mutation (actually run — see report): revert `handleRunNow`'s catch
  /// block to `console.error(...)`-only — reddens (no
  /// `[data-testid="run-now-error"]` element is ever rendered; the test's
  /// `waitFor` times out).
  it('shows the rejection message inline when runJobNow rejects', async () => {
    runJobNowMock.mockRejectedValue(new Error('Run now is disabled while a run is already in flight'));
    const { container, getAllByTitle } = render(ScheduledPanel, {});
    await waitFor(() => expect(listScheduledJobsMock).toHaveBeenCalled());

    const runNowBtn = await waitFor(() => getAllByTitle('Run now')[0]);
    await fireEvent.click(runNowBtn);

    await waitFor(() => {
      const err = container.querySelector('[data-testid="run-now-error"][data-job-id="job-active-1"]');
      expect(err).toBeTruthy();
      expect(err?.textContent).toContain('Run now is disabled while a run is already in flight');
    });
  });

  /// Regression guard: a SUCCESSFUL runJobNow call must never show the
  /// error element — proves the new state isn't rendered unconditionally.
  it('does NOT show a row error when runJobNow succeeds', async () => {
    runJobNowMock.mockResolvedValue(undefined);
    const { container, getAllByTitle } = render(ScheduledPanel, {});
    await waitFor(() => expect(listScheduledJobsMock).toHaveBeenCalled());

    const runNowBtn = await waitFor(() => getAllByTitle('Run now')[0]);
    await fireEvent.click(runNowBtn);

    await waitFor(() => expect(runJobNowMock).toHaveBeenCalled());
    await new Promise((r) => setTimeout(r, 0));
    expect(container.querySelector('[data-testid="run-now-error"]')).toBeNull();
  });
});
