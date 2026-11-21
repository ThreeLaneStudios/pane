/**
 * ChatPanel.test.ts — Component tests for ChatPanel.svelte
 *
 * CI-testable scope:
 *   ✓ D-4: Fresh-session hint renders when isFreshSession=true and messages=[]
 *   ✓ D-4: Generic placeholder renders when isFreshSession=false
 *   ✓ D-4: Home variant uses FRESH_SESSION_HINT_HOME (projectId === '__global__')
 *   ✓ #2563 B2: read-only banner label derives from the bound agent's own
 *     runtime (agentRuntimeType), agreeing with the header — not from the
 *     retired ai_settings.provider global scalar
 *
 * NOT tested here (live DMG tests — Matt):
 *   - Message streaming
 *   - File attachment flow
 *   - Scroll-to-bottom behaviour
 *   - SessionDropdownHeader interactions
 *
 * §395 PR B — design corrections
 */

import { describe, it, expect, vi, beforeAll, afterEach } from 'vitest';
import { render, screen, waitFor, within } from '@testing-library/svelte/svelte5';
import ChatPanel from './ChatPanel.svelte';
import { FRESH_SESSION_HINT, FRESH_SESSION_HINT_HOME } from '$lib/stores/sessionStore';
import * as sessionStoreMod from '$lib/stores/sessionStore';
import * as apiMod from '$lib/api';

// ─── Global polyfills ─────────────────────────────────────────────────────────

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

// ─── Module mocks ─────────────────────────────────────────────────────────────

// Mock @tauri-apps/plugin-shell (not in vitest-setup)
vi.mock('@tauri-apps/plugin-shell', () => ({
  open: vi.fn(),
}));

// vitest-setup.ts replaces `window` with a plain mock object (not real
// jsdom), so DOMPurify(window) in markdown.ts throws when assistant messages
// with real content are rendered. Unrelated to #2604 — stub renderMarkdown so
// tests exercising the speaker-label fallback (not markdown rendering) can
// mount assistant messages with content.
vi.mock('$lib/utils/markdown', () => ({
  renderMarkdown: (html: string) => html,
}));

// Mock $lib/api so ChatPanel doesn't hit real Tauri commands
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
  // #2563 B2: re-pointed from the retired aiLoadConfig().provider global
  // scalar to per-runtime connectivity (ai_list_runtime_connections).
  listRuntimeConnections: vi.fn().mockResolvedValue([
    { runtime_type: 'openclaw', connected: true, masked_credential: null, endpoint: null, error: null, agents: [] },
    { runtime_type: 'openrouter', connected: true, masked_credential: null, endpoint: null, error: null, agents: [] },
    { runtime_type: 'anthropic', connected: true, masked_credential: null, endpoint: null, error: null, agents: [] },
    { runtime_type: 'ollama', connected: true, masked_credential: null, endpoint: null, error: null, agents: [] },
  ]),
}));

// Mock sessionStore so SessionDropdownHeader (sub-component) doesn't break.
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
      // Test-only setter (mirrors SessionDropdownHeader.test.ts's __set pattern).
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

// ─── Helpers ─────────────────────────────────────────────────────────────────

function baseProps() {
  return {
    messages: [] as any[],
    onSendMessage: vi.fn(),
    showHeader: false,      // skip SessionDropdownHeader for these tests
    projectId: 'project-test',
  };
}

// ─── D-4: Fresh-session hint ──────────────────────────────────────────────────

