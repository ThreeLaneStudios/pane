/**
 * ChatSidebar.ocMessageIngested.test.ts — §36.6 (#3050 PR 7), Reed's
 * audit (2026-10-29, PR #3115, §7): "the PR's own build brief... explicitly
 * called for a frontend-visible pin... That test does not exist."
 *
 * `1c3402566` added the `pane-oc-message-ingested` event AND the frontend
 * listener (`ChatSidebar.svelte`, not `ChatPanel.svelte` — that file's own
 * nearby comment is stale, see the nit in Reed's audit §7) — but shipped
 * with no Vitest pin. This is the mirror image of PR 6's exact miss (an
 * emit with no listener): a listener with no test, which works today and
 * silently dies the next time someone refactors `ChatSidebar`'s onMount.
 *
 * Mutation (actually run — see report): removed the
 * `listen('pane-oc-message-ingested', ...)` registration block from
 * `ChatSidebar.svelte`'s `onMount`. Failure: the "registers a listener"
 * assertion reddens immediately (`listenMock` never called with this event
 * name) — restored immediately after, confirmed green again.
 *
 * Uses the same `listenersByEvent` capture-and-fire pattern
 * `ChatPanel.chipListener.test.ts`/`UpdatingScreen.test.ts` already
 * established for this exact `@tauri-apps/api/event` mocking shape, and
 * the same component-mount scaffolding `ChatSidebar.viewingSession.test.ts`
 * already uses for mounting the real `ChatSidebar`.
 */

import { describe, it, expect, vi, beforeAll, beforeEach, afterEach } from 'vitest';
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

vi.mock('@tauri-apps/plugin-shell', () => ({
  open: vi.fn(),
}));

vi.mock('$lib/utils/markdown', () => ({
  renderMarkdown: (html: string) => html,
}));

// Same `listenersByEvent` capture-and-fire shape `ChatPanel.chipListener
// .test.ts` already established — captures the real handler ChatSidebar's
// onMount registers for each event name, so it can be invoked directly to
// simulate a real backend emit landing.
const { listenMock, listenersByEvent } = vi.hoisted(() => ({
  listenMock: vi.fn(),
  listenersByEvent: new Map<string, (event: { payload: unknown }) => void>(),
}));
vi.mock('@tauri-apps/api/event', () => ({
  listen: listenMock,
}));

function fireOcMessageIngested(payload: { session_id: string }) {
  const handler = listenersByEvent.get('pane-oc-message-ingested');
  if (!handler) throw new Error('no listener registered for pane-oc-message-ingested');
  handler({ payload });
}

function activeAgent(overrides: Partial<any> = {}) {
  return {
    id: 'agent-active-1',
    name: 'Nova',
    slug: 'nova',
    role: 'main',
    description: null,
    model: 'claude-sonnet-4-5',
    is_default: false,
    is_active: true,
    workspace_path: 'WPagent-active-1',
    soul_hash: null,
    memory_hash: null,
    config_json: '{}',
    owner_id: null,
    runtime_ref: 'oc-main',
    runtime_type: 'openclaw',
    source: 'openclaw',
    created_at: '2026-01-01T00:00:00Z',
    updated_at: '2026-01-01T00:00:00Z',
    ...overrides,
  };
}

function mkSession(overrides: Partial<any> = {}) {
  return {
    id: 'sess-oc-ingest-open',
    project_id: 'workspace',
    title: 'Open session',
    origin: 'user',
    created_at: '2026-01-01T10:00:00Z',
    last_active_at: '2026-10-29T08:00:00Z',
    hidden_at: null,
    is_active: true,
    agent_id: 'agent-active-1',
    ...overrides,
  };
}

vi.mock('$lib/api', async (importOriginal) => {
  const original = await importOriginal() as any;
  return {
    ...original,
    getAgents: vi.fn().mockResolvedValue([activeAgent()]),
    getChatMessages: vi.fn().mockResolvedValue([]),
    listRuntimeConnections: vi.fn().mockResolvedValue([
      { runtime_type: 'openclaw', connected: true, masked_credential: null, endpoint: null, error: null, agents: [] },
      { runtime_type: 'openrouter', connected: true, masked_credential: null, endpoint: null, error: null, agents: [] },
      { runtime_type: 'anthropic', connected: true, masked_credential: null, endpoint: null, error: null, agents: [] },
      { runtime_type: 'ollama', connected: true, masked_credential: null, endpoint: null, error: null, agents: [] },
    ]),
    aiLoadConfig: vi.fn().mockResolvedValue({ provider: 'openclaw' }),
    listProjects: vi.fn().mockResolvedValue([]),
    setViewingSession: vi.fn().mockResolvedValue(undefined),
  };
});

