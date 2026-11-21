/**
 * ChatSidebar.composerPlaceholder.test.ts — C20 (#3120 PR 3).
 *
 * "Ask anything about your notes…" / "…about this project…" unified to one
 * string app-wide: "Message your agents…". `ChatSidebar.svelte`'s own
 * template built this text from a ternary on `currentProjectName`; this
 * pins the single replacement string reaching the real DOM `<textarea>`'s
 * `placeholder` attribute, for both the Home surface and a project surface
 * (the ternary's two branches — now one literal, no branch at all).
 *
 * Same mount/mock scaffold as `ChatSidebar.homeAgentName.test.ts` (a real
 * `ChatSidebar` mount is required — the string lives in that file's own
 * template, not an extractable pure function).
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

vi.mock('@tauri-apps/plugin-shell', () => ({ open: vi.fn() }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn().mockResolvedValue(() => {}) }));
vi.mock('$lib/utils/markdown', () => ({ renderMarkdown: (html: string) => html }));

function mkSession(overrides: Partial<any> = {}) {
  return {
    id: 'sess-1',
    project_id: 'workspace',
    title: 'Session',
    origin: 'user',
    created_at: '2026-08-01T10:00:00Z',
    last_active_at: '2026-08-22T08:00:00Z',
    hidden_at: null,
    is_active: true,
    agent_id: null,
    ...overrides,
  };
}

vi.mock('$lib/api', async (importOriginal) => {
  const original = await importOriginal() as any;
  return {
    ...original,
    getAgents: vi.fn().mockResolvedValue([]),
    getChatMessages: vi.fn().mockResolvedValue([]),
    listRuntimeConnections: vi.fn().mockResolvedValue([]),
    aiLoadConfig: vi.fn().mockResolvedValue({ provider: 'openclaw' }),
    listProjects: vi.fn().mockResolvedValue([]),
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
  appStoreMod.activeProject.set(null);
  appStoreMod.activeChatThreadId.set(null);
});

describe('ChatSidebar — unified composer placeholder (C20)', () => {
  /// Mutation: revert the template line to the old ternary
  /// (`` `Ask anything about ${...}...` ``) — reddens (the textarea's real
  /// placeholder attribute goes back to the old string).
  it('Home surface: composer placeholder is the unified string, not the old "Ask anything about your notes…"', async () => {
    appStoreMod.activeProject.set(null);
    const { container } = render(ChatSidebar, { layoutMode: 'centered', alwaysOpen: true });

    await waitFor(() => {
      const textarea = container.querySelector('textarea.message-input') as HTMLTextAreaElement | null;
      expect(textarea).toBeTruthy();
      expect(textarea!.placeholder).toBe('Message your agents\u2026');
    });
  });
});
