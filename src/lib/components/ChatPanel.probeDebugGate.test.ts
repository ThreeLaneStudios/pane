/**
 * ChatPanel.probeDebugGate.test.ts — #2682 (gate [2604-PROBE] console
 * markers behind a debug flag)
 *
 * Root problem: the `[2604-PROBE]` diagnostic markers added by #2604 (see
 * ChatPanel.agentNameRace.test.ts / docs/audits/2026-08-23-2604-agent-name-
 * fallback-audit.md) were shipped as unconditional `console.log` calls —
 * useful for the live repro at the time, but left permanently polluting
 * production console output on every session's first stream token and every
 * `SessionDropdownHeader` mount.
 *
 * Fix: both call sites (`ChatPanel.svelte`'s `resolveSenderName` first-token
 * probe, and `SessionDropdownHeader.svelte`'s `agentsLoaded`-resolved /
 * suppressed-null-emission probes) are now gated behind the SAME
 * `pane-debug` localStorage flag already used elsewhere in the codebase
 * (`ChatSidebar.svelte`, `Editor.svelte`, `KanbanBoard.svelte`,
 * `NoteList.svelte`, `Sidebar.svelte`, `CanvasRenderer.svelte`,
 * `NotesBrowser.svelte`, `noteService.ts`) — no new flag mechanism invented.
 *
 * These tests exercise the REAL race exactly like
 * `ChatPanel.agentNameRace.test.ts` (mounting the real SessionDropdownHeader
 * via showHeader=true, controlling `getAgents()`'s resolution timing
 * directly) so all three probe call sites in that flow fire during the
 * test, then assert on `console.log` calls filtered to the `[2604-PROBE]`
 * prefix — never on the underlying resolution logic itself, which is
 * already covered by ChatPanel.agentNameRace.test.ts and is explicitly NOT
 * what this fix changes.
 */

import { describe, it, expect, vi, beforeAll, afterEach } from 'vitest';
import { render, waitFor } from '@testing-library/svelte/svelte5';
import ChatPanel from './ChatPanel.svelte';
import * as sessionStoreMod from '$lib/stores/sessionStore';
import * as apiMod from '$lib/api';
import type { Agent, Session, ChatMessage } from '$lib/types';

// ─── Global polyfills (mirrors ChatPanel.agentNameRace.test.ts) ───────────

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

vi.mock('$lib/api', () => ({
  getAgents: vi.fn(),
  listRuntimeConnections: vi.fn().mockResolvedValue([
    { runtime_type: 'openclaw', connected: true, masked_credential: null, endpoint: null, error: null, agents: [] },
    { runtime_type: 'openrouter', connected: true, masked_credential: null, endpoint: null, error: null, agents: [] },
    { runtime_type: 'anthropic', connected: true, masked_credential: null, endpoint: null, error: null, agents: [] },
    { runtime_type: 'ollama', connected: true, masked_credential: null, endpoint: null, error: null, agents: [] },
  ]),
}));

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

// ─── Fixtures ───────────────────────────────────────────────────────────────

function mkOcAgent(overrides: Partial<Agent> = {}): Agent {
  return {
    id: 'oc-agent-1',
    name: 'Nova',
    slug: 'nova',
    role: 'main',
    description: null,
    model: 'claude-sonnet-4-5',
    is_default: false,
    is_active: true,
    workspace_path: 'WPoc-agent-1',
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
  } as Agent;
}

function mkSession(overrides: Partial<Session> = {}): Session {
  return {
    id: 'sess-probe-gate-1',
    project_id: 'project-test',
    title: 'Freshly opened session',
    origin: 'user',
    created_at: '2026-08-23T10:00:00Z',
    last_active_at: '2026-08-23T10:00:00Z',
    hidden_at: null,
    is_active: true,
    agent_id: null,
    ...overrides,
  } as Session;
}

// Live-streamed placeholder: exactly what ChatSidebar.svelte creates on
// session open — no agent_name/agent_id at all, being actively streamed.
// This is the shape that drives ChatPanel's first-token probe call site.
function liveStreamingMessage(overrides: Partial<ChatMessage> = {}): ChatMessage {
  return {
    id: 'msg-streaming-probe-1',
    thread_id: 'sess-probe-gate-1',
    role: 'assistant',
    content: '',
    created_at: new Date().toISOString(),
    tool_calls: null,
    tool_results: null,
    ...overrides,
  } as ChatMessage;
}

