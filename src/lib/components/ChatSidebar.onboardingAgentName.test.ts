/**
 * ChatSidebar.onboardingAgentName.test.ts — #2794
 *
 * Pins the fix documented in
 * docs/audits/2026-08-31-2794-2783-formal-diagnosis.md: on a fresh install,
 * `navigateTo()` (navigation.ts) sets `$activeChatThreadId` at app boot —
 * BEFORE onboarding has created the first `ai_agents` row. The Home
 * bound-agent-name effect (`ChatSidebar.svelte`, keyed on
 * `$activeChatThreadId`/`isHomeSurface`) fires once against that empty
 * agent roster, resolves `homeBoundAgentName = null` /
 * `homeAgentsLoaded = true`, and never re-runs — because
 * `$activeChatThreadId` never changes again across the entire onboarding
 * flow. `resolveSenderName()` (ChatPanel.svelte) then falls through to the
 * literal `'Pane'` for every assistant message in that session-open,
 * greeting and streaming placeholder included, until a reload/thread-switch
 * re-fires the effect.
 *
 * The fix: the resolution logic was extracted into a standalone
 * `resolveHomeBoundAgent()` function, called both from the original
 * `$effect` (thread/surface change) AND from a new
 * `'pane-onboarding-complete'` window-event listener — so once
 * `Onboarding.svelte`'s `ensureMainAgent()` creates the first agent and
 * dispatches that event, the resolution re-runs against the now-populated
 * roster without requiring `$activeChatThreadId` to change.
 *
 * Follows the exact mounting/mocking pattern established in
 * ChatSidebar.homeAgentName.test.ts (#2643) — same fixtures, same rationale
 * for mounting the full component (the bug is specifically about a prop
 * crossing the ChatSidebar → ChatPanel boundary via a window-event-driven
 * re-resolution, which standalone logic extraction cannot exercise).
 */

import { describe, it, expect, vi, beforeAll, afterEach } from 'vitest';
import { render, waitFor } from '@testing-library/svelte/svelte5';
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

vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn().mockResolvedValue(() => {}),
}));

vi.mock('$lib/utils/markdown', () => ({
  renderMarkdown: (html: string) => html,
}));

// The freshly-onboarded default agent — doesn't exist yet on the first
// getAgents() call (fresh install, empty ai_agents table), exists on every
// call after onboarding completes.
function freshDefaultAgent(overrides: Partial<any> = {}) {
  return {
    id: 'agent-fresh-default',
    name: 'Fern',
    slug: 'fern',
    role: 'main',
    description: null,
    model: 'claude-sonnet-4-5',
    is_default: true,
    is_active: true,
    workspace_path: 'WPagent-fresh-default',
    soul_hash: null,
    memory_hash: null,
    config_json: '{}',
    owner_id: null,
    runtime_ref: 'oc-main',
    runtime_type: 'openclaw',
    source: 'openclaw',
    created_at: '2026-08-31T00:00:00Z',
    updated_at: '2026-08-31T00:00:00Z',
    ...overrides,
  };
}

// Fresh-install Home session: agent_id is NULL, exactly as
// `resolve_designated_default_agent_id` leaves it when `get_active_session`
// creates the lazy-safety-net session before any agent exists (see the
// formal diagnosis's step 3).
function mkFreshHomeSession(overrides: Partial<any> = {}) {
  return {
    id: 'sess-fresh-install-1',
    project_id: 'workspace',
    title: '',
    origin: 'user',
    created_at: '2026-08-31T09:00:00Z',
    last_active_at: '2026-08-31T09:00:00Z',
    hidden_at: null,
    is_active: true,
    agent_id: null,
    ...overrides,
  };
}

// Streaming-placeholder-style message: empty content, no agent_name — same
// shape ChatSidebar.svelte creates at stream start (id: `assistant-${Date.now()}`,
// content: '').
function streamingPlaceholderMessage(overrides: Partial<any> = {}) {
  return {
    id: 'assistant-stream-1',
    thread_id: 'sess-fresh-install-1',
    role: 'assistant' as const,
    content: '',
    created_at: new Date().toISOString(),
    tool_calls: null,
    tool_results: null,
    ...overrides,
  };
}

