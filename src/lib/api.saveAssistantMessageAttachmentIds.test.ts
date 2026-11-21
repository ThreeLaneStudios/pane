/**
 * api.saveAssistantMessageAttachmentIds.test.ts — #3161 PR 1 (§39.5).
 *
 * `saveAssistantMessage` hardcoded `attachmentIds: null` in its
 * `invoke('save_chat_message', ...)` call — the backend's own
 * `attachment_ids` parameter already existed and already persists
 * whatever is passed (zero DB/command-signature change needed); nobody
 * on the assistant side ever passed a non-null value in. This PR adds an
 * optional `attachmentIds` parameter and forwards it.
 *
 * Mutation (actually run — see report): revert to the hardcoded `null` —
 * reddens (the first test's assertion on `request.attachmentIds` fails).
 */

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { saveAssistantMessage } from '$lib/api';
import * as tauriCore from '@tauri-apps/api/core';

describe('saveAssistantMessage — attachmentIds forwarding (§39.5, #3161 PR 1)', () => {
  let invokeSpy: ReturnType<typeof vi.spyOn>;

  beforeEach(() => {
    invokeSpy = vi.spyOn(tauriCore, 'invoke').mockResolvedValue(undefined);
  });

  afterEach(() => {
    invokeSpy.mockRestore();
  });

  it('forwards a non-null attachmentIds argument through to save_chat_message', async () => {
    await saveAssistantMessage(
      'thread-1',
      'project-1',
      'Generated image (attachment: att-1)',
      'msg-1',
      null,
      null,
      null,
      'att-1',
    );

    expect(invokeSpy).toHaveBeenCalledTimes(1);
    const [command, request] = invokeSpy.mock.calls[0] as [string, any];
    expect(command).toBe('save_chat_message');
    expect(request.attachmentIds).toBe('att-1');
    expect(request.role).toBe('assistant');
  });

  it('comma-joined multi-id strings pass through unchanged (the caller does the join)', async () => {
    await saveAssistantMessage('thread-1', 'project-1', 'two images', 'msg-2', null, null, null, 'att-1,att-2');
    const [, request] = invokeSpy.mock.calls[0] as [string, any];
    expect(request.attachmentIds).toBe('att-1,att-2');
  });

  it('defaults to null when attachmentIds is omitted — the pre-existing (no-image) call shape is unaffected', async () => {
    await saveAssistantMessage('thread-1', 'project-1', 'plain text reply', 'msg-3', null, null, 'claude-x');
    const [, request] = invokeSpy.mock.calls[0] as [string, any];
    expect(request.attachmentIds).toBeNull();
    expect(request.model).toBe('claude-x');
  });

  it('an empty string attachmentIds is normalized to null, not persisted as an empty string', async () => {
    await saveAssistantMessage('thread-1', 'project-1', 'no attachments after all', 'msg-4', null, null, null, '');
    const [, request] = invokeSpy.mock.calls[0] as [string, any];
    expect(request.attachmentIds).toBeNull();
  });
});
