/**
 * ChatSidebar.jobAwareEmptyState.test.ts — C19 (#3120 PR 3).
 *
 * "A job session that hasn't fired shows the generic 'Fresh session — a
 * clean thread…' copy. Replace with job-aware copy." `ChatSidebar.svelte`
 * resolves whether the zero-message session on screen IS a job's own
 * session (via `api.listScheduledJobs()`, matched on `session_id`) and, if
 * so, passes `jobAwareEmptyStateHint`'s computed string down to
 * `ChatPanel` as `jobAwareHint` — covered at the unit level by
 * `ChatPanel.test.ts` (does the right thing with the prop) and
 * `scheduledPanel.test.ts` (computes the right string); this is the one
 * seam only a real `ChatSidebar` mount can exercise — the wiring itself.
 *
 * Same mount/mock scaffold as `ChatSidebar.composerPlaceholder.test.ts`.
 */

import { describe, it, expect, vi, beforeAll, afterEach } from 'vitest';
import { render, waitFor } from '@testing-library/svelte/svelte5';
import ChatSidebar from './ChatSidebar.svelte';
import * as sessionStoreMod from '$lib/stores/sessionStore';
import * as appStoreMod from '$lib/stores/app';
import * as apiMod from '$lib/api';

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

vi.mock('@tauri-apps/plugin-shell', () => ({ open: vi.fn() }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn().mockResolvedValue(() => {}) }));
vi.mock('$lib/utils/markdown', () => ({ renderMarkdown: (html: string) => html }));

function mkSession(overrides: Partial<any> = {}) {
  return {
    id: 'sess-job-1',
    project_id: 'workspace',
    title: 'Morning brief',
    origin: 'schedule',
    created_at: '2026-08-01T10:00:00Z',
    last_active_at: '2026-08-22T08:00:00Z',
    hidden_at: null,
    is_active: false,
    agent_id: 'agent-brit',
    ...overrides,
  };
}

function jobRow(overrides: Partial<any> = {}) {
  return {
    id: 'job-1',
    title: 'Morning brief',
    kind: 'recurring',
    agent: 'Brit',
    project: 'Home',
    project_id: 'workspace',
    next_run_at: new Date(Date.now() + 3600_000).toISOString(),
    status: 'pending',
    missed_count: 0,
    started_at: null,
    finished_at: null,
    last_error: null,
    session_id: 'sess-job-1',
    ...overrides,
  };
}

const { listScheduledJobsMock } = vi.hoisted(() => ({ listScheduledJobsMock: vi.fn() }));

vi.mock('$lib/api', async (importOriginal) => {
  const original = await importOriginal() as any;
  return {
    ...original,
    getAgents: vi.fn().mockResolvedValue([]),
    getChatMessages: vi.fn().mockResolvedValue([]),
    listRuntimeConnections: vi.fn().mockResolvedValue([]),
    aiLoadConfig: vi.fn().mockResolvedValue({ provider: 'openclaw' }),
    listProjects: vi.fn().mockResolvedValue([]),
    listScheduledJobs: listScheduledJobsMock,
  };
});

vi.mock('$lib/stores/sessionStore', async (importOriginal) => {
  const original = await importOriginal() as any;
  const { writable } = await import('svelte/store');
  const store = writable({ sessions: [], activeSession: mkSession(), loading: false, error: null });
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
      getActiveSession: vi.fn().mockResolvedValue(mkSession()),
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

vi.mock('$lib/services/streamManager', () => ({
  streamManager: {
    init: vi.fn().mockResolvedValue(undefined),
    consumePendingResult: vi.fn().mockReturnValue(false),
    getBuffer: vi.fn().mockReturnValue(''),
    isStreamingThread: vi.fn().mockReturnValue(false),
    reattachCallbacks: vi.fn(),
    getCapturedContext: vi.fn().mockReturnValue(null),
    setPendingResult: vi.fn(),
    startStream: vi.fn(),
  },
}));

afterEach(() => {
  (sessionStoreMod.sessionStore as any).__set({ sessions: [], activeSession: mkSession(), loading: false, error: null });
  appStoreMod.activeProject.set(null);
  appStoreMod.activeChatThreadId.set(null);
  listScheduledJobsMock.mockReset();
});

describe('ChatSidebar — job-aware empty state (C19)', () => {
  /// Mutation: remove the `$effect` that calls `resolveJobAwareHint` (or
  /// revert `ChatPanel`'s `jobAwareHint ??` back to the unconditional
  /// generic hint) — reddens (the generic "Fresh session" text renders
  /// instead of the job-aware copy).
  it('a zero-message job session shows job-aware copy, not the generic "Fresh session" hint', async () => {
    listScheduledJobsMock.mockResolvedValue([jobRow()]);
    (sessionStoreMod.sessionStore as any).__set({ sessions: [], activeSession: mkSession(), loading: false, error: null });

    const { container } = render(ChatSidebar, { layoutMode: 'sidebar', alwaysOpen: true });

    await waitFor(() => expect(listScheduledJobsMock).toHaveBeenCalled());
    await waitFor(() => {
      expect(container.textContent).toContain('Morning brief · Brit · next run');
      expect(container.textContent).toContain('Reports land here.');
    });
    expect(container.textContent).not.toContain('Fresh session');
  });

  it('an ordinary (non-job) zero-message session keeps the generic "Fresh session" hint — regression guard', async () => {
    listScheduledJobsMock.mockResolvedValue([]); // no job matches this session
    (sessionStoreMod.sessionStore as any).__set({
      sessions: [],
      activeSession: mkSession({ id: 'sess-ordinary-1', origin: 'user' }),
      loading: false,
      error: null,
    });

    const { container } = render(ChatSidebar, { layoutMode: 'sidebar', alwaysOpen: true });

    await waitFor(() => expect(listScheduledJobsMock).toHaveBeenCalled());
    await waitFor(() => {
      expect(container.textContent).toContain('Fresh session');
    });
  });
});
