/**
 * ChatPanel.jobBriefSynthetic.test.ts — C32 (#3128).
 *
 * `insert_job_brief_message` (scheduler.rs) writes the job/wake dispatch
 * brief with role='user', synthetic=1, job_id set. ChatPanel's per-message
 * render chain had exactly 3 branches (continuation divider, schedule
 * chip, normal bubble) — none aware of `synthetic`, so this row fell
 * through to the normal bubble branch, which reads only `message.role`
 * (hardcoded 'user'), rendering a Pane-authored brief pixel-identical to
 * something the user typed. This pins the new 4th branch.
 */

import { describe, it, expect, vi, beforeAll, beforeEach } from 'vitest';
import { render, waitFor } from '@testing-library/svelte/svelte5';
import ChatPanel from './ChatPanel.svelte';

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

vi.mock('@tauri-apps/plugin-shell', () => ({
  open: vi.fn(),
}));

vi.mock('$lib/utils/markdown', () => ({
  renderMarkdown: (html: string) => html,
}));

vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn().mockResolvedValue(() => {}),
}));

vi.mock('$lib/api', () => ({
  getMainAgent: vi.fn().mockResolvedValue({ name: 'Pane' }),
  listSessions: vi.fn().mockResolvedValue([]),
  createSession: vi.fn().mockResolvedValue({ id: 'sess-1', title: '' }),
  switchSession: vi.fn().mockResolvedValue(undefined),
  hideSession: vi.fn().mockResolvedValue(undefined),
  unhideSession: vi.fn().mockResolvedValue(undefined),
  renameSession: vi.fn().mockResolvedValue(undefined),
  getActiveSession: vi.fn().mockResolvedValue(null),
  getAgents: vi.fn().mockResolvedValue([]),
  listRuntimeConnections: vi.fn().mockResolvedValue([
    { runtime_type: 'openclaw', connected: true, masked_credential: null, endpoint: null, error: null, agents: [] },
    { runtime_type: 'openrouter', connected: true, masked_credential: null, endpoint: null, error: null, agents: [] },
    { runtime_type: 'anthropic', connected: true, masked_credential: null, endpoint: null, error: null, agents: [] },
    { runtime_type: 'ollama', connected: true, masked_credential: null, endpoint: null, error: null, agents: [] },
  ]),
  getJobStatus: vi.fn().mockResolvedValue({ status: 'done', started_at: null, finished_at: null, last_error: null }),
}));

vi.mock('$lib/stores/sessionStore', async (importOriginal) => {
  const original = await importOriginal() as any;
  const { writable } = await import('svelte/store');
  const store = writable({ sessions: [], activeSession: null, loading: false, error: null });
  return {
    ...original,
    sessionStore: {
      subscribe: store.subscribe,
      loadSessions: vi.fn().mockResolvedValue(undefined),
      createSession: vi.fn().mockResolvedValue({ id: 'new', title: '' }),
      switchSession: vi.fn().mockResolvedValue(undefined),
      hideSession: vi.fn().mockResolvedValue(undefined),
      unhideSession: vi.fn().mockResolvedValue(undefined),
      renameSession: vi.fn().mockResolvedValue(undefined),
      reset: vi.fn(),
      __set: store.set,
    },
    sortSessions: original.sortSessions,
    shouldShowFilter: (count: number) => count >= 8,
    getSessionDisplayTitle: (s: any) => s.title || 'Session',
    EMPTY_STATE_SINGLE: { headline: 'Just this one so far', body: '' },
    filterSessions: (sessions: any[], q: string) => ({
      visible: sessions.filter((s: any) => s.hidden_at === null),
      hidden: sessions.filter((s: any) => s.hidden_at !== null),
    }),
  };
});

function baseProps(overrides: Partial<any> = {}) {
  return {
    messages: [] as any[],
    onSendMessage: vi.fn(),
    showHeader: false,
    projectId: 'project-test',
    activeThreadId: 'thread-open-1',
    ...overrides,
  };
}

beforeEach(() => {
  vi.clearAllMocks();
});

describe('ChatPanel — synthetic job-brief render branch (C32, #3128)', () => {
  /// Mutation 1 (actually run — see report): delete the new
  /// `{:else if message.job_id && message.synthetic && message.role ===
  /// 'user'}` branch entirely — reddens: the row falls through to
  /// `{:else}`, picks up `class="message user"`, and `.job-brief` is
  /// never rendered.
  it('a job_id+synthetic=1+role=user row does NOT render as class="message user"', async () => {
    const { container } = render(ChatPanel, baseProps({
      messages: [{
        id: 'msg-brief-1',
        thread_id: 'thread-open-1',
        role: 'user',
        content: 'Review open tasks and act on anything that needs doing.',
        created_at: new Date().toISOString(),
        job_id: 'job-brief-1',
        synthetic: 1,
      }],
    }));

    await waitFor(() => {
      const brief = container.querySelector('[data-message-id="msg-brief-1"]');
      expect(brief).toBeTruthy();
      expect(brief?.className).toContain('job-brief');
      expect(brief?.textContent).toContain('Review open tasks and act on anything that needs doing.');
    });

    // The normal bubble's own selector must NOT match this row.
    const bubble = container.querySelector('.message[data-message-id="msg-brief-1"]');
    expect(bubble).toBeNull();
  });

  /// Mutation 2 (actually run — see report): widen the new branch's
  /// condition to match any `role === 'user'` (drop the `synthetic`/
  /// `job_id` checks) — reddens: a REAL user message (synthetic=0, no
  /// job_id) would then also render as `.job-brief` instead of the normal
  /// bubble, losing its `class="message user"` chat styling. This is the
  /// companion regression guard proving the new branch isn't over-wide.
  it('a real user message (no job_id, synthetic=0) still gets the normal bubble with class="message user"', async () => {
    const { container } = render(ChatPanel, baseProps({
      messages: [{
        id: 'msg-real-user-1',
        thread_id: 'thread-open-1',
        role: 'user',
        content: 'Hey, what time is it?',
        created_at: new Date().toISOString(),
        job_id: null,
        synthetic: 0,
      }],
    }));

    await waitFor(() => {
      const bubble = container.querySelector('.message[data-message-id="msg-real-user-1"]');
      expect(bubble).toBeTruthy();
      expect(bubble?.className).toContain('user');
    });

    expect(container.querySelector('.job-brief')).toBeNull();
  });
});
