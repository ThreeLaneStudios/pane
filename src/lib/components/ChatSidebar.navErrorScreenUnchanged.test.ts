/**
 * ChatSidebar.navErrorScreenUnchanged.test.ts — #3044 item 7
 * (Reed's follow-up on #3043 item 1).
 *
 * `ChatSidebar.errorCaseNoScreenChange.test.ts` (#3043 item 1) proves the
 * "no-screen-change" half via SOURCE-TEXT extraction only: it reads the
 * real `case 'error':` branch and asserts, structurally, that it sets
 * `navError` and contains no `navigateTo(`/`canvasState` reference. That
 * proves the code SHAPE is right; it does not prove the running app
 * actually leaves the screen alone — the banner half already has a real
 * render assertion (`ChatPanel.navError.test.ts`), the no-screen-change
 * half didn't.
 *
 * This test drives the REAL, mounted `ChatSidebar`/`ChatPanel` pair
 * through an actual agent turn that ends with a `navigate` payload the
 * backend already rejected (`nav.error` set — the real-world "unknown
 * view" shape, `decideNavigationAction`'s check 0) by capturing the real
 * `onDone` callback `streamManager.startStream` is called with (mocked at
 * the `streamManager` boundary, same as every other ChatSidebar render
 * test in this file family — the stream transport itself, not
 * `handleNavigateFromStream`, is the seam) and invoking it exactly as the
 * real streaming machinery would on stream completion.
 *
 * Asserts BOTH halves by execution:
 *   1. The error banner actually renders in the DOM with the real message.
 *   2. `canvasState`'s view is byte-identical before and after — the
 *      screen genuinely did not move.
 *
 * RED-ON-REVERT: if `handleNavigateFromStream`'s `case 'error':` ever
 * regressed to also calling `navigateTo(...)` (or any canvasState
 * mutation) before or after setting `navError`, this test's canvasState
 * assertion goes red — something the source-text test alone cannot catch
 * if the added call were, say, wrapped in a syntactically-different form
 * the regex didn't anticipate.
 */

import { describe, it, expect, vi, beforeAll, afterEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/svelte/svelte5';
import { get } from 'svelte/store';
import ChatSidebar from './ChatSidebar.svelte';
import * as appStoreMod from '$lib/stores/app';
import * as apiMod from '$lib/api';
import { canvasState } from '$lib/stores/canvas';

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
vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn().mockResolvedValue(() => {}),
}));
vi.mock('$lib/utils/markdown', () => ({
  renderMarkdown: (html: string) => html,
}));

function activeAgent(overrides: Partial<any> = {}) {
  return {
    id: 'agent-naverr-1', name: 'Nova', slug: 'nova', role: 'main', description: null,
    model: 'claude-sonnet-4-5', is_default: false, is_active: true,
    workspace_path: 'WPagent-naverr-1', soul_hash: null, memory_hash: null,
    config_json: '{}', owner_id: null, runtime_ref: 'oc-main', runtime_type: 'openclaw',
    source: 'openclaw', created_at: '2026-01-01T00:00:00Z', updated_at: '2026-01-01T00:00:00Z',
    ...overrides,
  };
}

function mkHomeSession(overrides: Partial<any> = {}) {
  return {
    id: 'sess-naverr-1', project_id: 'workspace', title: 'Home session', origin: 'user',
    created_at: '2026-08-01T10:00:00Z', last_active_at: '2026-08-22T08:00:00Z', hidden_at: null,
    is_active: true, agent_id: 'agent-naverr-1',
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
    ]),
    aiLoadConfig: vi.fn().mockResolvedValue({ provider: 'openclaw' }),
    listProjects: vi.fn().mockResolvedValue([]),
    sendChatMessage: vi.fn().mockResolvedValue('saved-msg-id-naverr'),
  };
});

