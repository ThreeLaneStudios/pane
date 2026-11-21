/**
 * ChatSidebar.homeAgentNameRace.test.ts — #2604 (Home-surface load-order race)
 *
 * Companion to ChatSidebar.homeAgentName.test.ts (#2643, which pins the
 * boundAgentNameProp plumbing itself) and ChatPanel.agentNameRace.test.ts
 * (project-surface / SessionDropdownHeader race). This file pins the SAME
 * class of bug on the Home surface's own mirror path (ChatSidebar.svelte
 * ~178-230): `sessionStore.getActiveSession('workspace')` can resolve before
 * `api.getAgents()` finishes, and the fix must not let that window render
 * the literal 'Pane' for the first stream token.
 *
 * The fix: ChatSidebar tracks `homeAgentsLoaded` (mirrors
 * SessionDropdownHeader's `agentsLoaded`) and passes it to ChatPanel as
 * `agentsLoadedProp`. ChatPanel's fallback chain only reaches the 'Pane'
 * literal once `agentsLoadedProp` is true.
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

vi.mock('@tauri-apps/plugin-shell', () => ({
  open: vi.fn(),
}));

vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn().mockResolvedValue(() => {}),
}));

vi.mock('$lib/utils/markdown', () => ({
  renderMarkdown: (html: string) => html,
}));

function activeAgent(overrides: Partial<any> = {}) {
  return {
    id: 'agent-active-race-1',
    name: 'Nova',
    slug: 'nova',
    role: 'main',
    description: null,
    model: 'claude-sonnet-4-5',
    is_default: false,
    is_active: true,
    workspace_path: 'WPagent-active-race-1',
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
    id: 'sess-home-race-1',
    project_id: 'workspace',
    title: 'Home session',
    origin: 'user',
    created_at: '2026-08-23T10:00:00Z',
    last_active_at: '2026-08-23T10:00:00Z',
    hidden_at: null,
    is_active: true,
    agent_id: 'agent-active-race-1',
    ...overrides,
  };
}

function liveMessageNoAgentName(overrides: Partial<any> = {}) {
  return {
    id: 'msg-live-home-race-1',
    thread_id: 'sess-home-race-1',
    role: 'assistant' as const,
    content: 'Here is my answer.',
    created_at: new Date().toISOString(),
    tool_calls: null,
    tool_results: null,
    ...overrides,
  };
}

vi.mock('$lib/api', async (importOriginal) => {
  const original = await importOriginal() as any;
  return {
    ...original,
    getAgents: vi.fn(),
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
      // getActiveSession resolves IMMEDIATELY (synchronously-ish, on the
      // microtask queue) — this is the "activeSession resolves before
      // allAgents" half of the race. getAgents() is controlled per-test.
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
  (apiMod.getAgents as any).mockReset();
  (apiMod.getChatMessages as any).mockResolvedValue([]);
});

describe('ChatSidebar (Home) — #2604 first-message-after-session-open agent name race', () => {
  it('Home session bound to an agent: does NOT show "Pane" while getAgents() is in flight, self-corrects to the bound agent name once it resolves', async () => {
    appStoreMod.activeProject.set(null);

    let resolveAgents!: (agents: any[]) => void;
    const agentsPromise = new Promise<any[]>((resolve) => { resolveAgents = resolve; });
    (apiMod.getAgents as any).mockReturnValue(agentsPromise);
    (apiMod.getChatMessages as any).mockResolvedValue([liveMessageNoAgentName()]);

    (sessionStoreMod.sessionStore as any).__set({ sessions: [], activeSession: null, loading: false, error: null });
    (sessionStoreMod.sessionStore.getActiveSession as any).mockResolvedValue(mkHomeSession());

    const { container, component } = render(ChatSidebar, { layoutMode: 'centered', alwaysOpen: true });

    appStoreMod.activeChatThreadId.set('sess-home-race-1');
    await (component as any).handleSessionSwitch(mkHomeSession());

    // Real outcome assertion #1: while getAgents() is unresolved, the
    // Home-mirrored path must not fall through to 'Pane' — same guarantee
    // as the project-surface (SessionDropdownHeader) path.
    await waitFor(() => {
      const sender = container.querySelector('.chat-sender');
      expect(sender).not.toBeNull();
    });
    expect(container.querySelector('.chat-sender')?.textContent).not.toBe('Pane');

    // Now resolve the agent roster — the real async sync/fetch landing.
    resolveAgents([activeAgent()]);

    // Real outcome assertion #2: the bound agent's actual name renders.
    await waitFor(() => {
      const sender = container.querySelector('.chat-sender');
      expect(sender?.textContent).toBe('Nova');
    });
  });
});
