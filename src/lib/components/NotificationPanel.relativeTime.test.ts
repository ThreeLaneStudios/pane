/**
 * NotificationPanel.relativeTime.test.ts — #3151 PR3/C44
 *
 * `formatRelativeTime` (this component's own inline copy) parsed
 * SQLite's naive-UTC `created_at` string ("YYYY-MM-DD HH:MM:SS", no
 * `T`/`Z`/offset) straight through `new Date()`, which treats it as
 * LOCAL wall-clock time. In any timezone behind UTC that makes the diff
 * negative, satisfying `mins < 2` unconditionally — every notification
 * showed "now" forever. Built from the REAL `Date.now()` at test time
 * (no mocked clock), same approach as `utils/time.test.ts`'s own C44
 * suite and for the same reason (Vitest's jsdom worker pool does not
 * reliably pick up a mid-test `process.env.TZ` reassignment once
 * `Date`/`Intl` has already been touched once in that worker).
 */
import { describe, it, expect, vi, beforeAll, beforeEach } from 'vitest';
import { render, waitFor } from '@testing-library/svelte/svelte5';
import NotificationPanel from './NotificationPanel.svelte';

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

/** Builds SQLite's own naive-UTC `datetime('now')` shape, `secondsAgo`
 *  seconds before the REAL current instant. */
function naiveSqliteSecondsAgo(secondsAgo: number): string {
  return new Date(Date.now() - secondsAgo * 1000)
    .toISOString()
    .slice(0, 19)
    .replace('T', ' ');
}

beforeEach(() => {
  invokeMock.mockReset();
});

describe('NotificationPanel — a notification a few minutes old is never stuck at "now" (C44)', () => {
  it('a notification 150 seconds old shows "2m", not "now"', async () => {
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === 'get_notifications') {
        return [
          {
            id: 'notif-c44',
            type: 'agent_message',
            title: 'Message from Nova',
            body: 'done',
            source: 'system',
            read: 0,
            created_at: naiveSqliteSecondsAgo(150),
            target_kind: 'session',
            target_id: 'sess-x',
          },
        ];
      }
      return null;
    });

    const { container, findByText, queryByText } = render(NotificationPanel, { panelOpen: true, onRefreshBadge: vi.fn() });

    await waitFor(() => expect(invokeMock).toHaveBeenCalled(), { container });
    await findByText('2m');
    expect(queryByText('now')).toBeNull();
  });
});
