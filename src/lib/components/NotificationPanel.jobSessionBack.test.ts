/**
 * NotificationPanel.jobSessionBack.test.ts — #3151 PR1 Commit F
 *
 * The bell is the SECOND job-session entry point (the Jobs card's own
 * `openScheduledJobSession` is the first — covered by
 * `scheduledPanel.test.ts`). Proves `handleRowClick`'s `target_kind ===
 * 'session'` branch records the one-deep back target with the session
 * that was active BEFORE the click, not the one just switched into.
 */
import { describe, it, expect, vi, beforeAll, beforeEach } from 'vitest';
import { render } from '@testing-library/svelte/svelte5';
import { get } from 'svelte/store';
import NotificationPanel from './NotificationPanel.svelte';
import { activeChatThreadId, jobSessionBackTarget } from '$lib/stores/app';
import { sessionStore } from '$lib/stores/sessionStore';

beforeAll(() => {
  globalThis.ResizeObserver = class ResizeObserver {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
});

const invokeMock = vi.fn();
vi.mock('@tauri-apps/api/core', () => ({
  invoke: (...args: any[]) => invokeMock(...args),
}));
vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn().mockResolvedValue(() => {}),
}));
vi.mock('$lib/services/navigation', () => ({
  navigateTo: vi.fn().mockResolvedValue(undefined),
}));

beforeEach(() => {
  invokeMock.mockReset();
  invokeMock.mockImplementation(async (cmd: string) => {
    if (cmd === 'get_notifications') return [];
    if (cmd === 'switch_session') return { id: 'sess-job-target', project_id: 'proj-x', title: 'Morning brief', origin: 'schedule' };
    return null;
  });
  activeChatThreadId.set(null);
  jobSessionBackTarget.set(null);
  sessionStore.reset();
});

describe("#3151 PR1 Commit F — the bell's session click records the back target", () => {
  it('records whatever session was active BEFORE the click, not the job session just entered', async () => {
    activeChatThreadId.set('sess-was-here');

    const { component } = render(NotificationPanel, { panelOpen: true, onRefreshBadge: vi.fn() });

    await (component as any).handleRowClick({
      id: 'notif-1',
      type: 'agent_message',
      title: 'Message from Nova',
      body: 'done',
      source: 'system',
      read: 0,
      created_at: '2026-11-02 12:00:00',
      target_kind: 'session',
      target_id: 'sess-job-target',
    });

    expect(get(jobSessionBackTarget)).toBe('sess-was-here');
  });

  // #3166 C58: the bell is a job-session entry point that bypasses
  // SessionDropdownHeader's own switchSession/loadSessions path —
  // without this call, sessionStore.activeSession would stay pinned on
  // whatever the project's ordinary interactive session was (Matt's
  // stale "Aurum · New One" repro).
  it('sets sessionStore.activeSession to the job session switch_session resolved', async () => {
    activeChatThreadId.set('sess-was-here');

    const { component } = render(NotificationPanel, { panelOpen: true, onRefreshBadge: vi.fn() });

    await (component as any).handleRowClick({
      id: 'notif-3',
      type: 'agent_message',
      title: 'Message from Nova',
      body: 'done',
      source: 'system',
      read: 0,
      created_at: '2026-11-02 12:00:00',
      target_kind: 'session',
      target_id: 'sess-job-target',
    });

    expect(get(sessionStore).activeSession?.id).toBe('sess-job-target');
    expect(get(sessionStore).activeSession?.title).toBe('Morning brief');
  });

  it('records nothing when there was no previous session open (nothing to go back to)', async () => {
    activeChatThreadId.set(null);

    const { component } = render(NotificationPanel, { panelOpen: true, onRefreshBadge: vi.fn() });

    await (component as any).handleRowClick({
      id: 'notif-2',
      type: 'agent_message',
      title: 'Message from Nova',
      body: 'done',
      source: 'system',
      read: 0,
      created_at: '2026-11-02 12:00:00',
      target_kind: 'session',
      target_id: 'sess-job-target',
    });

    expect(get(jobSessionBackTarget)).toBeNull();
  });
});
