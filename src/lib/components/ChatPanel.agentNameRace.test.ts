/**
 * ChatPanel.agentNameRace.test.ts — #2604 (session-open agent-name fallback race)
 *
 * Pins the fix documented in
 * docs/audits/2026-08-23-2604-agent-name-fallback-audit.md: on the first
 * message sent immediately after a chat session opens, `activeSession`
 * (session store) can resolve on a faster tick than `allAgents`
 * (`getAgents()`, async) finishes its fetch. `SessionDropdownHeader`'s
 * `$effect` used to fire `onBoundAgentChange?.(boundAgent?.name ?? null)`
 * reactively off that not-yet-loaded `allAgents`, emitting a FALSE `null`
 * that ChatPanel's fallback chain (`message.agent_name ?? sessionBoundAgentName
 * ?? 'Pane'`) then rendered as the literal 'Pane' for the first stream token
 * — self-correcting only once `allAgents` finished loading (coincidentally
 * near stream completion, or on the session's second message).
 *
 * The fix: SessionDropdownHeader tracks `agentsLoaded` (true once
 * `getAgents()` has resolved at least once) and suppresses the null
 * emission until then; it also surfaces `agentsLoaded` itself via
 * `onAgentsLoadedChange`. ChatPanel uses that flag to distinguish "still
 * loading" (render the neutral `chat-sender--loading` treatment, reusing
 * the existing thinking-pulse visual language) from "genuinely no bound
 * agent" (render the literal 'Pane', ADR-0059 D5).
 *
 * These tests exercise the REAL race by controlling `getAgents()`'s
 * resolution timing directly (a manually-resolved Promise) while the
 * session store's `activeSession` is already synchronously populated at
 * mount — the exact ordering the audit traced. showHeader=true so the real
 * SessionDropdownHeader mounts and drives the callback chain end-to-end
 * (not a prop stub), covering the actual component boundary the bug lived
 * in.
 */

import { describe, it, expect, vi, beforeAll, afterEach } from 'vitest';
import { render, waitFor } from '@testing-library/svelte/svelte5';
import ChatPanel from './ChatPanel.svelte';
import * as sessionStoreMod from '$lib/stores/sessionStore';
import * as apiMod from '$lib/api';
import type { Agent, Session, ChatMessage } from '$lib/types';

// ─── Global polyfills (mirrors ChatPanel.test.ts) ──────────────────────────

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

function mkOrAgent(overrides: Partial<Agent> = {}): Agent {
  return {
    id: 'or-agent-1',
    name: 'Research',
    slug: 'research',
    role: 'sub',
    description: null,
    model: 'claude-sonnet-4-5',
    is_default: false,
    is_active: true,
    workspace_path: 'WPor-agent-1',
    soul_hash: null,
    memory_hash: null,
    config_json: '{}',
    owner_id: null,
    runtime_ref: null,
    runtime_type: 'openrouter',
    source: 'manual',
    created_at: '2026-01-01T00:00:00Z',
    updated_at: '2026-01-01T00:00:00Z',
    ...overrides,
  } as Agent;
}

