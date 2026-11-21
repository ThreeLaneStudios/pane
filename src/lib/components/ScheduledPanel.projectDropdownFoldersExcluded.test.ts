/**
 * ScheduledPanel.projectDropdownFoldersExcluded.test.ts — #3166 C56.
 *
 * `$projects` (`src/lib/stores/app.ts`) is a flat dump of the `projects`
 * table — root projects (`parent_id IS NULL`) and folders (`parent_id`
 * set to the owning project) together, by design. The New job dialog's
 * Project `<select>` iterated it with no `!p.parent_id` filter at all,
 * so every folder across every project ("General" × however many
 * projects exist, plus every named folder) showed up as a fake
 * "project."
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
  PROJECTS: [
    { id: 'proj-aurum', name: 'Aurum', parent_id: null },
    { id: 'proj-aurum-general', name: 'General', parent_id: 'proj-aurum' },
    { id: 'proj-aurum-archive', name: 'Archive', parent_id: 'proj-aurum' },
    { id: 'proj-valencia', name: 'Valencia', parent_id: null },
    { id: 'proj-valencia-general', name: 'General', parent_id: 'proj-valencia' },
    { id: 'proj-valencia-articles', name: 'Articles', parent_id: 'proj-valencia' },
    { id: 'proj-valencia-drafts', name: 'Drafts', parent_id: 'proj-valencia' },
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
  getAgentsMock.mockReset().mockResolvedValue([{ id: 'agent-brit', name: 'Brit Freed' }]);
});

describe('#3166 C56 / #3171 PR 2 — New job dialog Project field lists projects only', () => {
  it('shows exactly the 2 root projects — Home plus Aurum and Valencia, never "General" x2/Archive/Articles/Drafts', async () => {
    const { getByText, container } = render(ScheduledPanel, {});
    await waitFor(() => expect(listScheduledJobsMock).toHaveBeenCalled());

    await fireEvent.click(getByText('New job'));

    const modal = container.querySelector('[data-testid="new-schedule-modal"]') as HTMLElement;
    expect(modal).toBeTruthy();
    // #3171 PR 2 (C51/C57): the Project field is now the shared
    // SearchableSelect component, not a native `<select>` — its options
    // render as `.dropdown-item` buttons once the field is focused/open.
    const searchInput = within(modal).getByTestId('project-search') as HTMLInputElement;
    await fireEvent.focus(searchInput);

    const items = Array.from(modal.querySelectorAll('.dropdown-item'));
    const itemTexts = items.map((el) => el.textContent?.trim());

    // "Home" (the hardcoded workspace sentinel) + the 2 real root projects.
    expect(itemTexts).toEqual(['Home', 'Aurum', 'Valencia']);

    // None of the 5 folder rows (2 x "General", Archive, Articles, Drafts)
    // may appear as a fake project.
    expect(itemTexts.filter(t => t === 'General')).toHaveLength(0);
    expect(itemTexts).not.toContain('Archive');
    expect(itemTexts).not.toContain('Articles');
    expect(itemTexts).not.toContain('Drafts');
  });

  /// Mutation guard, named explicitly: dropping the `!p.parent_id`
  /// filter inside `formProjectItems` (iterating `$projects` raw again)
  /// reddens the exact assertion above — item count goes from 3 to 8
  /// (Home + 2 roots + 5 folders) and "General" appears twice.
  it('item count is exactly 3 (Home + 2 root projects), not 8 (Home + all 7 rows)', async () => {
    const { getByText, container } = render(ScheduledPanel, {});
    await waitFor(() => expect(listScheduledJobsMock).toHaveBeenCalled());

    await fireEvent.click(getByText('New job'));

    const modal = container.querySelector('[data-testid="new-schedule-modal"]') as HTMLElement;
    const searchInput = within(modal).getByTestId('project-search') as HTMLInputElement;
    await fireEvent.focus(searchInput);

    expect(modal.querySelectorAll('.dropdown-item').length).toBe(3);
  });
});