describe('ChatPanel — D-4 fresh-session hint', () => {
  it('renders FRESH_SESSION_HINT when isFreshSession=true and messages=[]', () => {
    render(ChatPanel, { ...baseProps(), isFreshSession: true });
    expect(screen.getByText(FRESH_SESSION_HINT)).toBeTruthy();
  });

  it('renders generic placeholder when isFreshSession=false and messages=[]', () => {
    render(ChatPanel, {
      ...baseProps(),
      isFreshSession: false,
      placeholder: 'Ask anything...',
    });
    expect(screen.getByText('Ask anything...')).toBeTruthy();
    expect(screen.queryByText(FRESH_SESSION_HINT)).toBeNull();
  });

  // C20 (#3120 PR 3): one placeholder string app-wide. Mutation: revert
  // ChatPanel.svelte's default back to 'Ask anything...' — reddens (the
  // new string is absent).
  it('defaults to the unified "Message your agents…" placeholder when no override is passed', () => {
    render(ChatPanel, { ...baseProps(), isFreshSession: false });
    expect(screen.getByText('Message your agents…')).toBeTruthy();
  });

  // C19 (#3120 PR 3): a job-aware hint, when the caller supplies one,
  // takes priority over the generic FRESH_SESSION_HINT. Mutation: revert
  // the empty-state span back to the unconditional generic hint (drop
  // `jobAwareHint ??`) — reddens (the job-aware text never renders).
  it('renders jobAwareHint instead of the generic FRESH_SESSION_HINT when supplied', () => {
    render(ChatPanel, {
      ...baseProps(),
      isFreshSession: true,
      jobAwareHint: 'Morning brief · Brit · next run tomorrow 07:30. Reports land here.',
    });
    expect(screen.getByText('Morning brief · Brit · next run tomorrow 07:30. Reports land here.')).toBeTruthy();
    expect(screen.queryByText(FRESH_SESSION_HINT)).toBeNull();
  });

  it('falls back to the generic FRESH_SESSION_HINT when jobAwareHint is null (every ordinary interactive session)', () => {
    render(ChatPanel, { ...baseProps(), isFreshSession: true, jobAwareHint: null });
    expect(screen.getByText(FRESH_SESSION_HINT)).toBeTruthy();
  });

  it('renders FRESH_SESSION_HINT_HOME when isFreshSession=true and projectId=__global__', () => {
    render(ChatPanel, {
      ...baseProps(),
      isFreshSession: true,
      projectId: '__global__',
    });
    expect(screen.getByText(FRESH_SESSION_HINT_HOME)).toBeTruthy();
    expect(screen.queryByText(FRESH_SESSION_HINT)).toBeNull();
  });

  it('does not render hint text when messages are present', () => {
    const message = {
      id: 'msg-1',
      thread_id: 'thread-1',
      role: 'user' as const,
      content: 'Hello there',
      created_at: new Date().toISOString(),
      tool_calls: null,
      tool_results: null,
    };
    render(ChatPanel, { ...baseProps(), isFreshSession: true, messages: [message] });
    expect(screen.queryByText(FRESH_SESSION_HINT)).toBeNull();
  });
});

// ─── #2563 B2: read-only banner runtime attribution (ADR-0063 D6) ──────────
//
// Pins the exact contradiction from Matt's DMG: the read-only banner's label
// must be derived from the bound agent's own runtime (agentRuntimeType), the
// same identity source the header uses — never from the retired
// `ai_settings.provider` global scalar. showHeader=true so
// SessionDropdownHeader's onReadOnlyChange wiring actually fires.

describe('ChatPanel — read-only banner runtime attribution', () => {
  function ocAgent(overrides: Partial<any> = {}) {
    return {
      id: 'oc-agent-1',
      name: 'Brit Freed',
      slug: 'brit-freed',
      role: 'main',
      description: null,
      model: 'claude-sonnet-4-5',
      is_default: true,
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
    };
  }

  function mkSession(overrides: Partial<any> = {}) {
    return {
      id: 'sess-1',
      project_id: 'project-test',
      title: 'Test session',
      origin: 'user',
      created_at: '2026-07-01T10:00:00Z',
      last_active_at: '2026-07-09T08:00:00Z',
      hidden_at: null,
      is_active: true,
      agent_id: 'oc-agent-1',
      ...overrides,
    };
  }

  afterEach(() => {
    (sessionStoreMod.sessionStore as any).__set({
      sessions: [],
      activeSession: null,
      loading: false,
      error: null,
    });
  });

  it('OC runtime disconnected: banner names "OpenClaw", agreeing with the header label', async () => {
    const agent = ocAgent();
    (apiMod.getAgents as any).mockResolvedValue([agent]);
    (apiMod.listRuntimeConnections as any).mockResolvedValue([
      { runtime_type: 'openclaw', connected: false, masked_credential: null, endpoint: null, error: 'unreachable', agents: [] },
      { runtime_type: 'openrouter', connected: true, masked_credential: null, endpoint: null, error: null, agents: [] },
      { runtime_type: 'anthropic', connected: true, masked_credential: null, endpoint: null, error: null, agents: [] },
      { runtime_type: 'ollama', connected: true, masked_credential: null, endpoint: null, error: null, agents: [] },
    ]);
    (sessionStoreMod.sessionStore as any).__set({
      sessions: [mkSession()],
      activeSession: mkSession(),
      loading: false,
      error: null,
    });

    render(ChatPanel, { ...baseProps(), showHeader: true });

    // Header binding line ("OpenClaw") and the read-only banner's "disconnected"
    // text must both resolve to the SAME label — never "OpenRouter" or any
    // other stale global-scalar value.
    await waitFor(() => {
      expect(screen.getAllByText('OpenClaw').length).toBeGreaterThan(0);
    });
    await waitFor(() => {
      expect(screen.getByText(/OpenClaw connection is disconnected/)).toBeTruthy();
    });
    expect(screen.queryByText(/OpenRouter/)).toBeNull();
  });

  it('OC runtime connected: no read-only banner at all', async () => {
    const agent = ocAgent();
    (apiMod.getAgents as any).mockResolvedValue([agent]);
    (apiMod.listRuntimeConnections as any).mockResolvedValue([
      { runtime_type: 'openclaw', connected: true, masked_credential: null, endpoint: null, error: null, agents: [] },
      { runtime_type: 'openrouter', connected: true, masked_credential: null, endpoint: null, error: null, agents: [] },
      { runtime_type: 'anthropic', connected: true, masked_credential: null, endpoint: null, error: null, agents: [] },
      { runtime_type: 'ollama', connected: true, masked_credential: null, endpoint: null, error: null, agents: [] },
    ]);
    (sessionStoreMod.sessionStore as any).__set({
      sessions: [mkSession()],
      activeSession: mkSession(),
      loading: false,
      error: null,
    });

    render(ChatPanel, { ...baseProps(), showHeader: true });

    await waitFor(() => {
      expect(screen.getAllByText('OpenClaw').length).toBeGreaterThan(0);
    });
    expect(screen.queryByText('This session is read-only')).toBeNull();
  });
});

