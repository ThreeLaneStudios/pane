/**
 * ScheduledPanel.newJobFooterCopy.test.ts — #3171 PR 2 (C43).
 *
 * The New job dialog's footer line already tracked `formName` LIVE (a
 * Svelte 5 `$state` referenced directly in markup is reactive by
 * construction — no snapshot/staleness bug). The actual defect was the
 * COPY: `formName || 'this job'` substituted the literal filler word
 * "this job" inside quotes when the field was empty, producing a false
 * claim ("...called "this job"." — the session will NOT literally be
 * named that). Fixed by restructuring the empty-state branch into its
 * own sentence (no quotes, no fake name) instead of a filler-word
 * substitution.
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
  return { projects: writable([]), activeChatThreadId: writable(null) };
});

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
  updateScheduledJob: vi.fn(),
  runJobNow: vi.fn(),
  switchSession: vi.fn(),
}));

beforeEach(() => {
  listScheduledJobsMock.mockReset().mockResolvedValue([]);
  getAgentsMock.mockReset().mockResolvedValue([{ id: 'agent-brit', name: 'Brit Freed' }]);
});

describe('#3171 PR 2 (C43) — New job dialog footer copy', () => {
  it('shows the honest no-fake-name sentence when the Name field is empty', async () => {
    const { getByText, container } = render(ScheduledPanel, {});
    await waitFor(() => expect(listScheduledJobsMock).toHaveBeenCalled());
    await fireEvent.click(getByText('New job'));

    const modal = container.querySelector('[data-testid="new-schedule-modal"]') as HTMLElement;
    expect(modal.querySelector('.sum')?.textContent).toBe('Runs go to a new session, named after the job.');
    // The old filler-word substitution must be gone entirely.
    expect(modal.textContent).not.toContain('this job"');
  });

  /// Named test (task item 14, "typing into Name updates the rendered
  /// footer text on every keystroke" — proves LIVE tracking, not just
  /// "the fallback renders once at mount").
  it('updates the footer live, character by character, as the Name field is typed', async () => {
    const { getByText, container } = render(ScheduledPanel, {});
    await waitFor(() => expect(listScheduledJobsMock).toHaveBeenCalled());
    await fireEvent.click(getByText('New job'));

    const modal = container.querySelector('[data-testid="new-schedule-modal"]') as HTMLElement;
    const nameInput = modal.querySelector('input[placeholder="Job name"]') as HTMLInputElement;
    const footer = () => modal.querySelector('.sum') as HTMLElement;

    await fireEvent.input(nameInput, { target: { value: 'M' } });
    expect(footer().textContent).toBe('Runs go to a new session called "M".');

    await fireEvent.input(nameInput, { target: { value: 'Mo' } });
    expect(footer().textContent).toBe('Runs go to a new session called "Mo".');

    await fireEvent.input(nameInput, { target: { value: 'Morning brief' } });
    expect(footer().textContent).toBe('Runs go to a new session called "Morning brief".');

    // Clearing it back out returns to the honest empty-state sentence,
    // not the old filler-word substitution.
    await fireEvent.input(nameInput, { target: { value: '' } });
    expect(footer().textContent).toBe('Runs go to a new session, named after the job.');
  });

  /// Mutation guard, named explicitly: hardcode the empty-state branch
  /// to a snapshot string regardless of `formName` (e.g. always render
  /// the string captured at the first keystroke) — reddens the
  /// character-by-character assertions above.
  it('mutation guard: a footer that only renders once at mount would fail the live-update assertions', async () => {
    const { getByText, container } = render(ScheduledPanel, {});
    await waitFor(() => expect(listScheduledJobsMock).toHaveBeenCalled());
    await fireEvent.click(getByText('New job'));

    const modal = container.querySelector('[data-testid="new-schedule-modal"]') as HTMLElement;
    const nameInput = modal.querySelector('input[placeholder="Job name"]') as HTMLInputElement;
    const footer = () => modal.querySelector('.sum') as HTMLElement;

    const atMount = footer().textContent;
    await fireEvent.input(nameInput, { target: { value: 'Changed Name' } });
    expect(footer().textContent).not.toBe(atMount);
    expect(footer().textContent).toBe('Runs go to a new session called "Changed Name".');
  });
});
