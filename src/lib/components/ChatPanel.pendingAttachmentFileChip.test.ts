/**
 * ChatPanel.pendingAttachmentFileChip.test.ts — leftover 2 (#3139, live test #4).
 *
 * Direct precedent: C30 (#3128) closed the identical "bare chip" complaint
 * for the POST-SEND in-message attachment rendering (`.attachment-file`,
 * `ChatPanel.attachmentFileIcon.test.ts`) by adding filename + file size
 * alongside the uppercased-extension icon — and its own comment explicitly
 * left the composer's PRE-SEND `.preview-file-icon` box untouched. This
 * test is that box's own equivalent coverage.
 *
 * Same `vi.mock`/render shape `ChatPanel.attachPicker.test.ts` already
 * established for driving `pendingAttachments` through the real picker
 * path (`handleAttachFilePicker`) rather than poking internal state.
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

describe('ChatPanel — pending (pre-send) attachment chip shows filename + size for a non-image file (leftover 2, #3139)', () => {
  /// Mutation (actually run — see report): remove the new
  /// `.preview-file-name`/`.preview-file-size` spans from the composer's
  /// pre-send `{:else}` branch — reddens (neither the filename nor the
  /// size text is ever rendered; only the bare uppercased-extension box
  /// remains).
  it('renders filename and formatted size alongside the icon for a non-image pending attachment', async () => {
    dialogOpenMock.mockResolvedValue('/Users/test/Documents/report.pdf');
    copyAttachmentFromPathMock.mockResolvedValue({
      id: 'att-pending-1',
      filename: 'report.pdf',
      mime_type: 'application/pdf',
      size: 2048,
      path: '/pane/Projects/project-test/Assets/att-pending-1.pdf',
    });

    const { getByTestId, container } = render(ChatPanel, baseProps());
    const button = getByTestId('attach-file-picker-btn');
    await fireEvent.click(button);

    await waitFor(() => {
      const chip = container.querySelector('.attachment-preview.is-file');
      expect(chip).toBeTruthy();
      expect(chip?.querySelector('.preview-file-icon')?.textContent).toBe('PDF');
      expect(chip?.querySelector('.preview-file-name')?.textContent).toBe('report.pdf');
      expect(chip?.querySelector('.preview-file-size')?.textContent).toMatch(/KB|B/);
    });
  });
});
