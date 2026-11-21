/**
 * ChatPanel.attachPicker.test.ts — §36.9.2 (#3050 PR 9), the picker upload
 * path in the chat composer.
 *
 * Reed's scope audit (2026-10-29-scope-3050-pr9-attachments.md §5): of
 * notes' three upload paths (drag-drop, picker, paste), chat already had
 * drag-drop and paste — the picker was "the one real gap." Mirrors
 * `Editor.svelte::handleToolbarAttachFile` almost verbatim: `@tauri-apps/
 * plugin-dialog`'s `open({multiple:true})`, then `api.copyAttachmentFromPath`
 * per selected path.
 *
 * Same `vi.mock`/render shape `ChatPanel.chipListener.test.ts` already
 * established for this component.
 */

import { describe, it, expect, vi, beforeAll, beforeEach } from 'vitest';
import { render, fireEvent, waitFor } from '@testing-library/svelte/svelte5';
import ChatPanel from './ChatPanel.svelte';

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

vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn().mockResolvedValue(() => {}),
}));

const { dialogOpenMock } = vi.hoisted(() => ({
  dialogOpenMock: vi.fn(),
}));
vi.mock('@tauri-apps/plugin-dialog', () => ({
  open: dialogOpenMock,
}));

const { copyAttachmentFromPathMock } = vi.hoisted(() => ({
  copyAttachmentFromPathMock: vi.fn(),
}));
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
  listRuntimeConnections: vi.fn().mockResolvedValue([
    { runtime_type: 'openclaw', connected: true, masked_credential: null, endpoint: null, error: null, agents: [] },
    { runtime_type: 'openrouter', connected: true, masked_credential: null, endpoint: null, error: null, agents: [] },
    { runtime_type: 'anthropic', connected: true, masked_credential: null, endpoint: null, error: null, agents: [] },
    { runtime_type: 'ollama', connected: true, masked_credential: null, endpoint: null, error: null, agents: [] },
  ]),
  copyAttachmentFromPath: copyAttachmentFromPathMock,
  deleteAttachment: vi.fn().mockResolvedValue(undefined),
}));

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

function baseProps(overrides: Partial<any> = {}) {
  return {
    messages: [] as any[],
    onSendMessage: vi.fn(),
    showHeader: false,
    projectId: 'project-test',
    activeThreadId: 'thread-open-1',
    ...overrides,
  };
}

beforeEach(() => {
  dialogOpenMock.mockReset();
  copyAttachmentFromPathMock.mockReset();
});

describe('ChatPanel — file-picker upload path (§36.9.2, #3050 PR 9)', () => {
  /// Core acceptance: clicking the picker button calls plugin-dialog's
  /// open() then api.copyAttachmentFromPath for the selected path — the
  /// exact two-call shape Editor.svelte's own picker already uses.
  /// Mutation: swap handleAttachFilePicker's body for a no-op → neither
  /// mock is ever called → red.
  it('clicking the attach button opens the native file dialog then copies the selected file', async () => {
    dialogOpenMock.mockResolvedValue('/Users/test/Documents/report.pdf');
    copyAttachmentFromPathMock.mockResolvedValue({
      id: 'att-1',
      filename: 'report.pdf',
      mime_type: 'application/pdf',
      size: 1234,
      path: '/pane/Projects/project-test/Assets/att-1.pdf',
    });

    const { getByTestId } = render(ChatPanel, baseProps());
    const button = getByTestId('attach-file-picker-btn');
    await fireEvent.click(button);

    await waitFor(() => {
      expect(dialogOpenMock).toHaveBeenCalledWith({ multiple: true });
    });
    await waitFor(() => {
      expect(copyAttachmentFromPathMock).toHaveBeenCalledWith(
        'report.pdf',
        '/Users/test/Documents/report.pdf',
        'project-test',
      );
    });
  });

  /// Multi-select: one copyAttachmentFromPath call per selected path.
  it('calls copyAttachmentFromPath once per file when multiple files are selected', async () => {
    dialogOpenMock.mockResolvedValue(['/a/one.md', '/a/two.csv']);
    copyAttachmentFromPathMock
      .mockResolvedValueOnce({ id: 'att-x1', filename: 'one.md', mime_type: 'text/markdown', size: 1, path: '/pane/one.md' })
      .mockResolvedValueOnce({ id: 'att-x2', filename: 'two.csv', mime_type: 'text/csv', size: 2, path: '/pane/two.csv' });

    const { getByTestId } = render(ChatPanel, baseProps());
    await fireEvent.click(getByTestId('attach-file-picker-btn'));

    await waitFor(() => {
      expect(copyAttachmentFromPathMock).toHaveBeenCalledTimes(2);
    });
  });

  /// Cancel (`open()` resolves `null`) is a no-op — regression pin, not a
  /// mutation target (the early `if (!sel) return;` is already exercised
  /// by its inverse in the test above).
  it('does nothing when the dialog is cancelled (open() resolves null)', async () => {
    dialogOpenMock.mockResolvedValue(null);

    const { getByTestId } = render(ChatPanel, baseProps());
    await fireEvent.click(getByTestId('attach-file-picker-btn'));

    await waitFor(() => {
      expect(dialogOpenMock).toHaveBeenCalled();
    });
    expect(copyAttachmentFromPathMock).not.toHaveBeenCalled();
  });
});
