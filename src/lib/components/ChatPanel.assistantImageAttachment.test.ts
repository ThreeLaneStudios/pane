/**
 * ChatPanel.assistantImageAttachment.test.ts — #3161 PR 1 (§39.5).
 *
 * §39.5: "Reed confirms whether the existing image-attachment renderer on
 * user messages can draw an attachment referenced from a tool result; if
 * not, that is a small addition ... not assumed." Confirmed TRUE in code
 * (ChatPanel.svelte's `{#if message.attachment_ids}` block is gated only
 * on attachment_ids presence, no `message.role === 'user'` check) — this
 * test proves it by actually mounting an `assistant`-role message with
 * `attachment_ids` set and asserting the same `<img>` a user message
 * would get. Same render harness as `ChatPanel.attachmentFileIcon.test.ts`
 * (C30, #3128).
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

describe('ChatPanel — assistant-row image attachment (§39.5, #3161 PR 1)', () => {
  /// Mutation (would redden if run against a hypothetical `message.role
  /// === 'user'` guard added to the `{#if message.attachment_ids}` block):
  /// an assistant-authored row with attachment_ids must render the exact
  /// same `<img>` a user-authored row does — the renderer is genuinely
  /// role-agnostic, not "works for user messages, assumed to work for
  /// assistant ones."
  it('renders an <img> for an assistant-role message with attachment_ids, the same as a user message would', async () => {
    getAttachmentMock.mockResolvedValue({
      id: 'att-img-1',
      original_filename: 'generated-image.png',
      mime_type: 'image/png',
      size_bytes: 4096,
    });
    getAttachmentDataMock.mockResolvedValue({ data: 'aGVsbG8=' });

    const { container } = render(ChatPanel, baseProps({
      messages: [{
        id: 'msg-assistant-img-1',
        thread_id: 'thread-open-1',
        role: 'assistant',
        content: 'Generated image (attachment: att-img-1)',
        created_at: new Date().toISOString(),
        attachment_ids: 'att-img-1',
      }],
    }));

    await waitFor(() => {
      const img = container.querySelector('.attachment-image') as HTMLImageElement | null;
      expect(img, 'an <img class="attachment-image"> must render for the assistant row').toBeTruthy();
      expect(img?.getAttribute('src')).toBe('data:image/png;base64,aGVsbG8=');
    });

    expect(getAttachmentMock).toHaveBeenCalledWith('att-img-1');
    expect(getAttachmentDataMock).toHaveBeenCalledWith('att-img-1');
  });

  it('a message with no attachment_ids at all renders no .message-attachments block', () => {
    const { container } = render(ChatPanel, baseProps({
      messages: [{
        id: 'msg-assistant-plain',
        thread_id: 'thread-open-1',
        role: 'assistant',
        content: 'just text, no image',
        created_at: new Date().toISOString(),
      }],
    }));
    expect(container.querySelector('.message-attachments')).toBeNull();
  });
});
