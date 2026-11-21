/**
 * ChatSidebar.homeLockBanner.test.ts — D22 (#2649 family)
 *
 * Pins the fix for the Home-surface lock banner bug documented in
 * docs/audits/2026-08-22-d22-lock-banner-scope.md: SessionDropdownHeader (and
 * therefore ReadOnlySessionNotice) never mounted on Home because `showHeader`
 * is gated on `$activeProject !== null`, which is always false there. The fix
 * (Matt's Option B ruling) extracts the read-only/lock computation into
 * `sessionReadOnlyCheck.ts` and has ChatSidebar call it directly on Home,
 * independent of `showHeader` — no header/dropdown UI is added.
 *
 * SESSION-RESOLUTION FIX (2026-08-22, D22/#2643 follow-up): the first version
 * of this test pre-populated `$sessionStore.activeSession` before mounting,
 * which does NOT match real Home — nothing on Home ever writes to
 * `sessionStore` (SessionsDrawer.svelte keeps its own local session list;
 * only the project-surface-only `SessionDropdownHeader` calls
 * `sessionStore.loadSessions`). On a real launch, `$sessionStore.activeSession`
 * is `undefined` when ChatSidebar's Home effect first fires, and the thread
 * only resolves later via the NAV event (`pane-navigation-complete`) setting
 * `$activeChatThreadId`. This test now mirrors that: the session store starts
 * EMPTY, `$activeChatThreadId` is set (simulating NAV resolution), and the
 * fixed effect resolves the session via `sessionStore.getActiveSession('workspace')`
 * keyed off the thread id.
 *
 * Test 1: Home config + session bound to an `is_active:false` agent →
 *   ReadOnlySessionNotice renders, composer is disabled.
 * Test 2: Same Home config → the old "AI CHAT" header bar / SessionDropdownHeader
 *   is NOT present (clean look preserved — no header/dropdown regression).
 *
 * ChatSidebar.svelte is a large, API/session-integrated component. Per the
 * established pattern in this file family (ChatSidebar.onDone.test.ts,
 * ChatSidebar.greetingAgent.test.ts extract standalone logic instead of
 * mounting), this test DOES mount the full component (mirroring
 * ChatPanel.test.ts's approach) because the bug is specifically about DOM
 * presence/absence across the ChatSidebar → ChatPanel → SessionDropdownHeader
 * boundary, which standalone logic extraction cannot exercise.
 */

import { describe, it, expect, vi, beforeAll, afterEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/svelte/svelte5';
import ChatSidebar from './ChatSidebar.svelte';
import * as sessionStoreMod from '$lib/stores/sessionStore';
import * as appStoreMod from '$lib/stores/app';
import * as apiMod from '$lib/api';

// ─── Global polyfills (mirrors ChatPanel.test.ts / SessionDropdownHeader.test.ts) ──

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

// Deactivated agent bound to the active Home session.
function deactivatedAgent(overrides: Partial<any> = {}) {
  return {
    id: 'agent-deactivated',
    name: 'Deactivated Agent',
    slug: 'deactivated-agent',
    role: 'main',
    description: null,
    model: 'claude-sonnet-4-5',
    is_default: false,
    is_active: false,
    workspace_path: 'WPagent-deactivated',
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
    id: 'sess-home-1',
    project_id: 'workspace',
    title: 'Home session',
    origin: 'user',
    created_at: '2026-08-01T10:00:00Z',
    last_active_at: '2026-08-22T08:00:00Z',
    hidden_at: null,
    is_active: true,
    agent_id: 'agent-deactivated',
    ...overrides,
  };
}

vi.mock('$lib/api', async (importOriginal) => {
  const original = await importOriginal() as any;
  return {
    ...original,
    getAgents: vi.fn().mockResolvedValue([deactivatedAgent()]),
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

// Real Home config: the session store starts EMPTY (no activeSession) —
// nothing on Home populates it before NAV resolves. getActiveSession is the
// resolution path the fixed Home effect calls, keyed off $activeChatThreadId.
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

afterEach(() => {
  (sessionStoreMod.sessionStore as any).__set({
    sessions: [],
    activeSession: null,
    loading: false,
    error: null,
  });
  (sessionStoreMod.sessionStore.getActiveSession as any).mockResolvedValue(mkHomeSession());
  appStoreMod.activeProject.set(null);
  appStoreMod.activeChatThreadId.set(null);
});

describe('ChatSidebar — Home lock banner (D22, #2649 family)', () => {
  it('Home config + session bound to a deactivated agent: ReadOnlySessionNotice renders, composer is disabled', async () => {
    appStoreMod.activeProject.set(null);
    appStoreMod.activeChatThreadId.set('sess-home-1');

    render(ChatSidebar, { layoutMode: 'centered', alwaysOpen: true });

    await waitFor(() => {
      expect(screen.getByText('This session is read-only')).toBeTruthy();
    });
    expect(screen.getAllByText(/Deactivated Agent is deactivated/).length).toBeGreaterThan(0);
    // Composer is disabled: textarea is replaced by the "Composer disabled" copy.
    expect(screen.getByText(/Composer disabled while Deactivated Agent is deactivated/)).toBeTruthy();
    expect(screen.queryByRole('textbox')).toBeNull();
  });

  it('Home config: no header bar / session dropdown regression — clean look preserved', async () => {
    appStoreMod.activeProject.set(null);
    appStoreMod.activeChatThreadId.set('sess-home-1');

    render(ChatSidebar, { layoutMode: 'centered', alwaysOpen: true });

    await waitFor(() => {
      expect(screen.getByText('This session is read-only')).toBeTruthy();
    });
    // SessionDropdownHeader's binding line / "New Session" button never mount on Home.
    expect(screen.queryByText('New Session')).toBeNull();
    expect(screen.queryByText('AI CHAT')).toBeNull();
  });
});