function baseProps(overrides: Record<string, any> = {}) {
  return {
    messages: [] as ChatMessage[],
    onSendMessage: vi.fn(),
    showHeader: true, // mounts the REAL SessionDropdownHeader — both probe sites live here
    projectId: 'project-test',
    ...overrides,
  };
}

function setActiveSession(session: Session) {
  (sessionStoreMod.sessionStore as any).__set({
    sessions: [session],
    activeSession: session,
    loading: false,
    error: null,
  });
}

/**
 * Renders ChatPanel with the exact race-condition timing that fires all
 * probe call sites: activeSession resolves synchronously, getAgents() stays
 * pending (fires SessionDropdownHeader's "suppressed null emission" probe
 * and ChatPanel's first-token probe while loading), then resolves (fires
 * SessionDropdownHeader's "agentsLoaded=true" probe).
 */
async function renderAndDriveProbeSites() {
  const agent = mkOcAgent();
  const session = mkSession({ agent_id: agent.id });
  setActiveSession(session);

  let resolveAgents!: (agents: Agent[]) => void;
  const agentsPromise = new Promise<Agent[]>((resolve) => { resolveAgents = resolve; });
  (apiMod.getAgents as any).mockReturnValue(agentsPromise);

  const message = liveStreamingMessage({ thread_id: session.id });

  const { container } = render(ChatPanel, {
    ...baseProps(),
    messages: [message],
    streamingMessageId: message.id,
    streamingContent: '',
    projectId: 'project-test',
  });

  // Wait for the loading-state render (drives the "suppressed null
  // emission" probe + ChatPanel's first-token probe while agents are
  // still in flight).
  await waitFor(() => {
    const sender = container.querySelector('.chat-sender');
    expect(sender).not.toBeNull();
  });

  // Resolve getAgents() (drives the "agentsLoaded=true" probe).
  resolveAgents([agent]);

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
  (apiMod.getAgents as any).mockReset();
  (apiMod.listRuntimeConnections as any).mockResolvedValue([
    { runtime_type: 'openclaw', connected: true, masked_credential: null, endpoint: null, error: null, agents: [] },
    { runtime_type: 'openrouter', connected: true, masked_credential: null, endpoint: null, error: null, agents: [] },
    { runtime_type: 'anthropic', connected: true, masked_credential: null, endpoint: null, error: null, agents: [] },
    { runtime_type: 'ollama', connected: true, masked_credential: null, endpoint: null, error: null, agents: [] },
  ]);
  localStorage.removeItem('pane-debug');
});

describe('#2682 — [2604-PROBE] console markers gated behind pane-debug flag', () => {
  it('markers ABSENT by default: no [2604-PROBE] console.log calls without the flag set', async () => {
    localStorage.removeItem('pane-debug');
    const logSpy = vi.spyOn(console, 'log').mockImplementation(() => {});

    try {
      await renderAndDriveProbeSites();
      expect(probeLogCalls(logSpy)).toHaveLength(0);
    } finally {
      logSpy.mockRestore();
    }
  });

  it('markers PRESENT with the flag enabled: all three probe call sites (ChatPanel first-token, SessionDropdownHeader suppressed-null-emission, SessionDropdownHeader agentsLoaded) fire [2604-PROBE] console.log calls', async () => {
    localStorage.setItem('pane-debug', '1');
    const logSpy = vi.spyOn(console, 'log').mockImplementation(() => {});

    try {
      await renderAndDriveProbeSites();

      const calls = probeLogCalls(logSpy);
      expect(calls.length).toBeGreaterThan(0);

      const messages = calls.map(call => call[0] as string);
      expect(messages).toContain('[2604-PROBE] first-token render for message');
      expect(messages).toContain('[2604-PROBE] SessionDropdownHeader effect: suppressed null emission (agentsLoaded=false)');
      expect(messages).toContain('[2604-PROBE] SessionDropdownHeader agentsLoaded=true (getAgents resolved)');
    } finally {
      logSpy.mockRestore();
    }
  });

  it('flag value other than "1" (e.g. unset after being toggled off) keeps markers absent', async () => {
    localStorage.setItem('pane-debug', '0');
    const logSpy = vi.spyOn(console, 'log').mockImplementation(() => {});

    try {
      await renderAndDriveProbeSites();
      expect(probeLogCalls(logSpy)).toHaveLength(0);
    } finally {
      logSpy.mockRestore();
    }
  });
});
