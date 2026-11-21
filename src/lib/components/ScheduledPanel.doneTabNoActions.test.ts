/**
 * ScheduledPanel.doneTabNoActions.test.ts — C11 (#3120 PR 2, Matt's ruling):
 * "Done cards render no action buttons at all. Just Open session."
 *
 * **SUPERSEDED for Done cards by §3151 PR2/C24** (Matt's later ruling,
 * #3151's own acceptance text: "delete removes the session"): a Delete
 * action is back on every Done row, regardless of `kind` — but it is NOT
 * the old `kind === 'once' && (done||failed)` branch C11 removed (that one
 * called `execute_cancel_job`, soft `status='cancelled'`). The new one
 * calls `api.deleteArchivedJob` — a HARD delete of the job row AND its own
 * session, offered on every Done row (not kind-gated). This file's own
 * tests are inverted accordingly, per the dated-ruling convention: C11
 * stays correct-as-written for its own PR; this later PR narrows its
 * scope to "no OTHER action buttons" (pause/resume/edit/run-now stay
 * absent) while adding the one Delete button back.
 *
 * Same render/mock shape as `ScheduledPanel.stopRunningJob.test.ts`
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

function doneOnceRow(overrides: Partial<any> = {}) {
  return {
    id: 'job-done-1',
    title: 'A finished once-job',
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
    session_id: 'session-done-1',
    ...overrides,
  };
}

const { listScheduledJobsMock, getAgentsMock } = vi.hoisted(() => ({
  listScheduledJobsMock: vi.fn(),
  getAgentsMock: vi.fn(),
}));

const { deleteArchivedJobMock, clearDoneJobsMock } = vi.hoisted(() => ({
  deleteArchivedJobMock: vi.fn(),
  clearDoneJobsMock: vi.fn(),
}));

vi.mock('$lib/api', () => ({
  listScheduledJobs: listScheduledJobsMock,
  waitForRunningJobToStop: vi.fn().mockResolvedValue(undefined),
  pauseScheduledJob: vi.fn().mockResolvedValue(undefined),
  resumeScheduledJob: vi.fn().mockResolvedValue(undefined),
  deleteScheduledJob: vi.fn().mockResolvedValue(undefined),
  deleteArchivedJob: deleteArchivedJobMock,
  clearDoneJobs: clearDoneJobsMock,
  getAgents: getAgentsMock,
  createScheduledJob: vi.fn(),
  runJobNow: vi.fn(),
}));

beforeEach(() => {
  listScheduledJobsMock.mockReset().mockResolvedValue([doneOnceRow()]);
  getAgentsMock.mockReset().mockResolvedValue([]);
  deleteArchivedJobMock.mockReset().mockResolvedValue(undefined);
  clearDoneJobsMock.mockReset().mockResolvedValue(0);
});

describe('ScheduledPanel — Done tab renders ONLY the Delete action (C11 superseded by §3151 PR2/C24)', () => {
  /// Mutation: remove the new Delete `{#if confirmDeleteDoneId ===
  /// row.id}...{:else}<button data-testid="delete-btn">` block from the
  /// Done-tab render — `delete-btn` disappears, reddening this test
  /// (the "deleted job + session, from any Done row" acceptance item has
  /// no entry point left).
  it('a done once-job row has exactly one action button (Delete) and no others', async () => {
    const { getByText, queryByTestId, container } = render(ScheduledPanel, {});
    await waitFor(() => expect(listScheduledJobsMock).toHaveBeenCalled());

    // Switch to the Done tab.
    const doneTab = await waitFor(() => getByText('Done').closest('span')!);
    await fireEvent.click(doneTab);

    await waitFor(() => expect(container.querySelector('.sc.done')).toBeTruthy());

    expect(queryByTestId('delete-btn')).toBeTruthy();
    expect(container.querySelectorAll('.sc.done button')).toHaveLength(1);
    expect(container.querySelector('.sc.done [data-testid="edit-btn"]')).toBeNull();
    expect(container.querySelector('.sc.done [data-testid="pause-resume-btn"]')).toBeNull();
    expect(getByText('Open session')).toBeTruthy();
  });

  /// Same proof for a `failed` once-job row.
  it('a failed once-job row also has exactly one action button (Delete)', async () => {
    listScheduledJobsMock.mockResolvedValue([doneOnceRow({ id: 'job-failed-1', status: 'failed', last_error: 'boom' })]);
    const { getByText, container } = render(ScheduledPanel, {});
    await waitFor(() => expect(listScheduledJobsMock).toHaveBeenCalled());

    const doneTab = await waitFor(() => getByText('Done').closest('span')!);
    await fireEvent.click(doneTab);

    await waitFor(() => expect(container.querySelector('.sc.done')).toBeTruthy());
    expect(container.querySelectorAll('.sc.done button')).toHaveLength(1);
  });

  /// Named test: clicking Delete → Yes calls `api.deleteArchivedJob`
  /// (the HARD delete), never `api.deleteScheduledJob` (the Active tab's
  /// soft-cancel). Mutation: wire the Done card's Delete button to
  /// `requestDelete`/`confirmDelete` (the Active tab's own functions)
  /// instead of the dedicated `requestDeleteDone`/`confirmDeleteDoneRow`
  /// — reddens (the wrong mock gets called, or the wrong confirm-id
  /// state gates the Yes/No render).
  it('Delete -> Yes calls deleteArchivedJob, not deleteScheduledJob', async () => {
    const { getByText, getByTitle, container } = render(ScheduledPanel, {});
    await waitFor(() => expect(listScheduledJobsMock).toHaveBeenCalled());

    const doneTab = await waitFor(() => getByText('Done').closest('span')!);
    await fireEvent.click(doneTab);
    await waitFor(() => expect(container.querySelector('.sc.done')).toBeTruthy());

    await fireEvent.click(getByTitle('Delete (removes job and session)'));
    await fireEvent.click(getByText('Yes'));

    await waitFor(() => expect(deleteArchivedJobMock).toHaveBeenCalledWith('job-done-1'));
  });
});