function mkSession(overrides: Partial<Session> = {}): Session {
  return {
    id: 'sess-race-1',
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

// Live-streamed placeholder: exactly what ChatSidebar.svelte:1288-1290 creates
// on session open — no agent_name/agent_id at all, being actively streamed.
function liveStreamingMessage(overrides: Partial<ChatMessage> = {}): ChatMessage {
  return {
    id: 'msg-streaming-1',
    thread_id: 'sess-race-1',
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
    showHeader: true, // mounts the REAL SessionDropdownHeader — this bug lives in that boundary
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

afterEach(() => {
  (sessionStoreMod.sessionStore as any).__set({ sessions: [], activeSession: null, loading: false, error: null });
  (apiMod.getAgents as any).mockReset();
  (apiMod.listRuntimeConnections as any).mockResolvedValue([
    { runtime_type: 'openclaw', connected: true, masked_credential: null, endpoint: null, error: null, agents: [] },
    { runtime_type: 'openrouter', connected: true, masked_credential: null, endpoint: null, error: null, agents: [] },
    { runtime_type: 'anthropic', connected: true, masked_credential: null, endpoint: null, error: null, agents: [] },
    { runtime_type: 'ollama', connected: true, masked_credential: null, endpoint: null, error: null, agents: [] },
  ]);
});

describe('ChatPanel — #2604 first-message-after-session-open agent name race', () => {
  it('OC-bound session: does NOT render "Pane" while getAgents() is in flight, renders the bound agent name once it resolves', async () => {
    const agent = mkOcAgent();
    const session = mkSession({ id: 'sess-oc-race', agent_id: agent.id });

    // activeSession resolves synchronously (session store already populated
    // at mount) — the exact ordering from the audit: activeSession is ready
    // before allAgents.
    setActiveSession(session);

    // getAgents() stays pending until we resolve it manually, simulating the
    // async fetch window during which the old code emitted a false null.
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

    // Real outcome assertion #1: while agents are still loading, the sender
    // label must NOT be the literal 'Pane' — it must show the neutral
    // loading treatment instead.
    await waitFor(() => {
      const sender = container.querySelector('.chat-sender');
      expect(sender).not.toBeNull();
    });
    const senderDuringLoad = container.querySelector('.chat-sender');
    expect(senderDuringLoad?.textContent).not.toBe('Pane');
    expect(senderDuringLoad?.classList.contains('chat-sender--loading')).toBe(true);

    // Now let getAgents() resolve — the roster including the bound agent
    // lands, exactly like the real async sync/fetch completing.
    resolveAgents([agent]);

    // Real outcome assertion #2: once the roster has loaded, the sender
    // label must show the ACTUAL bound agent's name.
    await waitFor(() => {
      const sender = container.querySelector('.chat-sender');
      expect(sender?.textContent).toBe('Nova');
    });
    expect(container.querySelector('.chat-sender')?.classList.contains('chat-sender--loading')).toBe(false);
  });

  it('OR-bound session: does NOT render "Pane" while getAgents() is in flight, renders the bound agent name once it resolves', async () => {
    const agent = mkOrAgent();
    const session = mkSession({ id: 'sess-or-race', agent_id: agent.id });

    setActiveSession(session);

    let resolveAgents!: (agents: Agent[]) => void;
    const agentsPromise = new Promise<Agent[]>((resolve) => { resolveAgents = resolve; });
    (apiMod.getAgents as any).mockReturnValue(agentsPromise);

    const message = liveStreamingMessage({ id: 'msg-streaming-or-1', thread_id: session.id });

    const { container } = render(ChatPanel, {
      ...baseProps(),
      messages: [message],
      streamingMessageId: message.id,
      streamingContent: '',
      projectId: 'project-test',
    });

    await waitFor(() => {
      const sender = container.querySelector('.chat-sender');
      expect(sender).not.toBeNull();
    });
    const senderDuringLoad = container.querySelector('.chat-sender');
    expect(senderDuringLoad?.textContent).not.toBe('Pane');
    expect(senderDuringLoad?.classList.contains('chat-sender--loading')).toBe(true);

    resolveAgents([agent]);

    await waitFor(() => {
      const sender = container.querySelector('.chat-sender');
      expect(sender?.textContent).toBe('Research');
    });
    expect(container.querySelector('.chat-sender')?.classList.contains('chat-sender--loading')).toBe(false);
  });

  it('genuinely unbound session: falls back to "Pane" only AFTER the agent roster has loaded, never before', async () => {
    const session = mkSession({ id: 'sess-unbound-race', agent_id: null });
    setActiveSession(session);

    let resolveAgents!: (agents: Agent[]) => void;
    const agentsPromise = new Promise<Agent[]>((resolve) => { resolveAgents = resolve; });
    (apiMod.getAgents as any).mockReturnValue(agentsPromise);

    const message = liveStreamingMessage({ id: 'msg-streaming-unbound-1', thread_id: session.id });

    const { container } = render(ChatPanel, {
      ...baseProps(),
      messages: [message],
      streamingMessageId: message.id,
      streamingContent: '',
      projectId: 'project-test',
    });

    // Real outcome assertion: during the load window, still NOT 'Pane' even
    // though the session has no bound agent — the roster hasn't loaded yet,
    // so we don't yet know that for certain.
    await waitFor(() => {
      const sender = container.querySelector('.chat-sender');
      expect(sender).not.toBeNull();
    });
    expect(container.querySelector('.chat-sender')?.textContent).not.toBe('Pane');

    // Resolve with an empty roster (agent_id is null, so no lookup would
    // find anything even with a populated roster) — NOW 'Pane' is the
    // correct, genuine ADR-0059 D5 fallback.
    resolveAgents([]);

    await waitFor(() => {
      const sender = container.querySelector('.chat-sender');
      expect(sender?.textContent).toBe('Pane');
    });
  });
});
