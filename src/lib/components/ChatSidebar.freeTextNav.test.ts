/**
 * ChatSidebar.freeTextNav.test.ts — #3044 item 4.
 *
 * Matt's ruling: every chat message goes to the agent. The old `navMap`
 * lookup + `fuzzyMatch` fallback + "go to [project]" regex + fake
 * "Done — navigated" assistant replies inside `handleSendMessage`
 * (ChatSidebar.svelte) intercepted phrases like "go to home" BEFORE they
 * ever reached the agent, and fabricated an assistant reply the agent
 * never produced. That whole block is deleted outright (not gated) in this
 * commit.
 *
 * This test drives the REAL, mounted `ChatSidebar`/`ChatPanel` pair (same
 * house pattern as `ChatSidebar.homeAgentName.test.ts`) through the real
 * `.message-input` textarea + Enter key — typing exactly the phrase the
 * old `navMap` used to intercept — and proves it now reaches
 * `api.sendChatMessage` (the real agent-send path) instead of being
 * answered locally with a fake "Done — navigated..." reply and zero
 * backend call.
 *
 * RED-ON-REVERT: restoring the deleted `navMap`/`fuzzyMatch` block makes
 * `api.sendChatMessage` NOT get called for "go to home" (intercepted
 * before reaching the `try { ... }` real-send block), and makes a fake
 * "Done — navigated to Home." assistant message appear in the DOM that
 * `api.sendChatMessage` never produced.
 */

import { describe, it, expect, vi, beforeAll, afterEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/svelte/svelte5';
import ChatSidebar from './ChatSidebar.svelte';
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

// Same unlisten-safety mock ChatSidebar.homeAgentName.test.ts already uses.
vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn().mockResolvedValue(() => {}),
}));

vi.mock('$lib/utils/markdown', () => ({
  renderMarkdown: (html: string) => html,
}));

function activeAgent(overrides: Partial<any> = {}) {
  return {
    id: 'agent-freetext-1', name: 'Nova', slug: 'nova', role: 'main', description: null,
    model: 'claude-sonnet-4-5', is_default: false, is_active: true,
    workspace_path: 'WPagent-freetext-1', soul_hash: null, memory_hash: null,
    config_json: '{}', owner_id: null, runtime_ref: 'oc-main', runtime_type: 'openclaw',
    source: 'openclaw', created_at: '2026-01-01T00:00:00Z', updated_at: '2026-01-01T00:00:00Z',
    ...overrides,
  };
}

function mkHomeSession(overrides: Partial<any> = {}) {
  return {
    id: 'sess-freetext-1', project_id: 'workspace', title: 'Home session', origin: 'user',
    created_at: '2026-08-01T10:00:00Z', last_active_at: '2026-08-22T08:00:00Z', hidden_at: null,
    is_active: true, agent_id: 'agent-freetext-1',
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
    sendChatMessage: vi.fn().mockResolvedValue('saved-msg-id-1'),
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

import { sessionStore } from '$lib/stores/sessionStore';

afterEach(() => {
  (sessionStore as any).__set({ sessions: [], activeSession: null, loading: false, error: null });
  (sessionStore.getActiveSession as any).mockResolvedValue(mkHomeSession());
  appStoreMod.activeProject.set(null);
  appStoreMod.activeChatThreadId.set(null);
  (apiMod.getAgents as any).mockResolvedValue([activeAgent()]);
  (apiMod.getChatMessages as any).mockResolvedValue([]);
  (apiMod.sendChatMessage as any).mockClear();
});

describe('#3044 item 4 — chat free-text phrases reach the agent, not a local navMap', () => {
  it('typing "go to home" and pressing Enter calls api.sendChatMessage (the real agent-send path) — no local interception, no fake "Done — navigated" reply', async () => {
    (sessionStore as any).__set({ sessions: [], activeSession: null, loading: false, error: null });
    (sessionStore.getActiveSession as any).mockResolvedValue(mkHomeSession());

    const { container, component } = render(ChatSidebar, { layoutMode: 'centered', alwaysOpen: true });
    appStoreMod.activeChatThreadId.set('sess-freetext-1');
    await (component as any).handleSessionSwitch(mkHomeSession());

    const textarea = await screen.findByPlaceholderText(/./) as HTMLTextAreaElement;
    await fireEvent.input(textarea, { target: { value: 'go to home' } });
    await fireEvent.keyDown(textarea, { key: 'Enter' });

    await waitFor(() => {
      expect(apiMod.sendChatMessage).toHaveBeenCalled();
    });
    const call = (apiMod.sendChatMessage as any).mock.calls.at(-1);
    // (threadId, projectId, content, ...) — content is the raw phrase, untouched.
    expect(call[2]).toBe('go to home');

    // RED-ON-REVERT: the old navMap path never called sendChatMessage at
    // all for this exact phrase, and instead synthesized this exact fake
    // reply text locally.
    expect(container.textContent).not.toContain('Done — navigated to Home.');
  });

  it('typing "show tasks" (another old navMap entry) also reaches api.sendChatMessage, not a local fuzzyMatch intercept', async () => {
    (sessionStore as any).__set({ sessions: [], activeSession: null, loading: false, error: null });
    (sessionStore.getActiveSession as any).mockResolvedValue(mkHomeSession());

    const { component } = render(ChatSidebar, { layoutMode: 'centered', alwaysOpen: true });
    appStoreMod.activeChatThreadId.set('sess-freetext-1');
    await (component as any).handleSessionSwitch(mkHomeSession());

    const textarea = await screen.findByPlaceholderText(/./) as HTMLTextAreaElement;
    await fireEvent.input(textarea, { target: { value: 'show tasks' } });
    await fireEvent.keyDown(textarea, { key: 'Enter' });

    await waitFor(() => {
      expect(apiMod.sendChatMessage).toHaveBeenCalled();
    });
    const call = (apiMod.sendChatMessage as any).mock.calls.at(-1);
    expect(call[2]).toBe('show tasks');
  });

  it('source-level guard: handleSendMessage no longer contains fuzzyMatch, navMap, or the "Done — navigated" reply text as real code (a comment mentioning the deletion is fine)', async () => {
    const { readFileSync } = await import('node:fs');
    const { resolve } = await import('node:path');
    const src = readFileSync(resolve(__dirname, 'ChatSidebar.svelte'), 'utf-8');
    // Strip `//` line comments so a comment describing the deletion (as
    // this commit's own deletion-site comment does) can't false-positive
    // this check — only real, live code is asserted on.
    const codeOnly = src
      .split('\n')
      .map((line) => line.replace(/\/\/.*$/, ''))
      .join('\n');
    expect(codeOnly).not.toMatch(/function fuzzyMatch/);
    expect(codeOnly).not.toMatch(/const navMap/);
    expect(codeOnly).not.toMatch(/Done — navigated/);
    expect(codeOnly).not.toMatch(/go to \[project name\]/i);
  });
});
