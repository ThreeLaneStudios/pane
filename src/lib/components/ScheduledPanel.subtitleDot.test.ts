/**
 * ScheduledPanel.subtitleDot.test.ts — C2 (#3120 PR 3, Reed's PR 3 brief,
 * "literal `·` in panel card subtitles").
 *
 * The row subtitle's separator between project and agent name
 * (`.sc-m .dt`) was written in the Svelte template as the SIX-character
 * escape-sequence text `\u00b7` — valid syntax inside a JS *string*
 * literal, but meaningless inside plain HTML/Svelte markup text, which
 * has no unicode-escape interpretation of its own. The browser rendered
 * those six literal characters verbatim instead of the intended U+00B7
 * MIDDLE DOT character, so a row subtitle read "Home\u00b7Brit Freed"
 * rather than "Home · Brit Freed". Fixed by typing the real character
 * directly into the template (`·`), matching every other separator dot
 * elsewhere in the app that already does this correctly.
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

describe('ScheduledPanel — subtitle separator is a real middle dot, not literal escape text (C2)', () => {
  /// Mutation: revert `.dt`'s template text from the real `·` character
  /// back to the six-character string `\u00b7` — reddens (the row's own
  /// text content would contain the literal backslash-u-zero-zero-b-seven
  /// substring instead of the single real dot character, and the
  /// charCodeAt assertion below would fail since there'd be no U+00B7 in
  /// the node at all).
  it('an Active row\u2019s project · agent separator is the real U+00B7 character', async () => {
    const { container } = render(ScheduledPanel, {});
    await waitFor(() => expect(listScheduledJobsMock).toHaveBeenCalled());
    await waitFor(() => expect(container.querySelector('.sc-m .dt')).toBeTruthy());

    const dot = container.querySelector('.sc-m .dt')!;
    expect(dot.textContent).toBe('\u00b7');
    expect(dot.textContent).not.toContain('\\u00b7');
    expect(dot.textContent!.charCodeAt(0)).toBe(0xb7);

    const subtitle = container.querySelector('.sc-m')!;
    expect(subtitle.textContent).not.toContain('\\u00b7');
    expect(subtitle.textContent).toContain('Home');
    expect(subtitle.textContent).toContain('Brit Freed');
  });
});
