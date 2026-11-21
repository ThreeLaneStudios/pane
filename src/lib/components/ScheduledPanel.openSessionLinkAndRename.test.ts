/**
 * ScheduledPanel.openSessionLinkAndRename.test.ts — C7 + C21 (#3120 PR 3,
 * Reed's PR 3 brief).
 *
 * C7: "'Open session' link on Active cards too (Done has it; Active opens
 * on row click, undiscoverable)". Active rows already opened on a
 * whole-row click (`onclick={() => openRow(row)}`, unchanged by this fix)
 * but had no visible affordance saying so. Done rows already show a
 * "Open session" text link. This adds the identical link to Active rows —
 * purely a visual hint, no new click handler (the row's own existing
 * onclick already covers the whole row, including this new span).
 *
 * C21 (Matt's ruling): panel header "Scheduled" → "Jobs", button "+ New
 * schedule" → "+ New job" — exactly the two strings the ruling names, no
 * wider rename (the modal's own header, the empty-state copy, and the
 * "Couldn't load schedules" error text are untouched, not named by the
 * ruling).
 *
 * Same render/mock shape as `ScheduledPanel.doneTabNoActions.test.ts`.
 */

import { describe, it, expect, vi, beforeAll, beforeEach } from 'vitest';
import { render, waitFor } from '@testing-library/svelte/svelte5';
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

const { listScheduledJobsMock, getAgentsMock } = vi.hoisted(() => ({
  listScheduledJobsMock: vi.fn(),
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
  runJobNow: vi.fn(),
}));

beforeEach(() => {
  listScheduledJobsMock.mockReset().mockResolvedValue([activeRow()]);
  getAgentsMock.mockReset().mockResolvedValue([]);
});

describe('ScheduledPanel — panel rename (C21)', () => {
  /// Mutation: revert either string back to "Scheduled"/"New schedule" —
  /// reddens (the old literal reappears, the new one is absent).
  it('panel header reads "Jobs", button reads "New job" — not "Scheduled"/"New schedule"', async () => {
    const { container, getByText, queryByText } = render(ScheduledPanel, {});
    await waitFor(() => expect(listScheduledJobsMock).toHaveBeenCalled());

    expect(getByText('Jobs')).toBeTruthy();
    expect(getByText('New job', { exact: false })).toBeTruthy();
    expect(queryByText('Scheduled')).toBeNull();
    expect(container.textContent).not.toContain('New schedule');
  });

  /// C21 (#3128, Matt's widened ruling): this PR 3 test originally
  /// asserted "New schedule" was absent from `container.textContent`
  /// WITHOUT ever opening the modal (`{#if showModal}` — the modal's own
  /// `<h3>` was never rendered either way), so the assertion passed
  /// vacuously. #3128 names "the dialog is still titled 'New schedule'"
  /// explicitly — this closes that exact blind spot by actually opening
  /// the modal first. Mutation (actually run — see report): revert the
  /// modal's `<h3>New job</h3>` back to `<h3>New schedule</h3>` — reddens
  /// this time for real, since the modal is open and its h3 is rendered.
  it('the New job modal\'s own header also reads "New job", not "New schedule" (modal actually opened)', async () => {
    const { container, getByText } = render(ScheduledPanel, {});
    await waitFor(() => expect(listScheduledJobsMock).toHaveBeenCalled());

    getByText('New job', { exact: false }).click();

    await waitFor(() => {
      expect(container.querySelector('[data-testid="new-schedule-modal"]')).toBeTruthy();
    });
    const modal = container.querySelector('[data-testid="new-schedule-modal"]')!;
    expect(modal.querySelector('h3')?.textContent).toBe('New job');
    expect(modal.textContent).not.toContain('New schedule');
  });
});

describe('ScheduledPanel — Active rows show an "Open session" link too (C7)', () => {
  /// Mutation: remove the new `<span class="lnk">Open session</span>` from
  /// the Active-row render block — reddens (the text, and the `.lnk`
  /// element, are both absent from an Active row).
  it('an Active row renders the "Open session" hint alongside Next/Last', async () => {
    const { container, getByText } = render(ScheduledPanel, {});
    await waitFor(() => expect(listScheduledJobsMock).toHaveBeenCalled());
    await waitFor(() => expect(container.querySelector('.sc:not(.done)')).toBeTruthy());

    expect(getByText('Open session')).toBeTruthy();
    expect(container.querySelector('.sc:not(.done) .lnk')).toBeTruthy();
    // Regression guard: still no second/independent click handler — the
    // link has no onclick of its own, the row's existing onclick covers it.
    expect(container.querySelector('.sc:not(.done) .lnk')?.getAttribute('onclick')).toBeNull();
  });
});
