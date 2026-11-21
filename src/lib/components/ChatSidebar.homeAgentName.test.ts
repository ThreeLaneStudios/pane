/**
 * ChatSidebar.homeAgentName.test.ts — #2643
 *
 * Pins the fix documented in
 * docs/audits/2026-08-22-2641-2643-card-colors-home-header-scope.md §2.4:
 * on the Home surface, SessionDropdownHeader never mounts (`showHeader` is
 * gated on `$activeProject !== null`), so it never fires its
 * `onBoundAgentChange` callback and ChatPanel's speaker label fell through
 * to the hardcoded 'Pane' literal until a message's DB-sourced `agent_name`
 * landed (i.e. until `onDone`). The fix: ChatSidebar resolves the Home
 * session's bound agent name independently via `resolveBoundAgent()` (the
 * same pure helper `SessionDropdownHeader` uses) and passes it to
 * `ChatPanel` as `boundAgentNameProp`, which `ChatPanel` derives
 * `sessionBoundAgentName` from when `showHeader` is false — mirroring the
 * existing `readOnlyState`/`readOnlyStateProp` pattern exactly (D22, #2649
 * family).
 *
 * Unlike `homeReadOnlyState` (only set when the session IS locked),
 * `homeBoundAgentName` must resolve UNCONDITIONALLY — the healthy/common
 * case (ordinary streaming with an active, connected agent) is exactly what
 * this bug affected, so the test fixtures here use an ACTIVE, CONNECTED
 * agent (not the deactivated fixture from ChatSidebar.homeLockBanner.test.ts)
 * to isolate the agent-name resolution path from the read-only lock path.
 *
 * ChatSidebar.svelte is a large, API/session-integrated component. Per the
 * established pattern in this file family (ChatSidebar.homeLockBanner.test.ts,
 * ChatPanel.test.ts), this DOES mount the full component because the bug is
 * specifically about a prop crossing the ChatSidebar → ChatPanel boundary,
 * which standalone logic extraction cannot exercise.
 */

import { describe, it, expect, vi, beforeAll, afterEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/svelte/svelte5';
import ChatSidebar from './ChatSidebar.svelte';
import ChatPanel from './ChatPanel.svelte';
import * as sessionStoreMod from '$lib/stores/sessionStore';
import * as appStoreMod from '$lib/stores/app';
import * as apiMod from '$lib/api';

// ─── Global polyfills (mirrors ChatSidebar.homeLockBanner.test.ts) ─────────

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

// ChatSidebar.svelte's onMount listens for 'memory-extracted' and
// 'pane-navigate' via @tauri-apps/api/event, then unlistens them on
// teardown. Without this mock, the real listen() import reaches into
// window.__TAURI_EVENT_PLUGIN_INTERNALS__ (unset in jsdom) during unmount,
// throwing an unhandled rejection from _unlisten. Same pattern as
// Onboarding.test.ts / WorkshopReview.test.ts / AgentCreateOverlay.workshopSession.test.ts.
vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn().mockResolvedValue(() => {}),
}));

vi.mock('$lib/utils/markdown', () => ({
  renderMarkdown: (html: string) => html,
}));

// Active, fully-connected agent bound to the Home session — isolates the
// agent-name resolution path from the read-only/lock path (that's covered
// separately by ChatSidebar.homeLockBanner.test.ts).
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

function mkHomeSession(overrides: Partial<any> = {}) {
  return {
    id: 'sess-home-agent-1',
    project_id: 'workspace',
    title: 'Home session',
    origin: 'user',
    created_at: '2026-08-01T10:00:00Z',
    last_active_at: '2026-08-22T08:00:00Z',
    hidden_at: null,
    is_active: true,
    agent_id: 'agent-active-1',
    ...overrides,
  };
}

// Live-streamed-style message: no `agent_name` yet (exactly the failure
// window from the audit — before onDone/DB reload populates it).
function liveMessageNoAgentName(overrides: Partial<any> = {}) {
  return {
    id: 'msg-live-home-1',
    thread_id: 'sess-home-agent-1',
    role: 'assistant' as const,
    content: 'Here is my answer.',
    created_at: new Date().toISOString(),
    tool_calls: null,
    tool_results: null,
    // agent_name intentionally absent
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
  };
});

vi.mock('$lib/stores/sessionStore', async (importOriginal) => {
  const original = await importOriginal() as any;
  const { writable } = await import('svelte/store');

  // Real Home config: session store starts EMPTY — nothing on Home populates
  // it before NAV resolves the thread. getActiveSession('workspace') is the
  // resolution path the fixed Home effect calls, keyed off $activeChatThreadId.
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
      getActiveSession: vi.fn().mockResolvedValue(mkHomeSession()),
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
  (sessionStoreMod.sessionStore.getActiveSession as any).mockResolvedValue(mkHomeSession());
  appStoreMod.activeProject.set(null);
  appStoreMod.activeChatThreadId.set(null);
  (apiMod.getAgents as any).mockResolvedValue([activeAgent()]);
  (apiMod.getChatMessages as any).mockResolvedValue([]);
});

