/**
 * ScheduledPanel.newJobFieldsInline.test.ts — #3178 (C57, rewrite,
 * Matt's ruling).
 *
 * Matt: "A prop on SearchableSelect — inline mode... Not a sibling
 * component." The New job dialog's Project and Agent fields are the
 * two callers this prop was added FOR (five prior passes all targeted
 * exactly these two fields' positioning — see SearchableSelect.svelte's
 * own `inline` prop doc comment and SearchableSelect.inlineMode.test.ts
 * for the generic mechanics). This file is the wiring check: does
 * ScheduledPanel's dialog actually pass `inline` to both, so the
 * dropdown that renders is the in-flow one (no `position: fixed`, no
 * `data-position`) and not the floating one a regression could
 * silently reintroduce by dropping the prop from one call site.
 *
 * Mutation (actually run): removing `inline` from the Project
 * `<SearchableSelect>` call site in ScheduledPanel.svelte reddens the
 * first test below (`data-position` becomes present, `.dropdown-list-
 * inline` becomes absent) while leaving the second (Agent) test green
 * — confirming the two fields are checked independently, not by one
 * assertion that could pass by accident if only one regressed.
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

const { PROJECTS } = vi.hoisted(() => ({
  PROJECTS: [{ id: 'proj-aurum', name: 'Aurum', parent_id: null }],
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
  getAgentsMock.mockReset().mockResolvedValue([{ id: 'agent-brit', name: 'Brit Freed' }]);
});

describe('#3178 (C57, rewrite) — New job dialog Project/Agent fields render inline, not floating', () => {
  it('Project field: dropdown has no data-position / fixed-position styling, carries .dropdown-list-inline', async () => {
    const { getByText, container } = render(ScheduledPanel, {});
    await waitFor(() => expect(listScheduledJobsMock).toHaveBeenCalled());
    await fireEvent.click(getByText('New job'));

    const modal = container.querySelector('[data-testid="new-schedule-modal"]') as HTMLElement;
    const searchInput = within(modal).getByTestId('project-search') as HTMLInputElement;
    await fireEvent.focus(searchInput);

    const list = modal.querySelector('.dropdown-list') as HTMLElement;
    expect(list).toBeTruthy();
    expect(list.classList.contains('dropdown-list-inline')).toBe(true);
    expect(list.hasAttribute('data-position')).toBe(false);
    expect(list.style.top).toBe('');
  });

  it('Agent field: dropdown has no data-position / fixed-position styling, carries .dropdown-list-inline', async () => {
    const { getByText, container } = render(ScheduledPanel, {});
    await waitFor(() => expect(listScheduledJobsMock).toHaveBeenCalled());
    await fireEvent.click(getByText('New job'));

    const modal = container.querySelector('[data-testid="new-schedule-modal"]') as HTMLElement;
    const searchInput = within(modal).getByTestId('agent-search') as HTMLInputElement;
    await fireEvent.focus(searchInput);

    const list = modal.querySelector('.dropdown-list') as HTMLElement;
    expect(list).toBeTruthy();
    expect(list.classList.contains('dropdown-list-inline')).toBe(true);
    expect(list.hasAttribute('data-position')).toBe(false);
    expect(list.style.top).toBe('');
  });
});