vi.mock('$lib/stores/sessionStore', async (importOriginal) => {
  const original = await importOriginal() as any;
  const { writable } = await import('svelte/store');

  const store = writable({
    sessions: [],
    activeSession: null,
    loading: false,
    error: null,
  });

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

beforeEach(() => {
  listenMock.mockReset();
  listenersByEvent.clear();
  listenMock.mockImplementation(async (eventName: string, handler: (event: { payload: unknown }) => void) => {
    listenersByEvent.set(eventName, handler);
    return () => listenersByEvent.delete(eventName);
  });
  (apiMod.getChatMessages as any).mockReset().mockResolvedValue([]);
});

afterEach(() => {
  (sessionStoreMod.sessionStore as any).__set({ sessions: [], activeSession: null, loading: false, error: null });
  (sessionStoreMod.sessionStore.getActiveSession as any).mockResolvedValue(mkSession());
  appStoreMod.activeProject.set(null);
  appStoreMod.activeChatThreadId.set(null);
  (apiMod.getAgents as any).mockResolvedValue([activeAgent()]);
  (apiMod.getChatMessages as any).mockResolvedValue([]);
});

describe('ChatSidebar — pane-oc-message-ingested listener (§36.6, #3050 PR 7 / PR #3115 audit §7)', () => {
  /// This is the core pin. Mutation: delete the
  /// `listen('pane-oc-message-ingested', ...)` block from ChatSidebar's
  /// onMount. Failure: this assertion reddens — `listenMock` is never
  /// called with this event name because the registration no longer runs.
  it('registers a pane-oc-message-ingested listener on mount', async () => {
    appStoreMod.activeChatThreadId.set('sess-oc-ingest-open');
    render(ChatSidebar, { layoutMode: 'centered', alwaysOpen: true });

    await waitFor(() => {
      expect(listenMock).toHaveBeenCalledWith('pane-oc-message-ingested', expect.any(Function));
    });
  });

  /// Acceptance proof: an event for the OPEN session reloads its
  /// transcript — `loadMessages` is not itself exported, so its one
  /// observable side effect (`api.getChatMessages(threadId, 50)`) is the
  /// proxy. Same mutation as above reddens this test too (the handler is
  /// never registered, so `fireOcMessageIngested` throws before the
  /// assertion is even reached).
  it('reloads the open session transcript when the event session matches the open thread', async () => {
    appStoreMod.activeChatThreadId.set('sess-oc-ingest-open');
    render(ChatSidebar, { layoutMode: 'centered', alwaysOpen: true });

    await waitFor(() => {
      expect(listenersByEvent.has('pane-oc-message-ingested')).toBe(true);
    });
    (apiMod.getChatMessages as any).mockClear();

    fireOcMessageIngested({ session_id: 'sess-oc-ingest-open' });

    await waitFor(() => {
      expect(apiMod.getChatMessages).toHaveBeenCalledWith('sess-oc-ingest-open', 50);
    });
  });

  /// The filter: an event for a session the user is NOT currently
  /// looking at must not trigger a reload of the open thread's own
  /// transcript — mirrors `ChatPanel.chipListener.test.ts`'s own
  /// different-session filter proof for `pane-chip-changed`.
  it('does NOT reload when the event session does not match the open thread', async () => {
    appStoreMod.activeChatThreadId.set('sess-oc-ingest-open');
    render(ChatSidebar, { layoutMode: 'centered', alwaysOpen: true });

    await waitFor(() => {
      expect(listenersByEvent.has('pane-oc-message-ingested')).toBe(true);
    });
    (apiMod.getChatMessages as any).mockClear();

    fireOcMessageIngested({ session_id: 'sess-oc-OTHER-closed' });

    // No await-able positive signal for "nothing happened" — flush a tick
    // and assert the reload never fired.
    await new Promise(r => setTimeout(r, 0));
    expect(apiMod.getChatMessages).not.toHaveBeenCalled();
  });
});
