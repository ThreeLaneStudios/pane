/**
 * ChatPanel.attachmentFileIcon.test.ts — C30 icon-parity half (#3128).
 *
 * Reed's brief: click-to-open is explicitly OUT of scope (no lightbox
 * mechanism exists anywhere in ChatPanel.svelte to reuse — building one is
 * a new feature, not a fix). Icon-parity only: a rendered `.attachment-file`
 * chip (the non-image/audio/video branch) must show a file-type icon
 * matching `pendingAttachments`' existing `.preview-file-icon` convention
 * (uppercased extension).
 */

import { describe, it, expect, vi, beforeAll, beforeEach } from 'vitest';
import { render, waitFor } from '@testing-library/svelte/svelte5';
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

const { getAttachmentMock, getAttachmentDataMock } = vi.hoisted(() => ({
  getAttachmentMock: vi.fn(),
  getAttachmentDataMock: vi.fn(),
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
  getJobStatus: vi.fn().mockResolvedValue({ status: 'done', started_at: null, finished_at: null, last_error: null }),
  getAttachment: getAttachmentMock,
  getAttachmentData: getAttachmentDataMock,
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
  vi.clearAllMocks();
  getAttachmentMock.mockReset();
  getAttachmentDataMock.mockReset();
});

describe('ChatPanel — attachment-file chip icon parity (C30, #3128)', () => {
  /// Mutation (actually run — see report): remove the new
  /// `.attachment-file-icon` span from the non-image/audio/video branch —
  /// reddens (no element with that class is ever rendered).
  it('renders an uppercased-extension icon for a non-media attachment, matching pendingAttachments\' own convention', async () => {
    getAttachmentMock.mockResolvedValue({
      id: 'att-1',
      original_filename: 'report.pdf',
      mime_type: 'application/pdf',
      size_bytes: 2048,
    });

    const { container } = render(ChatPanel, baseProps({
      messages: [{
        id: 'msg-att-1',
        thread_id: 'thread-open-1',
        role: 'user',
        content: 'here is the file',
        created_at: new Date().toISOString(),
        attachment_ids: 'att-1',
      }],
    }));

    await waitFor(() => {
      const chip = container.querySelector('.attachment-file');
      expect(chip).toBeTruthy();
      const icon = chip?.querySelector('.attachment-file-icon');
      expect(icon).toBeTruthy();
      expect(icon?.textContent).toBe('PDF');
    });
  });
});
