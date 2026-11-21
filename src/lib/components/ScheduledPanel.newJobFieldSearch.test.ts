/**
 * ScheduledPanel.newJobFieldSearch.test.ts — #3171 PR 2 (C51/C57).
 *
 * The New job dialog's Project and Agent fields were a search `<input>`
 * floating above a bare native `<select>` (#3166 C57) — the exact
 * anti-pattern #3168 introduced elsewhere. Matt's ruling: "it has to be
 * the same component the model pickers use. One fix for all three."
 * Both fields now render the generic SearchableSelect component (the
 * same one backing SearchableModelSelect for the AI-tab model pickers
 * and the image-model picker) instead of a native `<select>`.
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
    { id: 'proj-valencia', name: 'Valencia', parent_id: null },
    { id: 'proj-zephyr', name: 'Zephyr', parent_id: null },
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
  getAgentsMock.mockReset().mockResolvedValue([
    { id: 'agent-brit', name: 'Brit Freed' },
    { id: 'agent-astro', name: 'Astro' },
    { id: 'agent-nova', name: 'Nova' },
  ]);
});

function projectSearchIn(modal: HTMLElement) {
  return within(modal).getByTestId('project-search') as HTMLInputElement;
}

function agentSearchIn(modal: HTMLElement) {
  return within(modal).getByTestId('agent-search') as HTMLInputElement;
}

function dropdownItemLabels(container: HTMLElement) {
  return Array.from(container.querySelectorAll('.dropdown-item')).map((el) => el.textContent?.trim());
}

describe('#3171 PR 2 (C51/C57) — New job dialog Project/Agent fields are searchable via the shared SearchableSelect', () => {
  it('typing in the Project field narrows its dropdown to matching projects, "Home" always pinned', async () => {
    const { getByText, container } = render(ScheduledPanel, {});
    await waitFor(() => expect(listScheduledJobsMock).toHaveBeenCalled());
    await fireEvent.click(getByText('New job'));

    const modal = container.querySelector('[data-testid="new-schedule-modal"]') as HTMLElement;
    const searchInput = projectSearchIn(modal);
    await fireEvent.focus(searchInput);

    expect(dropdownItemLabels(modal)).toEqual(['Home', 'Aurum', 'Valencia', 'Zephyr']);

    await fireEvent.input(searchInput, { target: { value: 'aur' } });

    // #3178 (C57, third attempt): "Home" stays PINNED (never filtered
    // out — formProjectFilterItem's unconditional pass-through,
    // unchanged) but no longer jumps ahead of an actual typed match.
    // "aur" matches "Aurum", not "Home" — Aurum ranks first.
    await waitFor(() => {
      expect(dropdownItemLabels(modal)).toEqual(['Aurum', 'Home']);
    });
  });

  it('a Project search matching nothing still pins "Home", never shows a dangling empty dropdown', async () => {
    const { getByText, container } = render(ScheduledPanel, {});
    await waitFor(() => expect(listScheduledJobsMock).toHaveBeenCalled());
    await fireEvent.click(getByText('New job'));

    const modal = container.querySelector('[data-testid="new-schedule-modal"]') as HTMLElement;
    const searchInput = projectSearchIn(modal);
    await fireEvent.focus(searchInput);
    await fireEvent.input(searchInput, { target: { value: 'zzzznope' } });

    await waitFor(() => {
      expect(dropdownItemLabels(modal)).toEqual(['Home']);
    });
  });

  it('typing in the Agent field narrows its dropdown to matching agents only', async () => {
    const { getByText, container } = render(ScheduledPanel, {});
    await waitFor(() => expect(listScheduledJobsMock).toHaveBeenCalled());
    await fireEvent.click(getByText('New job'));
    await waitFor(() => expect(getAgentsMock).toHaveBeenCalled());

    const modal = container.querySelector('[data-testid="new-schedule-modal"]') as HTMLElement;
    const searchInput = agentSearchIn(modal);
    await fireEvent.focus(searchInput);

    await waitFor(() => {
      expect(dropdownItemLabels(modal)).toEqual(['Brit Freed', 'Astro', 'Nova']);
    });

    await fireEvent.input(searchInput, { target: { value: 'astro' } });

    await waitFor(() => {
      expect(dropdownItemLabels(modal)).toEqual(['Astro']);
    });
  });

  /// Mutation guard, named explicitly: drop the `filterItem` wiring on
  /// SearchableSelect's `formProjectFilterItem` call site (pass a
  /// predicate that always returns true) — reddens the narrowing
  /// assertions above (the list stays full length regardless of input).
  it('clearing the Project search restores the full project list', async () => {
    const { getByText, container } = render(ScheduledPanel, {});
    await waitFor(() => expect(listScheduledJobsMock).toHaveBeenCalled());
    await fireEvent.click(getByText('New job'));

    const modal = container.querySelector('[data-testid="new-schedule-modal"]') as HTMLElement;
    const searchInput = projectSearchIn(modal);
    await fireEvent.focus(searchInput);

    await fireEvent.input(searchInput, { target: { value: 'val' } });
    await waitFor(() => expect(dropdownItemLabels(modal).length).toBe(2));

    await fireEvent.input(searchInput, { target: { value: '' } });
    await waitFor(() => {
      expect(dropdownItemLabels(modal)).toEqual(['Home', 'Aurum', 'Valencia', 'Zephyr']);
    });
  });

  it('selecting a project from the dropdown sets formProjectId and closes the dropdown', async () => {
    const { getByText, container } = render(ScheduledPanel, {});
    await waitFor(() => expect(listScheduledJobsMock).toHaveBeenCalled());
    await fireEvent.click(getByText('New job'));

    const modal = container.querySelector('[data-testid="new-schedule-modal"]') as HTMLElement;
    const searchInput = projectSearchIn(modal);
    await fireEvent.focus(searchInput);

    const aurumBtn = Array.from(modal.querySelectorAll('.dropdown-item')).find(
      (el) => el.textContent?.trim() === 'Aurum'
    ) as HTMLButtonElement;
    await fireEvent.mouseDown(aurumBtn);

    await waitFor(() => {
      expect((searchInput as HTMLInputElement).value).toBe('Aurum');
    });
    expect(modal.querySelector('.dropdown-list')).toBeNull();
  });
});
