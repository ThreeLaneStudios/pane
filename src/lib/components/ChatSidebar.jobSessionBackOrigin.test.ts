/**
 * ChatSidebar.jobSessionBackOrigin.test.ts — #3155 C49b.
 *
 * The origin-gate half of the fix (`showBackButton` requiring the active
 * session's own `origin` to be a job session's) is a pure function,
 * `shouldShowJobSessionBackButton` (`$lib/services/jobSessionBack.ts`),
 * and is tested directly there — no component mount needed, see that
 * file's own test additions.
 *
 * This file covers the OTHER half: `handleSessionSwitch` (the ONE
 * session-switch path that bypasses `navigateTo` entirely — the drawer/
 * sidebar manual-switch path) must clear `jobSessionBackTarget` as its
 * own first step, unconditionally, regardless of which session it's
 * switching to. Mounts the full `ChatSidebar` component — same
 * justification as `ChatSidebar.viewingSession.test.ts`:
 * `handleSessionSwitch` is an exported method on the real component
 * instance, not something a standalone logic extraction could exercise.
 *
 * Mutation (the exact bug this closes): remove the
 * `jobSessionBackTarget.set(null)` line from `handleSessionSwitch` — the
 * target survives a manual switch — this test reddens (reads back
 * 'prev-session-id' instead of `null`).
 */
import { describe, it, expect, vi, beforeAll, afterEach } from 'vitest';
import { render } from '@testing-library/svelte/svelte5';
import { get } from 'svelte/store';
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

vi.mock('@tauri-apps/plugin-shell', () => ({ open: vi.fn() }));
vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn().mockResolvedValue(() => {}),
}));
vi.mock('$lib/utils/markdown', () => ({ renderMarkdown: (html: string) => html }));

function activeAgent(overrides: Partial<any> = {}) {
  return {
    id: 'agent-active-1', name: 'Nova', slug: 'nova', role: 'main', description: null,
    model: 'claude-sonnet-4-5', is_default: false, is_active: true, workspace_path: 'WPagent-active-1',
    soul_hash: null, memory_hash: null, config_json: '{}', owner_id: null,
    runtime_ref: 'oc-main', runtime_type: 'openclaw', source: 'openclaw',
    created_at: '2026-01-01T00:00:00Z', updated_at: '2026-01-01T00:00:00Z',
    ...overrides,
  };
}

function mkSession(overrides: Partial<any> = {}) {
  return {
    id: 'sess-target-1', project_id: 'workspace', title: 'Target session', origin: 'user',
    created_at: '2026-01-01T10:00:00Z', last_active_at: '2026-10-29T08:00:00Z',
    hidden_at: null, is_active: true, agent_id: 'agent-active-1',
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
  appStoreMod.activeSessionOrigin.set(null);
  appStoreMod.jobSessionBackTarget.set(null);
  (apiMod.getAgents as any).mockResolvedValue([activeAgent()]);
  (apiMod.getChatMessages as any).mockResolvedValue([]);
  (apiMod.setViewingSession as any).mockClear();
});

describe('#3155 C49b — handleSessionSwitch clears the job-session back target', () => {
  it('clears jobSessionBackTarget on ANY manual navigation, unconditionally, regardless of the switched-to session', async () => {
    const { component } = render(ChatSidebar, { layoutMode: 'centered', alwaysOpen: true });

    appStoreMod.jobSessionBackTarget.set('prev-session-id');
    expect(get(appStoreMod.jobSessionBackTarget)).toBe('prev-session-id');

    await (component as any).handleSessionSwitch(mkSession({ id: 'aurum-session', origin: 'user' }));

    expect(
      get(appStoreMod.jobSessionBackTarget),
      'handleSessionSwitch must clear the back target at its own first step'
    ).toBeNull();
  });

  it('also clears the target when switching to another job session (unconditional — not origin-gated)', async () => {
    const { component } = render(ChatSidebar, { layoutMode: 'centered', alwaysOpen: true });

    appStoreMod.jobSessionBackTarget.set('prev-session-id');

    await (component as any).handleSessionSwitch(mkSession({ id: 'another-job-session', origin: 'spawn' }));

    expect(get(appStoreMod.jobSessionBackTarget)).toBeNull();
  });

  it("mirrors the switched-to session's own origin into activeSessionOrigin", async () => {
    const { component } = render(ChatSidebar, { layoutMode: 'centered', alwaysOpen: true });

    await (component as any).handleSessionSwitch(mkSession({ id: 'job-session-x', origin: 'spawn' }));

    expect(get(appStoreMod.activeSessionOrigin)).toBe('spawn');
  });
});
