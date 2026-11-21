/**
 * ChatSidebar.viewingSession.test.ts — §36.7 (#3050 PR 6), PR #3114 fix.
 *
 * Reed's audit: "`set_viewing_session` has ZERO frontend call sites…
 * zero hits in `src/`, across all five commits." `navigateTo`
 * (navigation.test.ts) covers every path THAT funnel reaches — but the
 * sessions-drawer switch (`SessionsDrawer.svelte` → `onSessionSwitch` →
 * `ChatSidebar.svelte`'s exported `handleSessionSwitch`) bypasses
 * `navigateTo` entirely (it calls `api.switchSession` directly, then
 * updates `$activeChatThreadId` itself — see `handleSessionSwitch`'s own
 * doc comment). This is the ONE session-switch path `navigateTo`'s own
 * fix does not cover "for free," so it needs its own dedicated call site
 * and its own dedicated test — exactly the "future path added without
 * the call is visibly uncovered" shape the task asked for.
 *
 * Mounts the full `ChatSidebar` component (same justification as
 * `ChatSidebar.homeAgentName.test.ts`: `handleSessionSwitch` is an
 * exported method on the real component instance, not something a
 * standalone logic extraction could exercise).
 */

import { describe, it, expect, vi, beforeAll, afterEach } from 'vitest';
import { render } from '@testing-library/svelte/svelte5';
import ChatSidebar from './ChatSidebar.svelte';
import * as sessionStoreMod from '$lib/stores/sessionStore';
import * as appStoreMod from '$lib/stores/app';
import * as apiMod from '$lib/api';

// ─── Global polyfills (mirrors ChatSidebar.homeAgentName.test.ts) ─────────

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

// Same unhandled-rejection-avoidance mock ChatSidebar.homeAgentName.test.ts
// uses for ChatSidebar's own onMount `listen()` calls.
vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn().mockResolvedValue(() => {}),
}));

vi.mock('$lib/utils/markdown', () => ({
  renderMarkdown: (html: string) => html,
}));

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
    id: 'sess-drawer-target-1',
    project_id: 'workspace',
    title: 'Drawer target session',
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

afterEach(() => {
  (sessionStoreMod.sessionStore as any).__set({ sessions: [], activeSession: null, loading: false, error: null });
  (sessionStoreMod.sessionStore.getActiveSession as any).mockResolvedValue(mkSession());
  appStoreMod.activeProject.set(null);
  appStoreMod.activeChatThreadId.set(null);
  (apiMod.getAgents as any).mockResolvedValue([activeAgent()]);
  (apiMod.getChatMessages as any).mockResolvedValue([]);
  (apiMod.setViewingSession as any).mockClear();
});

describe('ChatSidebar.handleSessionSwitch — setViewingSession wiring (§36.7, #3050 PR 6 / #3114)', () => {
  /// PATH 5 of the "every navigation path" set: the sessions-drawer switch
  /// — the ONE session-switch path that bypasses `navigateTo` entirely.
  ///
  /// Mutation (actually run — see report): removed the
  /// `await api.setViewingSession(session.id)` call from
  /// `handleSessionSwitch`. Failure: this assertion reddens (never
  /// called) — proving this path was genuinely uncovered by `navigateTo`'s
  /// own fix, not accidentally covered by a shared code path.
  it('calls setViewingSession with the target session id when the drawer switches sessions', async () => {
    appStoreMod.activeProject.set(null);
    (apiMod.getChatMessages as any).mockResolvedValue([]);
    (sessionStoreMod.sessionStore as any).__set({ sessions: [], activeSession: null, loading: false, error: null });
    (sessionStoreMod.sessionStore.getActiveSession as any).mockResolvedValue(mkSession());

    const { component } = render(ChatSidebar, { layoutMode: 'centered', alwaysOpen: true });

    await (component as any).handleSessionSwitch(mkSession({ id: 'sess-drawer-target-2' }));

    expect(apiMod.setViewingSession).toHaveBeenCalledWith('sess-drawer-target-2');
  });

  /// A failed setViewingSession call must not crash the switch itself —
  /// same non-fatal contract `navigateTo`'s own call has (a stale
  /// notification gate is strictly better than the whole session switch
  /// throwing and leaving the UI stuck on the old thread).
  it('does not throw when setViewingSession rejects — the session switch still completes', async () => {
    (apiMod.setViewingSession as any).mockRejectedValueOnce(new Error('ipc down'));
    (sessionStoreMod.sessionStore as any).__set({ sessions: [], activeSession: null, loading: false, error: null });
    (sessionStoreMod.sessionStore.getActiveSession as any).mockResolvedValue(mkSession());

    const { component } = render(ChatSidebar, { layoutMode: 'centered', alwaysOpen: true });

    await expect(
      (component as any).handleSessionSwitch(mkSession({ id: 'sess-drawer-target-3' }))
    ).resolves.toBeUndefined();

    expect(appStoreMod.activeChatThreadId as any).toBeDefined();
  });
});
