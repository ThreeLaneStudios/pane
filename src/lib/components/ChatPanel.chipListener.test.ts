/**
 * ChatPanel.chipListener.test.ts — §2 (#3050 PR 6), PR #3114 fix.
 *
 * Reed's audit: "`pane-chip-changed` has ZERO frontend listeners…
 * `ChatPanel.svelte` never calls `listen('pane-chip-changed', ...)`…
 * the quit-and-relaunch chip bug is not fixed." This pins the listener
 * ChatPanel.svelte now registers:
 *
 *   - `status: "inserted"` for the OPEN session → `onChipInserted` fires
 *     (Matt's acceptance: "a chip appears in the open chat without
 *     relaunch" — the message reload itself is the caller's job, this
 *     proves ChatPanel hands off correctly).
 *   - `status: "inserted"` for a DIFFERENT session → `onChipInserted`
 *     must NOT fire (the filter: "an event for a job the open chat isn't
 *     showing must not cause a pointless re-query").
 *   - each status flip (`sent`, `done`, `failed`) for an EXISTING chip in
 *     the open session → `jobStatuses` updates in place (no message
 *     reload, no `onChipInserted` call) — proven via the chip's own
 *     `data-job-status` DOM attribute.
 *   - a status flip for a DIFFERENT session → no re-query at all (asserts
 *     `api.getJobStatus` is never called for that job id).
 *
 * Uses the same `listenersByEvent` capture-and-fire pattern
 * `UpdatingScreen.test.ts` already established for this exact
 * `@tauri-apps/api/event` mocking shape.
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

const { listenMock, listenersByEvent } = vi.hoisted(() => ({
  listenMock: vi.fn(),
  listenersByEvent: new Map<string, (event: { payload: unknown }) => void>(),
}));
vi.mock('@tauri-apps/api/event', () => ({
  listen: listenMock,
}));

const { getJobStatusMock } = vi.hoisted(() => ({
  getJobStatusMock: vi.fn(),
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
  getJobStatus: getJobStatusMock,
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

function fireChipChanged(payload: { job_id: string; session_id: string; status: string }) {
  const handler = listenersByEvent.get('pane-chip-changed');
  if (!handler) throw new Error('no listener registered for pane-chip-changed');
  handler({ payload });
}

function chipMessage(overrides: Partial<any> = {}) {
  return {
    id: 'msg-chip-1',
    thread_id: 'thread-open-1',
    role: 'assistant' as const,
    content: 'Spawned agent on "do the thing"',
    created_at: new Date().toISOString(),
    job_id: 'job-1',
    synthetic: 0,
    ...overrides,
  };
}

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
  listenMock.mockReset();
  getJobStatusMock.mockReset();
  listenersByEvent.clear();
  listenMock.mockImplementation(async (eventName: string, handler: (event: { payload: unknown }) => void) => {
    listenersByEvent.set(eventName, handler);
    return () => listenersByEvent.delete(eventName);
  });
});

describe('ChatPanel — pane-chip-changed listener (§2, #3050 PR 6 / #3114)', () => {
  /// Mutation (actually run — see report): removed the whole `$effect`
  /// registering the `listen('pane-chip-changed', ...)` call. Failure:
  /// `listenersByEvent` never gets an entry, `fireChipChanged` throws
  /// immediately ("no listener registered") — every test in this file
  /// reddens.
  it('registers a pane-chip-changed listener on mount', () => {
    render(ChatPanel, baseProps());
    expect(listenMock).toHaveBeenCalledWith('pane-chip-changed', expect.any(Function));
  });

  /// Core acceptance proof: an "inserted" event for the OPEN session hands
  /// off to onChipInserted — the caller's own re-query (loadMessages) is
  /// what actually makes "a chip appears in the open chat without
  /// relaunch" true in the app; this proves ChatPanel's own half of that
  /// hand-off fires correctly and with the right arguments.
  it('calls onChipInserted when an "inserted" event arrives for the open session', async () => {
    const onChipInserted = vi.fn();
    render(ChatPanel, baseProps({ onChipInserted }));

    fireChipChanged({ job_id: 'job-new-1', session_id: 'thread-open-1', status: 'inserted' });

    await waitFor(() => {
      expect(onChipInserted).toHaveBeenCalledWith('job-new-1', 'thread-open-1');
    });
  });

  /// The filter, proven directly: an event for a DIFFERENT session (a
  /// background job the open chat isn't showing) must not fire
  /// onChipInserted at all — "an event for a job the open chat isn't
  /// showing must not cause a pointless re-query."
  it('does NOT call onChipInserted when the event session does not match activeThreadId', async () => {
    const onChipInserted = vi.fn();
    render(ChatPanel, baseProps({ onChipInserted, activeThreadId: 'thread-open-1' }));

    fireChipChanged({ job_id: 'job-elsewhere-1', session_id: 'thread-OTHER-session', status: 'inserted' });

    // No await-able signal for "nothing happened" — flush a tick and assert.
    await new Promise(r => setTimeout(r, 0));
    expect(onChipInserted).not.toHaveBeenCalled();
  });

  /// Status-flip sub-case 1 of 3: "sent" — an EXISTING chip in the open
  /// session re-queries get_job_status and its own chip text updates,
  /// with NO message reload (onChipInserted never called for a flip).
  it('re-queries and renders a "sent" status flip for an existing chip in the open session', async () => {
    getJobStatusMock.mockResolvedValue({ status: 'sent', started_at: null, finished_at: null, last_error: null });
    const onChipInserted = vi.fn();
    const { container } = render(ChatPanel, baseProps({
      messages: [chipMessage({ job_id: 'job-1' })],
      onChipInserted,
    }));

    fireChipChanged({ job_id: 'job-1', session_id: 'thread-open-1', status: 'sent' });

    await waitFor(() => {
      const chip = container.querySelector('[data-job-id="job-1"]');
      expect(chip?.getAttribute('data-job-status')).toBe('sent');
    });
    expect(getJobStatusMock).toHaveBeenCalledWith('job-1');
    expect(onChipInserted).not.toHaveBeenCalled();
  });

  /// Status-flip sub-case 2 of 3: "done" — also exercises the
  /// started_at/finished_at duration text, which the event payload alone
  /// (job_id/session_id/status) can't supply — proving the listener does
  /// a REAL get_job_status re-read, not just echoing the event payload.
  it('re-queries and renders a "done" status flip with elapsed duration', async () => {
    getJobStatusMock.mockResolvedValue({
      status: 'done',
      started_at: '2026-10-29 10:00:00',
      finished_at: '2026-10-29 10:00:42',
      last_error: null,
    });
    const { container } = render(ChatPanel, baseProps({
      messages: [chipMessage({ job_id: 'job-1' })],
    }));

    fireChipChanged({ job_id: 'job-1', session_id: 'thread-open-1', status: 'done' });

    await waitFor(() => {
      const chip = container.querySelector('[data-job-id="job-1"]');
      expect(chip?.getAttribute('data-job-status')).toBe('done');
      expect(chip?.textContent).toContain('done in 42s');
    });
  });

  /// Status-flip sub-case 3 of 3: "failed".
  it('re-queries and renders a "failed" status flip for an existing chip in the open session', async () => {
    getJobStatusMock.mockResolvedValue({ status: 'failed', started_at: null, finished_at: null, last_error: 'boom' });
    const { container } = render(ChatPanel, baseProps({
      messages: [chipMessage({ job_id: 'job-1' })],
    }));

    fireChipChanged({ job_id: 'job-1', session_id: 'thread-open-1', status: 'failed' });

    await waitFor(() => {
      const chip = container.querySelector('[data-job-id="job-1"]');
      expect(chip?.getAttribute('data-job-status')).toBe('failed');
    });
  });

  /// The filter applies to status flips too, not just inserts: a flip for
  /// a job in a DIFFERENT session must not trigger any get_job_status
  /// call — that's still the 5s poll's job for ITS OWN open chat.
  it('does NOT call getJobStatus for a status flip belonging to a different session', async () => {
    // Resolve the pre-existing 5s poll's own fire-once-on-mount call (not
    // part of this fix, unchanged) to a TERMINAL status so job-1 drops out
    // of its own pollable set after exactly one merge — isolates this
    // assertion from any further ambient polling activity so the only
    // thing that could call `getJobStatus` again is the chip-changed
    // listener itself.
    getJobStatusMock.mockResolvedValue({ status: 'done', started_at: null, finished_at: null, last_error: null });
    const { container } = render(ChatPanel, baseProps({
      messages: [chipMessage({ job_id: 'job-1', thread_id: 'thread-open-1' })],
      activeThreadId: 'thread-open-1',
    }));

    await waitFor(() => {
      expect(container.querySelector('[data-job-id="job-1"]')?.getAttribute('data-job-status')).toBe('done');
    });
    getJobStatusMock.mockClear();

    fireChipChanged({ job_id: 'job-1', session_id: 'thread-OTHER-session', status: 'failed' });

    await new Promise(r => setTimeout(r, 0));
    expect(getJobStatusMock).not.toHaveBeenCalled();
  });
});
