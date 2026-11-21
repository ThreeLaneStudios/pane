/**
 * api.chatTranscript.test.ts — regression test for #2308
 *
 * Onboarding.svelte's `saveAndComplete` transcript-save block was calling
 * `invoke('save_chat_message', ...)` / `invoke('load_chat_messages', ...)`
 * directly with the wrong argument shape — missing the `threadId` key
 * `save_chat_message` requires and the `sessionId` key `load_chat_messages`
 * requires (see exact Rust signatures in src-tauri/src/ai/commands.rs:
 * `save_chat_message(id, project_id, thread_id, role, content, ...)` and
 * `load_chat_messages(session_id, limit, after, offset)` — Tauri's invoke
 * bridge camelCases these to `threadId` / `sessionId` on the JS side).
 * Console showed:
 *   invalid args threadId for command save_chat_message: missing required key threadId
 *   invalid args sessionId for command load_chat_messages: missing required key sessionId
 *
 * Fix: Onboarding.svelte now calls the shared api.ts wrappers
 * (`sendChatMessage` / `saveAssistantMessage` / `getChatMessages` /
 * `getActiveSession`) instead of raw `invoke(...)`, matching every other
 * call site (ChatSidebar.svelte). This test asserts those wrappers pass the
 * exact param keys the Rust commands require, so it goes red if the
 * wrapper's invoke args ever drift from the Rust command signature again —
 * which is the same class of bug that broke Onboarding.svelte's direct
 * invoke calls.
 *
 * Rust unit tests can't catch this: the mismatch is a Tauri invoke-layer
 * argument-shape error (JS object keys vs. command param names), which
 * never reaches the Rust handler at all — `cargo test` calling the Rust
 * function directly with correct Rust types would never see this failure
 * mode. A vitest against the call-site wrapper (this file) is the right
 * layer per the task's test guidance.
 */

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';

const invokeMock = vi.fn(async (_cmd: string, _args?: Record<string, unknown>) => ({}));

vi.mock('@tauri-apps/api/core', () => ({
  invoke: (...args: [string, Record<string, unknown>?]) => invokeMock(...args),
  convertFileSrc: (path: string) => path,
}));

import { sendChatMessage, saveAssistantMessage, getChatMessages, getActiveSession } from '$lib/api';

describe('#2308 chat transcript persistence — invoke argument shape', () => {
  beforeEach(() => {
    invokeMock.mockReset();
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === 'get_active_session') {
        return { id: 'thread-abc-123', project_id: 'workspace', title: '', origin: 'user', created_at: '', last_active_at: '', hidden_at: null, is_active: true };
      }
      return {};
    });
  });

  afterEach(() => {
    vi.resetAllMocks();
  });

  it('sendChatMessage calls save_chat_message with a threadId key (not sessionId, not missing)', async () => {
    await sendChatMessage('thread-abc-123', 'workspace', 'hello there');

    expect(invokeMock).toHaveBeenCalledWith(
      'save_chat_message',
      expect.objectContaining({
        threadId: 'thread-abc-123',
        projectId: 'workspace',
        role: 'user',
        content: 'hello there',
      })
    );
    const args = invokeMock.mock.calls[0][1] as Record<string, unknown>;
    expect(args).toHaveProperty('threadId');
    expect(args).not.toHaveProperty('sessionId');
  });

  it('saveAssistantMessage calls save_chat_message with a threadId key', async () => {
    await saveAssistantMessage('thread-abc-123', 'workspace', 'hi back');

    expect(invokeMock).toHaveBeenCalledWith(
      'save_chat_message',
      expect.objectContaining({
        threadId: 'thread-abc-123',
        projectId: 'workspace',
        role: 'assistant',
        content: 'hi back',
      })
    );
  });

  it('getChatMessages calls load_chat_messages with a sessionId key (not threadId, not missing)', async () => {
    await getChatMessages('thread-abc-123', 50);

    expect(invokeMock).toHaveBeenCalledWith(
      'load_chat_messages',
      expect.objectContaining({
        sessionId: 'thread-abc-123',
        limit: 50,
      })
    );
    const args = invokeMock.mock.calls[0][1] as Record<string, unknown>;
    expect(args).toHaveProperty('sessionId');
    expect(args).not.toHaveProperty('threadId');
  });

  it('getActiveSession calls get_active_session with a projectId key and resolves a real thread id', async () => {
    const session = await getActiveSession('workspace');

    expect(invokeMock).toHaveBeenCalledWith('get_active_session', { projectId: 'workspace' });
    expect(session.id).toBe('thread-abc-123');
  });

  it('reproduces the onboarding saveAndComplete flow: resolve session, save both roles, verify via getChatMessages', async () => {
    const session = await getActiveSession('workspace');
    const threadId = session.id;

    await sendChatMessage(threadId, 'workspace', 'user message', 'msg-1');
    await saveAssistantMessage(threadId, 'workspace', 'assistant message', 'msg-2');
    await getChatMessages(threadId, 50);

    const saveCalls = invokeMock.mock.calls.filter(([cmd]) => cmd === 'save_chat_message');
    expect(saveCalls).toHaveLength(2);
    for (const [, args] of saveCalls) {
      // This is the exact bug: threadId missing/wrong would fail this assertion,
      // reproducing "invalid args threadId for command save_chat_message: missing required key threadId"
      expect(args).toMatchObject({ threadId: 'thread-abc-123' });
    }

    const loadCall = invokeMock.mock.calls.find(([cmd]) => cmd === 'load_chat_messages');
    expect(loadCall).toBeDefined();
    // Reproduces "invalid args sessionId for command load_chat_messages: missing required key sessionId"
    expect(loadCall![1]).toMatchObject({ sessionId: 'thread-abc-123' });
  });
});