vi.mock('$lib/stores/sessionStore', async (importOriginal) => {
  const original = await importOriginal() as any;
  const { writable } = await import('svelte/store');
  const store = writable({ sessions: [], activeSession: null, loading: false, error: null });
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

// The seam: capture the REAL onDone (and other) callbacks ChatSidebar's
// own createStreamCallbacks() builds and passes to startStream — then
// invoke onDone ourselves exactly as the real stream transport would on
// completion. This exercises the REAL handleNavigateFromStream function,
// not a hand-copy of its logic.
let capturedCallbacks: any = null;
vi.mock('$lib/services/streamManager', () => ({
  streamManager: {
    init: vi.fn().mockResolvedValue(undefined),
    consumePendingResult: vi.fn().mockReturnValue(false),
    getBuffer: vi.fn().mockReturnValue(''),
    isStreamingThread: vi.fn().mockReturnValue(false),
    reattachCallbacks: vi.fn(),
    getCapturedContext: vi.fn((threadId: string) => ({ threadId })),
    setPendingResult: vi.fn(),
    startStream: vi.fn((_streamId: string, _threadId: string, _projectId: string | null, callbacks: any) => {
      capturedCallbacks = callbacks;
    }),
  },
}));

import { sessionStore } from '$lib/stores/sessionStore';

afterEach(() => {
  (sessionStore as any).__set({ sessions: [], activeSession: null, loading: false, error: null });
  (sessionStore.getActiveSession as any).mockResolvedValue(mkHomeSession());
  appStoreMod.activeProject.set(null);
  appStoreMod.activeChatThreadId.set(null);
  (apiMod.getAgents as any).mockResolvedValue([activeAgent()]);
  (apiMod.getChatMessages as any).mockResolvedValue([]);
  canvasState.set({ view: 'home', data: {}, context: {} } as any);
  capturedCallbacks = null;
});

describe('#3044 item 7 — a rejected navigate target shows the error banner AND genuinely leaves the screen unchanged (real render + real canvasState, not source-text)', () => {
  it('the screen (canvasState) is byte-identical before and after, and the real error banner renders with the real message', async () => {
    // Known starting screen — a real, specific view+context, not the default.
    const startingState = { view: 'all_tasks', data: {}, context: { projectId: 'proj-should-not-move' } };
    canvasState.set(startingState as any);

    (sessionStore as any).__set({ sessions: [], activeSession: null, loading: false, error: null });
    (sessionStore.getActiveSession as any).mockResolvedValue(mkHomeSession());

    const { container, component } = render(ChatSidebar, { layoutMode: 'centered', alwaysOpen: true });
    appStoreMod.activeChatThreadId.set('sess-naverr-1');
    await (component as any).handleSessionSwitch(mkHomeSession());

    // Send a real message — this is what causes ChatSidebar to call
    // streamManager.startStream(...), which our mock intercepts to capture
    // the real onDone callback.
    const textarea = await screen.findByPlaceholderText(/./) as HTMLTextAreaElement;
    await fireEvent.input(textarea, { target: { value: 'take me to the bogus screen' } });
    await fireEvent.keyDown(textarea, { key: 'Enter' });

    await waitFor(() => {
      expect(capturedCallbacks).not.toBeNull();
      expect(typeof capturedCallbacks.onDone).toBe('function');
    });

    // Re-affirm the starting screen is still exactly what we set — sanity
    // that sending the message itself didn't move anything, before the
    // navigate-error payload even arrives.
    expect(get(canvasState)).toEqual(startingState);

    // Simulate the real stream completing with a navigate payload the
    // BACKEND already rejected (decideNavigationAction check 0: `nav.error`
    // set) — the real "unknown view" shape agent instructions document.
    const REJECTION_MESSAGE = 'Navigation failed: "bogus_view" is not a recognized view.';
    await capturedCallbacks.onDone({
      fullText: 'Sure, let me take you there.',
      model: 'claude-sonnet-4-5',
      agentId: 'agent-naverr-1',
      agentName: 'Nova',
      toolResults: [],
      toolCalls: [],
      navigate: { error: REJECTION_MESSAGE },
    });

    // ── Half 1: the banner actually renders, with the real message ──────
    await waitFor(() => {
      const banner = container.querySelector('.connection-error');
      expect(banner, 'the error banner element must exist in the DOM').not.toBeNull();
      expect(banner?.textContent).toContain(REJECTION_MESSAGE);
    });

    // ── Half 2: the screen genuinely did not change ──────────────────────
    // RED-ON-REVERT: a regression that made case 'error' also call
    // navigateTo(...) or mutate canvasState would flip this assertion,
    // not just the source-text one in errorCaseNoScreenChange.test.ts.
    expect(get(canvasState)).toEqual(startingState);
  });
});
