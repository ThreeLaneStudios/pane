/**
 * ChatSidebar.probeDebugGate.test.ts — #2682 (gate [2604-PROBE] console
 * markers behind a debug flag, Home-surface call site)
 *
 * Companion to ChatPanel.probeDebugGate.test.ts (project-surface /
 * SessionDropdownHeader call sites). Pins the same fix for ChatSidebar.svelte's
 * own Home-surface mirror probe (`homeAgentsLoaded` resolution, ~line 230),
 * which fired unconditionally before #2682 and is now gated behind
 * ChatSidebar's existing `DEBUG` const (the same `pane-debug` localStorage
 * flag already used by Editor.svelte, KanbanBoard.svelte, etc.).
 *
 * Race-condition setup mirrors ChatSidebar.homeAgentNameRace.test.ts exactly
 * (controls getAgents()'s resolution timing directly) since that's the
 * scenario that drives ChatSidebar's probe call site.
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
    id: 'agent-active-probe-gate-1',
    name: 'Nova',
    slug: 'nova',
    role: 'main',
    description: null,
    model: 'claude-sonnet-4-5',
    is_default: false,
    is_active: true,
    workspace_path: 'WPagent-active-probe-gate-1',
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
    id: 'sess-home-probe-gate-1',
    project_id: 'workspace',
    title: 'Home session',
    origin: 'user',
    created_at: '2026-08-23T10:00:00Z',
    last_active_at: '2026-08-23T10:00:00Z',
    hidden_at: null,
    is_active: true,
    agent_id: 'agent-active-probe-gate-1',
    ...overrides,
  };
}

function liveMessageNoAgentName(overrides: Partial<any> = {}) {
  return {
    id: 'msg-live-home-probe-gate-1',
    thread_id: 'sess-home-probe-gate-1',
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

/**
 * Renders ChatSidebar (Home) with the exact race-condition timing that
 * drives ChatSidebar's own [2604-PROBE] call site: getAgents() stays
 * pending, then resolves, firing the "homeAgentsLoaded=true" probe.
 */
async function renderAndDriveHomeProbeSite() {
  appStoreMod.activeProject.set(null);

  let resolveAgents!: (agents: any[]) => void;
  const agentsPromise = new Promise<any[]>((resolve) => { resolveAgents = resolve; });
  (apiMod.getAgents as any).mockReturnValue(agentsPromise);
  (apiMod.getChatMessages as any).mockResolvedValue([liveMessageNoAgentName()]);

  (sessionStoreMod.sessionStore as any).__set({ sessions: [], activeSession: null, loading: false, error: null });
  (sessionStoreMod.sessionStore.getActiveSession as any).mockResolvedValue(mkHomeSession());

  const { container, component } = render(ChatSidebar, { layoutMode: 'centered', alwaysOpen: true });

  appStoreMod.activeChatThreadId.set('sess-home-probe-gate-1');
  await (component as any).handleSessionSwitch(mkHomeSession());

  await waitFor(() => {
    const sender = container.querySelector('.chat-sender');
    expect(sender).not.toBeNull();
  });

  resolveAgents([activeAgent()]);

  await waitFor(() => {
    const sender = container.querySelector('.chat-sender');
    expect(sender?.textContent).toBe('Nova');
  });

  return container;
}

function probeLogCalls(spy: ReturnType<typeof vi.spyOn>): unknown[][] {
  return spy.mock.calls.filter(call => typeof call[0] === 'string' && call[0].startsWith('[2604-PROBE]'));
}

afterEach(() => {
  (sessionStoreMod.sessionStore as any).__set({ sessions: [], activeSession: null, loading: false, error: null });
  (sessionStoreMod.sessionStore.getActiveSession as any).mockResolvedValue(mkHomeSession());
  appStoreMod.activeProject.set(null);
  appStoreMod.activeChatThreadId.set(null);
  (apiMod.getAgents as any).mockReset();
  (apiMod.getChatMessages as any).mockResolvedValue([]);
  localStorage.removeItem('pane-debug');
});

describe('#2682 — ChatSidebar (Home) [2604-PROBE] marker gated behind pane-debug flag', () => {
  it('marker ABSENT by default: no [2604-PROBE] console.log call without the flag set', async () => {
    localStorage.removeItem('pane-debug');
    const logSpy = vi.spyOn(console, 'log').mockImplementation(() => {});

    try {
      await renderAndDriveHomeProbeSite();
      expect(probeLogCalls(logSpy)).toHaveLength(0);
    } finally {
      logSpy.mockRestore();
    }
  });

  it('marker PRESENT with the flag enabled: "[2604-PROBE] ChatSidebar (Home) agentsLoaded=true" fires', async () => {
    localStorage.setItem('pane-debug', '1');
    const logSpy = vi.spyOn(console, 'log').mockImplementation(() => {});

    try {
      await renderAndDriveHomeProbeSite();

      const calls = probeLogCalls(logSpy);
      expect(calls.length).toBeGreaterThan(0);
      const messages = calls.map(call => call[0] as string);
      expect(messages).toContain('[2604-PROBE] ChatSidebar (Home) agentsLoaded=true (getAgents resolved)');
    } finally {
      logSpy.mockRestore();
    }
  });
});