// ─── #2604 RC-B: speaker-label fallback chain ────────────────────────────
//
// Pins Matt's repro 3 fix: a message's speaker label must resolve
// message.agent_name → sessionBoundAgentName (the SESSION's own bound agent,
// surfaced via SessionDropdownHeader's onBoundAgentChange) → generic
// legacy-NULL copy — and must NEVER fall back to mainAgentName (the app-wide
// default agent). showHeader=true so SessionDropdownHeader actually mounts
// and its onBoundAgentChange callback fires.

describe('ChatPanel — #2604 RC-B speaker-label fallback', () => {
  function biffAgent(overrides: Partial<any> = {}) {
    return {
      id: 'agent-biff',
      name: 'Biff',
      slug: 'biff',
      role: 'sub',
      description: null,
      model: 'openrouter/some-model',
      is_default: false,
      is_active: true,
      workspace_path: 'WPagent-biff',
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
    };
  }

  function blueAgent(overrides: Partial<any> = {}) {
    return {
      id: 'agent-blue',
      name: 'Blue',
      slug: 'blue',
      role: 'main',
      description: null,
      model: 'claude-sonnet-4-5',
      is_default: true, // Blue is the CURRENT DEFAULT agent
      is_active: true,
      workspace_path: 'WPagent-blue',
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
      id: 'sess-biff',
      project_id: 'project-test',
      title: 'Biff session',
      origin: 'user',
      created_at: '2026-08-01T10:00:00Z',
      last_active_at: '2026-08-20T08:00:00Z',
      hidden_at: null,
      is_active: true,
      agent_id: 'agent-biff', // session is BOUND to Biff
      ...overrides,
    };
  }

  afterEach(() => {
    (sessionStoreMod.sessionStore as any).__set({
      sessions: [],
      activeSession: null,
      loading: false,
      error: null,
    });
  });

  it('Matt repro 3: Biff-bound session, Blue as default — live message (no agent_name yet) labels "Biff", never "Blue"', async () => {
    (apiMod.getMainAgent as any).mockResolvedValue({ name: 'Blue' }); // current default
    (apiMod.getAgents as any).mockResolvedValue([biffAgent(), blueAgent()]);
    (apiMod.listRuntimeConnections as any).mockResolvedValue([
      { runtime_type: 'openclaw', connected: true, masked_credential: null, endpoint: null, error: null, agents: [] },
      { runtime_type: 'openrouter', connected: true, masked_credential: null, endpoint: null, error: null, agents: [] },
      { runtime_type: 'anthropic', connected: true, masked_credential: null, endpoint: null, error: null, agents: [] },
      { runtime_type: 'ollama', connected: true, masked_credential: null, endpoint: null, error: null, agents: [] },
    ]);
    (sessionStoreMod.sessionStore as any).__set({
      sessions: [mkSession()],
      activeSession: mkSession(),
      loading: false,
      error: null,
    });

    // Live-streamed message: agent_name not yet populated by a DB reload
    // (exactly the failure window from the audit — onDone hasn't/couldn't
    // set it, or this render predates that fix landing on this message).
    const liveMessage = {
      id: 'msg-live-1',
      thread_id: 'sess-biff',
      role: 'assistant' as const,
      content: 'Here is my answer.',
      created_at: new Date().toISOString(),
      tool_calls: null,
      tool_results: null,
      // agent_name intentionally absent
    };

    const { container } = render(ChatPanel, { ...baseProps(), showHeader: true, messages: [liveMessage] });

    await waitFor(() => {
      const sender = container.querySelector('.chat-sender');
      expect(sender?.textContent).toBe('Biff');
    });
    expect(container.querySelector('.chat-sender')?.textContent).not.toBe('Blue');
  });

  it('default switch mid-session does not relabel existing messages (no pane-agent-switched reactivity)', async () => {
    (apiMod.getMainAgent as any).mockResolvedValue({ name: 'Blue' });
    (apiMod.getAgents as any).mockResolvedValue([biffAgent(), blueAgent()]);
    (apiMod.listRuntimeConnections as any).mockResolvedValue([
      { runtime_type: 'openclaw', connected: true, masked_credential: null, endpoint: null, error: null, agents: [] },
      { runtime_type: 'openrouter', connected: true, masked_credential: null, endpoint: null, error: null, agents: [] },
      { runtime_type: 'anthropic', connected: true, masked_credential: null, endpoint: null, error: null, agents: [] },
      { runtime_type: 'ollama', connected: true, masked_credential: null, endpoint: null, error: null, agents: [] },
    ]);
    (sessionStoreMod.sessionStore as any).__set({
      sessions: [mkSession()],
      activeSession: mkSession(),
      loading: false,
      error: null,
    });

    const liveMessage = {
      id: 'msg-live-2',
      thread_id: 'sess-biff',
      role: 'assistant' as const,
      content: 'Here is my answer.',
      created_at: new Date().toISOString(),
      tool_calls: null,
      tool_results: null,
    };

    const { container } = render(ChatPanel, { ...baseProps(), showHeader: true, messages: [liveMessage] });

    await waitFor(() => {
      const sender = container.querySelector('.chat-sender');
      expect(sender?.textContent).toBe('Biff');
    });

    // Simulate the user switching the app-wide DEFAULT agent elsewhere in the
    // app while this bound-session ChatPanel is still open.
    (apiMod.getMainAgent as any).mockResolvedValue({ name: 'SomeOtherAgent' });
    window.dispatchEvent(new Event('pane-agent-switched'));

    // Give any (incorrect) reactive listener a tick to fire, then assert the
    // label is STILL "Biff" — not relabeled to the new default.
    await new Promise((r) => setTimeout(r, 10));
    expect(container.querySelector('.chat-sender')?.textContent).toBe('Biff');
    expect(container.querySelector('.chat-sender')?.textContent).not.toBe('SomeOtherAgent');
  });

  it('message.agent_name (DB-attributed, authoritative) always wins over the session-bound-agent fallback', async () => {
    (apiMod.getMainAgent as any).mockResolvedValue({ name: 'Blue' });
    (apiMod.getAgents as any).mockResolvedValue([biffAgent(), blueAgent()]);
    (apiMod.listRuntimeConnections as any).mockResolvedValue([
      { runtime_type: 'openclaw', connected: true, masked_credential: null, endpoint: null, error: null, agents: [] },
      { runtime_type: 'openrouter', connected: true, masked_credential: null, endpoint: null, error: null, agents: [] },
      { runtime_type: 'anthropic', connected: true, masked_credential: null, endpoint: null, error: null, agents: [] },
      { runtime_type: 'ollama', connected: true, masked_credential: null, endpoint: null, error: null, agents: [] },
    ]);
    (sessionStoreMod.sessionStore as any).__set({
      sessions: [mkSession()],
      activeSession: mkSession(),
      loading: false,
      error: null,
    });

    // A reloaded message with the real DB-attributed agent_name already set
    // (e.g. from an older sub-agent that once ran in this session, distinct
    // from the CURRENT session-bound agent Biff).
    const reloadedMessage = {
      id: 'msg-reloaded-1',
      thread_id: 'sess-biff',
      role: 'assistant' as const,
      content: 'An older message.',
      created_at: new Date().toISOString(),
      tool_calls: null,
      tool_results: null,
      agent_name: 'HistoricalSubAgent',
    };

    const { container } = render(ChatPanel, { ...baseProps(), showHeader: true, messages: [reloadedMessage] });

    await waitFor(() => {
      const sender = container.querySelector('.chat-sender');
      expect(sender?.textContent).toBe('HistoricalSubAgent');
    });
    expect(container.querySelector('.chat-sender')?.textContent).not.toBe('Biff');
    expect(container.querySelector('.chat-sender')?.textContent).not.toBe('Blue');
  });
});

