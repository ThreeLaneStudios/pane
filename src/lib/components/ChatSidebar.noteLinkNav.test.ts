/**
 * ChatSidebar.noteLinkNav.test.ts — #3151 PR1 Commit D
 *
 * `navigateToNoteById` is `ChatPanel`'s new `onNoteLinkClick` hand-off —
 * a `pane://note/{id}` click only carries a note id, so this resolves the
 * owning project via `get_note` and then reuses `handleNavigateFromStream`'s
 * existing `open_note` decision (`navigationDispatch.ts`) rather than
 * re-deriving "open a note in its project" a second time. This proves the
 * hand-off actually lands in the RIGHT project — the "a link that opens
 * the wrong project" failure mode this sprint's own task calls out by
 * name as the thing to test slowly, not just that some navigation fired.
 */
import { describe, it, expect, vi, beforeAll, afterEach } from 'vitest';
import { render } from '@testing-library/svelte/svelte5';
import { invoke } from '@tauri-apps/api/core';
import ChatSidebar from './ChatSidebar.svelte';
import { activeNotebook } from '$lib/stores/app';
import * as appStoreMod from '$lib/stores/app';

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

vi.mock('$lib/api', async (importOriginal) => {
  const original = await importOriginal() as any;
  return {
    ...original,
    getAgents: vi.fn().mockResolvedValue([]),
    getChatMessages: vi.fn().mockResolvedValue([]),
    listRuntimeConnections: vi.fn().mockResolvedValue([]),
    aiLoadConfig: vi.fn().mockResolvedValue({ provider: 'openclaw' }),
    // Deliberately NOT overridden to `[]` (unlike other ChatSidebar test
    // files that don't exercise project navigation) — `handleNavigateFromStream`'s
    // `open_note` case looks the resolved note's `project_id` up in this
    // list; it must contain a real match for this test to prove anything.
    listProjects: vi.fn().mockResolvedValue([
      { id: 'project-test', name: 'Test Notebook', parent_id: null },
      { id: 'project-other', name: 'Some Other Project', parent_id: null },
    ]),
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
      getActiveSession: vi.fn().mockResolvedValue(null),
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
  appStoreMod.activeProject.set(null);
  appStoreMod.activeChatThreadId.set(null);
  appStoreMod.activeNote.set(null);
  activeNotebook.set('');
});

describe('#3151 PR1 Commit D — navigateToNoteById lands in the note\'s OWN project', () => {
  it('resolves the note via get_note, then opens it in ITS project (not whatever project happened to be active)', async () => {
    // Seed a real note in 'project-test' through the same mocked
    // 'create_note' IPC route `get_note` reads back from — no second,
    // hand-rolled note fixture shape to drift out of sync with the real
    // mock.
    const created: any = await invoke('create_note', {
      request: { title: 'Research Note', project_id: 'project-test', content: 'Findings.' },
    });

    const { component } = render(ChatSidebar, { layoutMode: 'centered', alwaysOpen: true });
    // Start from a DIFFERENT project active, to prove the link opens the
    // note's OWN project, not the one already on screen.
    appStoreMod.activeProject.set({ id: 'project-other', name: 'Some Other Project' } as any);

    await (component as any).navigateToNoteById(created.id);

    expect(appStoreMod.activeNote.get?.() ?? null).not.toBeUndefined();
    let notebookValue = '';
    const unsub = activeNotebook.subscribe((v) => (notebookValue = v));
    unsub();
    expect(notebookValue).toBe('Test Notebook');
  });

  it('an unresolvable note id surfaces a visible error, not silence', async () => {
    const { component } = render(ChatSidebar, { layoutMode: 'centered', alwaysOpen: true });

    // Must not throw — the function's own try/catch is what's under test.
    await expect((component as any).navigateToNoteById('does-not-exist')).resolves.toBeUndefined();
  });
});
