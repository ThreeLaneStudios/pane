/**
 * ScheduledPanel.stopRunningJob.test.ts — §36.3.7 carry-over (#3050 PR 9,
 * carried from PR 8's scope per Reed's 2026-10-29 PR 9 scope audit §6 —
 * unblocked once #3116/PR 8 merged).
 *
 * Pause/Delete on a RUNNING row used to be a flat, disabled refusal (the
 * backend `execute_cancel_job`/`execute_pause_job` guards reject a
 * `status='running'` row outright, and the buttons disabled themselves
 * client-side to match). This pins the new sequencing: the panel now
 * calls `waitForRunningJobToStop` FIRST, then the normal pause/delete
 * command — never an optimistic row flip while the job is still really
 * running.
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

function runningRow(overrides: Partial<any> = {}) {
  return {
    id: 'job-running-1',
    title: 'A running job',
    kind: 'recurring',
    agent: 'Agent A',
    project: 'Home',
    project_id: 'workspace',
    next_run_at: null,
    status: 'running',
    missed_count: 0,
    started_at: new Date().toISOString(),
    finished_at: null,
    last_error: null,
    session_id: 'session-running-1',
    ...overrides,
  };
}

const {
  listScheduledJobsMock,
  waitForRunningJobToStopMock,
  pauseScheduledJobMock,
  deleteScheduledJobMock,
  getAgentsMock,
} = vi.hoisted(() => ({
  listScheduledJobsMock: vi.fn(),
  waitForRunningJobToStopMock: vi.fn(),
  pauseScheduledJobMock: vi.fn(),
  deleteScheduledJobMock: vi.fn(),
  getAgentsMock: vi.fn(),
}));

vi.mock('$lib/api', () => ({
  listScheduledJobs: listScheduledJobsMock,
  waitForRunningJobToStop: waitForRunningJobToStopMock,
  pauseScheduledJob: pauseScheduledJobMock,
  resumeScheduledJob: vi.fn().mockResolvedValue(undefined),
  deleteScheduledJob: deleteScheduledJobMock,
  getAgents: getAgentsMock,
  createScheduledJob: vi.fn(),
  runJobNow: vi.fn(),
}));

beforeEach(() => {
  listScheduledJobsMock.mockReset().mockResolvedValue([runningRow()]);
  waitForRunningJobToStopMock.mockReset().mockResolvedValue(undefined);
  pauseScheduledJobMock.mockReset().mockResolvedValue(undefined);
  deleteScheduledJobMock.mockReset().mockResolvedValue(undefined);
  getAgentsMock.mockReset().mockResolvedValue([]);
});

describe('ScheduledPanel — Pause on a running row (§36.3.7 carry-over)', () => {
  /// Mutation: swap handlePauseResume's running-row branch to call
  /// `pauseScheduledJob` directly without awaiting `waitForRunningJobToStop`
  /// first — the ORDER assertion below (`waitForRunningJobToStop` called
  /// before `pauseScheduledJob`) fails, confirmed red and reverted.
  it('calls waitForRunningJobToStop BEFORE pauseScheduledJob', async () => {
    const { getByTestId } = render(ScheduledPanel, {});
    await waitFor(() => expect(listScheduledJobsMock).toHaveBeenCalled());

    const btn = await waitFor(() => getByTestId('pause-resume-btn'));
    await fireEvent.click(btn);

    await waitFor(() => {
      expect(waitForRunningJobToStopMock).toHaveBeenCalledWith('job-running-1');
      expect(pauseScheduledJobMock).toHaveBeenCalledWith('job-running-1');
    });

    const waitOrder = waitForRunningJobToStopMock.mock.invocationCallOrder[0];
    const pauseOrder = pauseScheduledJobMock.mock.invocationCallOrder[0];
    expect(waitOrder).toBeLessThan(pauseOrder);
  });
});

describe('ScheduledPanel — Delete on a running row (§36.3.7 carry-over)', () => {
  /// Mutation: same ordering check, for the delete path — swap the
  /// running-row branch to call `deleteScheduledJob` directly — red,
  /// reverted.
  it('calls waitForRunningJobToStop BEFORE deleteScheduledJob, after confirm', async () => {
    const { getByTestId, findByText } = render(ScheduledPanel, {});
    await waitFor(() => expect(listScheduledJobsMock).toHaveBeenCalled());

    const deleteBtn = await waitFor(() => getByTestId('delete-btn'));
    await fireEvent.click(deleteBtn);

    const confirmBtn = await findByText('Yes');
    await fireEvent.click(confirmBtn);

    await waitFor(() => {
      expect(waitForRunningJobToStopMock).toHaveBeenCalledWith('job-running-1');
      expect(deleteScheduledJobMock).toHaveBeenCalledWith('job-running-1');
    });

    const waitOrder = waitForRunningJobToStopMock.mock.invocationCallOrder[0];
    const deleteOrder = deleteScheduledJobMock.mock.invocationCallOrder[0];
    expect(waitOrder).toBeLessThan(deleteOrder);
  });

  /// Regression pin: a running row's Delete button now offers the
  /// confirm step (it used to be `disabled` and short-circuit before ever
  /// reaching it — #3050 PR 8 follow-up's prior behavior).
  it('offers the confirm step for a running row (no longer a flat disabled refusal)', async () => {
    const { getByTestId, findByText } = render(ScheduledPanel, {});
    await waitFor(() => expect(listScheduledJobsMock).toHaveBeenCalled());
    const deleteBtn = await waitFor(() => getByTestId('delete-btn'));
    await fireEvent.click(deleteBtn);
    await findByText('Yes');
  });
});

describe('ScheduledPanel — a non-running row never calls waitForRunningJobToStop', () => {
  /// Regression pin: the wait step is only for a RUNNING row — a pending
  /// row's Pause must go straight to pauseScheduledJob.
  it('pausing a pending row does not call waitForRunningJobToStop', async () => {
    listScheduledJobsMock.mockResolvedValue([runningRow({ status: 'pending', started_at: null })]);
    const { getByTestId } = render(ScheduledPanel, {});
    await waitFor(() => expect(listScheduledJobsMock).toHaveBeenCalled());
    const btn = await waitFor(() => getByTestId('pause-resume-btn'));
    await fireEvent.click(btn);

    await waitFor(() => {
      expect(pauseScheduledJobMock).toHaveBeenCalledWith('job-running-1');
    });
    expect(waitForRunningJobToStopMock).not.toHaveBeenCalled();
  });
});
