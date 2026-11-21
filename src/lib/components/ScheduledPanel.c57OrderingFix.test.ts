/**
 * ScheduledPanel.c57OrderingFix.test.ts — #3178 (C57, third attempt).
 *
 * Reed's live-DOM audit (#3178, issue comment): typing "ha" against
 * [Home, Harbor, Aurum, Hazel Notes] rendered, in DOM order, ["Home",
 * "Harbor", "Hazel Notes"] — Home pinned AND highlighted AND first,
 * while the thing the user actually typed sat second, unhighlighted.
 * Root cause: `formItemsUnsorted` (ScheduledPanel.svelte) performs no
 * re-sort at all; Home is first in the source array by construction
 * and stays there regardless of query.
 *
 * Fix: the Project field's own `formProjectSortItems` ranks a label
 * that actually matches the typed query ahead of an unmatched Home,
 * while an EMPTY query still shows Home first (unaffected — C56's
 * pin is delivered by `formProjectFilterItem`'s pass-through, which
 * this commit does not touch).
 *
 * Pure data/filter+sort, zero DOM layout measurement involved — unlike
 * C57's positioning half (no jsdom test for that; see the PR
 * description for why), this is directly unit-testable through the
 * rendered dropdown's item order.
 *
 * Matt's mutation: revert the sort change (back to `formItemsUnsorted`
 * on the Project field) → this test reddens (Home stays first).
 */
import { describe, it, expect, vi, beforeAll, beforeEach } from 'vitest';
import { render, fireEvent, waitFor, within } from '@testing-library/svelte/svelte5';
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

// Reed's exact repro set: [Home, Harbor, Aurum, Hazel Notes] — "ha"
// matches "Harbor" and "Hazel Notes" but not "Home" or "Aurum".
const { PROJECTS } = vi.hoisted(() => ({
  PROJECTS: [
    { id: 'proj-harbor', name: 'Harbor', parent_id: null },
    { id: 'proj-aurum', name: 'Aurum', parent_id: null },
    { id: 'proj-hazel', name: 'Hazel Notes', parent_id: null },
  ],
}));

vi.mock('$lib/stores/app', () => {
  const { writable } = require('svelte/store');
  return { projects: writable(PROJECTS), activeChatThreadId: writable(null) };
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
  getAgentsMock.mockReset().mockResolvedValue([]);
});

function dropdownItemLabels(container: HTMLElement) {
  return Array.from(container.querySelectorAll('.dropdown-item')).map((el) => el.textContent?.trim());
}

describe('#3178 (C57, third attempt) — Project field: typed match ranks ahead of pinned-but-unmatched Home', () => {
  it('typing "ha" shows Harbor and Hazel Notes (the typed matches) before Home, not Home first', async () => {
    const { getByText, container } = render(ScheduledPanel, {});
    await waitFor(() => expect(listScheduledJobsMock).toHaveBeenCalled());
    await fireEvent.click(getByText('New job'));

    const modal = container.querySelector('[data-testid="new-schedule-modal"]') as HTMLElement;
    const searchInput = within(modal).getByTestId('project-search') as HTMLInputElement;
    await fireEvent.focus(searchInput);
    await fireEvent.input(searchInput, { target: { value: 'ha' } });

    await waitFor(() => {
      const labels = dropdownItemLabels(modal);
      expect(labels).toContain('Home');
      expect(labels).toContain('Harbor');
      expect(labels).toContain('Hazel Notes');

      const homeIndex = labels.indexOf('Home');
      const harborIndex = labels.indexOf('Harbor');
      const hazelIndex = labels.indexOf('Hazel Notes');
      // The real complaint: the typed match must be first AND
      // highlighted-worthy (ranked ahead), not Home.
      expect(harborIndex).toBeLessThan(homeIndex);
      expect(hazelIndex).toBeLessThan(homeIndex);
    });

    // Aurum doesn't match "ha" at all — must be filtered out entirely
    // (unrelated to the ordering fix; confirms filterItem is untouched).
    await waitFor(() => {
      expect(dropdownItemLabels(modal)).not.toContain('Aurum');
    });
  });

  it('an empty query still shows Home first, unaffected (C56 intent preserved)', async () => {
    const { getByText, container } = render(ScheduledPanel, {});
    await waitFor(() => expect(listScheduledJobsMock).toHaveBeenCalled());
    await fireEvent.click(getByText('New job'));

    const modal = container.querySelector('[data-testid="new-schedule-modal"]') as HTMLElement;
    const searchInput = within(modal).getByTestId('project-search') as HTMLInputElement;
    await fireEvent.focus(searchInput);

    await waitFor(() => {
      expect(dropdownItemLabels(modal)[0]).toBe('Home');
    });
  });

  it('typing "home" still finds Home among the matches (the pin is not what ordering is testing — relevance is)', async () => {
    const { getByText, container } = render(ScheduledPanel, {});
    await waitFor(() => expect(listScheduledJobsMock).toHaveBeenCalled());
    await fireEvent.click(getByText('New job'));

    const modal = container.querySelector('[data-testid="new-schedule-modal"]') as HTMLElement;
    const searchInput = within(modal).getByTestId('project-search') as HTMLInputElement;
    await fireEvent.focus(searchInput);
    await fireEvent.input(searchInput, { target: { value: 'home' } });

    await waitFor(() => {
      expect(dropdownItemLabels(modal)).toEqual(['Home']);
    });
  });
});