// ─── §36.2.3 (#3050 PR 6) — composer Send/Stop toggle ──────────────────────
//
// The composer was Enter-only before this PR: zero buttons while idle, one
// (Stop) while streaming, appearing/disappearing in an otherwise-empty
// composer (the "floating button" live-test note). Send/Stop now occupy
// the SAME slot, toggling on `isStreaming`.

describe('ChatPanel — composer Send/Stop toggle (§36.2.3, #3050 PR 6)', () => {
  /// Idle state — new behavior (before this PR, nothing rendered here at
  /// all). Mutation: revert the `{:else}` branch to render nothing (the
  /// pre-PR-6 shape) → this test's `getByText('Send')` reddens.
  it('shows "Send" when idle (new — previously showed nothing)', () => {
    render(ChatPanel, { ...baseProps(), isStreaming: false, onStop: vi.fn() });
    expect(screen.getByText('Send')).toBeTruthy();
    expect(screen.queryByText('Stop')).toBeNull();
  });

  /// Streaming state — pre-existing assertion (§36.2.3 PR 3), now proven
  /// against the TOGGLE rather than a conditionally-rendered standalone
  /// element. Mutation: revert the `isStreaming && onStop` guard to always
  /// render Send → this test's `queryByText('Send')` assertion reddens
  /// (both buttons would render, or Send would render instead of Stop).
  it('shows "Stop" when streaming (unchanged from pre-PR-6 behavior)', () => {
    render(ChatPanel, { ...baseProps(), isStreaming: true, onStop: vi.fn() });
    expect(screen.getByText('Stop')).toBeTruthy();
    expect(screen.queryByText('Send')).toBeNull();
  });

  /// Clicking "Send" when idle submits the composer's current text — parity
  /// with Enter (the pre-existing `handleKeyDown` path, untouched by this
  /// PR). Mutation: remove the `onclick={sendMessage}` wiring on the Send
  /// button → `onSendMessage` is never called, this test reddens.
  it('clicking "Send" when idle submits the composer\'s current text (parity with Enter)', async () => {
    const onSendMessage = vi.fn();
    const { container } = render(ChatPanel, { ...baseProps(), onSendMessage, isStreaming: false, onStop: vi.fn() });

    const textarea = container.querySelector('textarea.message-input') as HTMLTextAreaElement;
    expect(textarea).toBeTruthy();
    await textarea.focus();
    textarea.value = 'hello from the send button';
    textarea.dispatchEvent(new Event('input', { bubbles: true }));

    const sendBtn = () => screen.getByText('Send').closest('button') as HTMLButtonElement;
    await waitFor(() => expect(sendBtn().disabled).toBe(false));
    sendBtn().click();

    expect(onSendMessage).toHaveBeenCalledWith('hello from the send button', undefined);
  });

  /// Send is disabled (not just inert) when there's nothing to send — same
  /// guard `sendMessage()`'s own early-return already enforces for Enter,
  /// now visible on the button itself too.
  it('disables "Send" when the composer is empty', () => {
    render(ChatPanel, { ...baseProps(), isStreaming: false, onStop: vi.fn() });
    const sendBtn = screen.getByText('Send').closest('button') as HTMLButtonElement;
    expect(sendBtn.disabled).toBe(true);
  });

  /// Clicking "Stop" (streaming state) still calls the existing `onStop`
  /// callback — regression pin for the pre-PR-6 Stop behavior, now reached
  /// through the toggle's `{#if isStreaming && onStop}` branch instead of
  /// a standalone conditional element.
  it('clicking "Stop" when streaming calls onStop (regression pin, pre-PR-6 behavior)', () => {
    const onStop = vi.fn();
    render(ChatPanel, { ...baseProps(), isStreaming: true, onStop });
    screen.getByText('Stop').closest('button')?.dispatchEvent(new MouseEvent('click', { bubbles: true }));
    expect(onStop).toHaveBeenCalledOnce();
  });
});