describe('ChatSidebar — Home bound-agent name resolution (#2643)', () => {
  it('Home session bound to an agent: live message with no agent_name yet labels the BOUND agent ("Nova"), never "Pane"', async () => {
    appStoreMod.activeProject.set(null);
    (apiMod.getAgents as any).mockResolvedValue([activeAgent()]);
    (apiMod.getChatMessages as any).mockResolvedValue([liveMessageNoAgentName()]);
    // Session store starts empty (real Home config); getActiveSession resolves
    // the session bound to "Nova" once the Home effect keys off $activeChatThreadId.
    (sessionStoreMod.sessionStore as any).__set({ sessions: [], activeSession: null, loading: false, error: null });
    (sessionStoreMod.sessionStore.getActiveSession as any).mockResolvedValue(mkHomeSession());

    const { container, component } = render(ChatSidebar, { layoutMode: 'centered', alwaysOpen: true });

    // Simulates NAV resolution (pane-navigation-complete) setting the thread id
    // AFTER mount — this is what drives the Home effect on real Home.
    appStoreMod.activeChatThreadId.set('sess-home-agent-1');

    // Drives loadMessages(), which loads the mocked live message with no
    // agent_name, exercising the exact fallback window from the audit.
    await (component as any).handleSessionSwitch(mkHomeSession());

    await waitFor(() => {
      const sender = container.querySelector('.chat-sender');
      expect(sender?.textContent).toBe('Nova');
    });
    expect(container.querySelector('.chat-sender')?.textContent).not.toBe('Pane');
  });

  it('Home session with no bound agent: fallback to "Pane" is preserved (no boundAgentNameProp regression)', async () => {
    appStoreMod.activeProject.set(null);
    (apiMod.getAgents as any).mockResolvedValue([activeAgent()]);
    (apiMod.getChatMessages as any).mockResolvedValue([
      liveMessageNoAgentName({ id: 'msg-live-unbound-1', thread_id: 'sess-home-unbound' }),
    ]);
    (sessionStoreMod.sessionStore as any).__set({ sessions: [], activeSession: null, loading: false, error: null });
    (sessionStoreMod.sessionStore.getActiveSession as any).mockResolvedValue(
      mkHomeSession({ id: 'sess-home-unbound', agent_id: null })
    );

    const { container, component } = render(ChatSidebar, { layoutMode: 'centered', alwaysOpen: true });

    // Simulates NAV resolution setting the thread id AFTER mount.
    appStoreMod.activeChatThreadId.set('sess-home-unbound');

    await (component as any).handleSessionSwitch(mkHomeSession({ id: 'sess-home-unbound', agent_id: null }));

    // resolveBoundAgent() returns null for an unbound session →
    // homeBoundAgentName stays null → ChatPanel's fallback chain reaches the
    // literal 'Pane' (message.agent_name ?? sessionBoundAgentName ?? 'Pane').
    await waitFor(() => {
      const sender = container.querySelector('.chat-sender');
      expect(sender?.textContent).toBe('Pane');
    });
  });

  it('project surface does not use Home bound agent name: header-driven name wins over boundAgentNameProp', async () => {
    // Project surface = showHeader=true (ChatSidebar's own gate: showHeader
    // && $activeProject !== null — see ChatSidebar.svelte's ChatPanel
    // invocation). Setting $activeProject here mirrors that real-world gate
    // even though ChatPanel itself only consumes the resulting `showHeader`
    // boolean, not the store directly.
    appStoreMod.activeProject.set({
      id: 'project-test',
      name: 'Test Project',
      parent_id: null,
      created_at: '2026-01-01T00:00:00Z',
      updated_at: '2026-01-01T00:00:00Z',
    } as any);
    appStoreMod.activeChatThreadId.set('sess-header-agent-1');

    // Session bound to "HeaderAgent" — SessionDropdownHeader resolves this
    // itself (activeSession.agent_id → allAgents.find, the same
    // resolveBoundAgent() helper Home uses) and surfaces it via its own
    // onBoundAgentChange callback once it mounts (showHeader=true).
    const headerAgent = activeAgent({ id: 'agent-header-1', name: 'HeaderAgent' });
    const projectSession = mkHomeSession({
      id: 'sess-header-agent-1',
      project_id: 'project-test',
      agent_id: 'agent-header-1',
    });

    (apiMod.getAgents as any).mockResolvedValue([headerAgent]);
    (sessionStoreMod.sessionStore as any).__set({
      sessions: [projectSession],
      activeSession: projectSession,
      loading: false,
      error: null,
    });

    // At least one message with agent_name: null so the fallback chain
    // (message.agent_name ?? sessionBoundAgentName ?? 'Pane') is actually
    // exercised — the previous version of this test rendered zero messages
    // and never asserted the speaker label at all, so it would have passed
    // even if boundAgentNameProp leaked onto project surfaces.
    const liveMessage = liveMessageNoAgentName({
      id: 'msg-live-project-1',
      thread_id: 'sess-header-agent-1',
    });

    // Mount ChatPanel directly (rather than ChatSidebar) so boundAgentNameProp
    // can be set explicitly to "PropAgent" — simulating what a Home-style prop
    // would pass — while showHeader=true mounts the REAL SessionDropdownHeader,
    // which drives internalBoundAgentName to "HeaderAgent" via its own
    // onBoundAgentChange callback. Per the $derived ternary in ChatPanel.svelte
    // (`showHeader ? internalBoundAgentName : (boundAgentNameProp ?? null)`),
    // the header-driven value must win — proving boundAgentNameProp cannot
    // leak onto project surfaces even when explicitly set to a conflicting
    // name, not merely when left at its null default.
    const { container } = render(ChatPanel, {
      messages: [liveMessage],
      onSendMessage: vi.fn(),
      showHeader: true,
      projectId: 'project-test',
      projectName: 'Test Project',
      boundAgentNameProp: 'PropAgent',
    });

    await waitFor(() => {
      const sender = container.querySelector('.chat-sender');
      expect(sender?.textContent).toBe('HeaderAgent');
    });
    expect(container.querySelector('.chat-sender')?.textContent).not.toBe('PropAgent');
  });
});