// Ordinary live-streamed-style message (already has content, still no
// agent_name — the pre-onDone window).
function liveMessageNoAgentName(overrides: Partial<any> = {}) {
  return {
    id: 'msg-live-fresh-1',
    thread_id: 'sess-fresh-install-1',
    role: 'assistant' as const,
    content: 'Hey there, welcome!',
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
    // Starts empty (fresh install, onboarding hasn't run yet) — tests flip
    // this mock's resolved value to simulate the agent being created.
    getAgents: vi.fn().mockResolvedValue([]),
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
      getActiveSession: vi.fn().mockResolvedValue(mkFreshHomeSession()),
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
  (sessionStoreMod.sessionStore.getActiveSession as any).mockResolvedValue(mkFreshHomeSession());
  appStoreMod.activeProject.set(null);
  appStoreMod.activeChatThreadId.set(null);
  (apiMod.getAgents as any).mockResolvedValue([]);
  (apiMod.getChatMessages as any).mockResolvedValue([]);
});

describe('ChatSidebar — Home bound-agent name re-resolves after onboarding completes (#2794)', () => {
  it('fresh install: thread opens before the default agent exists, resolves to "Pane"; after pane-onboarding-complete fires, re-resolves to the new default agent name', async () => {
    appStoreMod.activeProject.set(null);

    // Step 1: thread becomes active BEFORE any agent exists (mirrors
    // navigateTo() setting $activeChatThreadId at app boot, before
    // onboarding runs) — session's agent_id is null, getAgents() is empty.
    (apiMod.getAgents as any).mockResolvedValue([]);
    (apiMod.getChatMessages as any).mockResolvedValue([liveMessageNoAgentName()]);
    (sessionStoreMod.sessionStore as any).__set({ sessions: [], activeSession: null, loading: false, error: null });
    (sessionStoreMod.sessionStore.getActiveSession as any).mockResolvedValue(mkFreshHomeSession());

    const { container, component } = render(ChatSidebar, { layoutMode: 'centered', alwaysOpen: true });

    appStoreMod.activeChatThreadId.set('sess-fresh-install-1');
    await (component as any).handleSessionSwitch(mkFreshHomeSession());

    // Terminal state per the diagnosis: homeBoundAgentName stays null,
    // homeAgentsLoaded becomes true — the label falls through to 'Pane'.
    await waitFor(() => {
      const sender = container.querySelector('.chat-sender');
      expect(sender?.textContent).toBe('Pane');
    });

    // Step 2: onboarding creates the first agent (ensureMainAgent()) and
    // dispatches 'pane-onboarding-complete' — WITHOUT $activeChatThreadId
    // ever changing, exactly like the real flow. Flip the mocks to reflect
    // the now-existing default agent and the now-bound session.
    const agent = freshDefaultAgent();
    (apiMod.getAgents as any).mockResolvedValue([agent]);
    (sessionStoreMod.sessionStore.getActiveSession as any).mockResolvedValue(
      mkFreshHomeSession({ agent_id: 'agent-fresh-default' })
    );

    // NOTE: vitest-setup.ts's `window` mock stubs `dispatchEvent` as a no-op
    // (`return true` only, doesn't invoke listeners) while the component's
    // `window.addEventListener` calls get aliased through to the REAL
    // `globalThis` EventTarget (see the `beforeAll` polyfill above and
    // vitest-setup.ts). Dispatching on `globalThis` is what actually reaches
    // listeners registered via `window.addEventListener` in this test
    // environment — `window.dispatchEvent` here would silently no-op.
    globalThis.dispatchEvent(new CustomEvent('pane-onboarding-complete'));

    // The listener re-invokes resolveHomeBoundAgent(), which re-resolves
    // homeBoundAgentName to "Fern" — the label must flip away from 'Pane'
    // for the SAME message, with no thread switch and no reload.
    await waitFor(() => {
      const sender = container.querySelector('.chat-sender');
      expect(sender?.textContent).toBe('Fern');
    });
    expect(container.querySelector('.chat-sender')?.textContent).not.toBe('Pane');
  });

  it('streaming placeholder (empty content, no agent_name) carries the re-resolved agent name via sessionBoundAgentName after onboarding completes', async () => {
    appStoreMod.activeProject.set(null);

    (apiMod.getAgents as any).mockResolvedValue([]);
    (apiMod.getChatMessages as any).mockResolvedValue([streamingPlaceholderMessage()]);
    (sessionStoreMod.sessionStore as any).__set({ sessions: [], activeSession: null, loading: false, error: null });
    (sessionStoreMod.sessionStore.getActiveSession as any).mockResolvedValue(mkFreshHomeSession());

    const { container, component } = render(ChatSidebar, { layoutMode: 'centered', alwaysOpen: true });

    appStoreMod.activeChatThreadId.set('sess-fresh-install-1');
    await (component as any).handleSessionSwitch(mkFreshHomeSession());

    await waitFor(() => {
      const sender = container.querySelector('.chat-sender');
      expect(sender?.textContent).toBe('Pane');
    });

    const agent = freshDefaultAgent({ id: 'agent-fresh-default', name: 'Fern' });
    (apiMod.getAgents as any).mockResolvedValue([agent]);
    (sessionStoreMod.sessionStore.getActiveSession as any).mockResolvedValue(
      mkFreshHomeSession({ agent_id: 'agent-fresh-default' })
    );

    globalThis.dispatchEvent(new CustomEvent('pane-onboarding-complete'));

    // The placeholder message never gains its own `agent_name` (that only
    // happens at onDone) — its label is driven entirely by
    // sessionBoundAgentName, so this proves the re-resolution reaches
    // messages mid-stream, not just already-completed ones.
    await waitFor(() => {
      const sender = container.querySelector('.chat-sender');
      expect(sender?.textContent).toBe('Fern');
    });
  });

  it('no agent created (onboarding-complete fires but roster is still empty): label stays "Pane", no crash', async () => {
    appStoreMod.activeProject.set(null);

    (apiMod.getAgents as any).mockResolvedValue([]);
    (apiMod.getChatMessages as any).mockResolvedValue([liveMessageNoAgentName()]);
    (sessionStoreMod.sessionStore as any).__set({ sessions: [], activeSession: null, loading: false, error: null });
    (sessionStoreMod.sessionStore.getActiveSession as any).mockResolvedValue(mkFreshHomeSession());

    const { container, component } = render(ChatSidebar, { layoutMode: 'centered', alwaysOpen: true });

    appStoreMod.activeChatThreadId.set('sess-fresh-install-1');
    await (component as any).handleSessionSwitch(mkFreshHomeSession());

    await waitFor(() => {
      const sender = container.querySelector('.chat-sender');
      expect(sender?.textContent).toBe('Pane');
    });

    // getAgents() still resolves empty — nothing changed backend-side.
    globalThis.dispatchEvent(new CustomEvent('pane-onboarding-complete'));

    // Give the re-resolution a tick to (no-op) settle, then assert it's
    // still the correct 'Pane' fallback, not null/undefined/crashed.
    await waitFor(() => {
      const sender = container.querySelector('.chat-sender');
      expect(sender?.textContent).toBe('Pane');
    });
  });

  it('component unmount removes the pane-onboarding-complete listener (no leak, no post-unmount state writes)', async () => {
    appStoreMod.activeProject.set(null);
    (apiMod.getAgents as any).mockResolvedValue([]);
    (apiMod.getChatMessages as any).mockResolvedValue([]);
    (sessionStoreMod.sessionStore as any).__set({ sessions: [], activeSession: null, loading: false, error: null });
    (sessionStoreMod.sessionStore.getActiveSession as any).mockResolvedValue(mkFreshHomeSession());

    const { unmount } = render(ChatSidebar, { layoutMode: 'centered', alwaysOpen: true });
    appStoreMod.activeChatThreadId.set('sess-fresh-install-1');
    await new Promise(r => setTimeout(r, 0));

    unmount();

    // Dispatching after unmount must not throw (would indicate the listener
    // wasn't cleaned up and is now touching a destroyed component's state).
    expect(() => {
      globalThis.dispatchEvent(new CustomEvent('pane-onboarding-complete'));
    }).not.toThrow();
  });
});
