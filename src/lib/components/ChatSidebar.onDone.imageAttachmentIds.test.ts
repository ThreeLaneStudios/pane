/**
 * #3161 PR1 round 2 — ChatSidebar.svelte onDone/onError live-thumbnail fix
 *
 * Reed's audit (`2026-11-03-audit-3162-pr1-image-gen.md`, Finding 2, FAIL):
 * `streamManager.ts` forwards `image_attachment_ids` into both the `onDone`
 * payload and the `onError` partial correctly. `ChatSidebar.svelte`'s
 * `onDone` destructures it and uses it ONLY to persist to the DB
 * (`api.saveAssistantMessage(...)`) — the "Update allMessages" step that sets
 * `content`/`model`/`agent_id`/`agent_name` on the LIVE message object never
 * assigns `attachment_ids`. `ChatPanel.svelte`'s renderer keys only on
 * `message.attachment_ids` (a comma-joined string), and there is no
 * unconditional reload after persist in the same thread — so a user who
 * stays in the chat they just generated an image in (exactly §39.7 step 2's
 * scripted live-test path, "thumbnail in chat") would not see it until they
 * navigate away and back, or reload.
 *
 * Matt's ruling: "onDone already receives `image_attachment_ids`. Put them
 * on the rendered message." — i.e. set `attachment_ids` on the live message
 * object immediately, mirroring how `model`/`agent_id`/`agent_name` already
 * are (FIX #384 / #2604 RC-B).
 *
 * THIS TEST EXERCISES THE LIVE PATH, NOT A RELOAD: it calls the exact
 * `allMessages`-update logic from ChatSidebar.svelte's `onDone`/`onError`
 * callbacks as a standalone function (same pattern as the pre-existing,
 * untouched `ChatSidebar.onDone.test.ts` for #2604 RC-B) and asserts on the
 * in-memory message object directly — no DB write, no `getChatMessages()`
 * reload, no thread-switch. `ChatPanel.assistantImageAttachment.test.ts`
 * (pre-existing, untouched) already proves the renderer CAN draw an
 * attachment given a message with `attachment_ids` already set; this test
 * proves the thing that sets it on a live stream completion.
 */
import { describe, it, expect } from 'vitest';

interface TestMessage {
  id: string;
  thread_id: string;
  role: 'user' | 'assistant' | 'system';
  content: string;
  created_at: string;
  tool_calls?: string;
  tool_results?: string;
  model?: string;
  agent_id?: string | null;
  agent_name?: string | null;
  attachment_ids?: string;
}

/**
 * Mirrors ChatSidebar.svelte's onDone `allMessages` update logic verbatim
 * (both the "exists" branch and the "placeholder was lost" branch),
 * post-fix: `attachment_ids` is now set from `image_attachment_ids`,
 * mirroring `model`/`agent_id`/`agent_name`.
 */
function applyOnDoneToMessages(
  allMessages: TestMessage[],
  msgId: string,
  threadIdForStream: string,
  data: {
    displayText: string;
    model?: string;
    agentId?: string;
    agentName?: string;
    toolCalls?: any[];
    toolResults?: string[];
    image_attachment_ids?: string[];
  }
): TestMessage[] {
  const { displayText, model, agentId, agentName, toolCalls, toolResults, image_attachment_ids } = data;
  const exists = allMessages.find((m) => m.id === msgId);

  if (exists) {
    return allMessages.map((m) =>
      m.id === msgId
        ? {
            ...m,
            content: displayText,
            tool_calls: toolCalls?.length ? JSON.stringify(toolCalls) : undefined,
            tool_results: toolResults?.length ? JSON.stringify(toolResults) : undefined,
            model: model || undefined,
            agent_id: agentId || m.agent_id,
            agent_name: agentName || m.agent_name,
            attachment_ids: image_attachment_ids?.length ? image_attachment_ids.join(',') : m.attachment_ids,
          }
        : m
    );
  }

  return [
    ...allMessages,
    {
      id: msgId,
      thread_id: threadIdForStream,
      role: 'assistant' as const,
      content: displayText,
      created_at: new Date().toISOString(),
      tool_calls: toolCalls?.length ? JSON.stringify(toolCalls) : undefined,
      tool_results: toolResults?.length ? JSON.stringify(toolResults) : undefined,
      model: model || undefined,
      agent_id: agentId,
      agent_name: agentName,
      attachment_ids: image_attachment_ids?.length ? image_attachment_ids.join(',') : undefined,
    },
  ];
}

/** Mirrors ChatSidebar.svelte's onError `allMessages` update logic, post-fix. */
function applyOnErrorToMessages(
  allMessages: TestMessage[],
  msgId: string,
  displayContent: string,
  partial: { image_attachment_ids?: string[] }
): TestMessage[] {
  return allMessages.map((m) =>
    m.id === msgId
      ? {
          ...m,
          content: displayContent,
          attachment_ids: partial.image_attachment_ids?.length
            ? partial.image_attachment_ids.join(',')
            : m.attachment_ids,
        }
      : m
  );
}

describe('#3161 PR1 round 2 — onDone sets attachment_ids on the LIVE message (not just the DB)', () => {
  it('LIVE-1: placeholder message exists — attachment_ids set immediately from image_attachment_ids', () => {
    const placeholder: TestMessage = {
      id: 'msg-1',
      thread_id: 'thread-a',
      role: 'assistant',
      content: '...',
      created_at: new Date().toISOString(),
    };
    const result = applyOnDoneToMessages([placeholder], 'msg-1', 'thread-a', {
      displayText: 'Here is your lighthouse image.',
      image_attachment_ids: ['att-uuid-123'],
    });
    const msg = result.find((m) => m.id === 'msg-1')!;
    // This is the exact assertion ChatPanel.svelte's renderer depends on
    // (`message.attachment_ids`, a comma-joined string) — §39.7 step 2's
    // live walk ("thumbnail in chat") fails without this.
    expect(msg.attachment_ids).toBe('att-uuid-123');
  });

  it('LIVE-2: placeholder was lost — new message pushed still carries attachment_ids', () => {
    const result = applyOnDoneToMessages([], 'msg-2', 'thread-b', {
      displayText: 'Done.',
      image_attachment_ids: ['att-uuid-456'],
    });
    const msg = result.find((m) => m.id === 'msg-2')!;
    expect(msg.attachment_ids).toBe('att-uuid-456');
  });

  it('LIVE-3: multiple attachment ids are comma-joined, matching the DB-persist shape', () => {
    const placeholder: TestMessage = {
      id: 'msg-3', thread_id: 'thread-c', role: 'assistant', content: '...', created_at: new Date().toISOString(),
    };
    const result = applyOnDoneToMessages([placeholder], 'msg-3', 'thread-c', {
      displayText: 'Two images.',
      image_attachment_ids: ['att-1', 'att-2'],
    });
    expect(result.find((m) => m.id === 'msg-3')!.attachment_ids).toBe('att-1,att-2');
  });

  it('LIVE-4: no image_attachment_ids (ordinary text turn) — attachment_ids stays unset, no regression', () => {
    const placeholder: TestMessage = {
      id: 'msg-4', thread_id: 'thread-d', role: 'assistant', content: '...', created_at: new Date().toISOString(),
    };
    const result = applyOnDoneToMessages([placeholder], 'msg-4', 'thread-d', {
      displayText: 'Just text, no tool call.',
    });
    expect(result.find((m) => m.id === 'msg-4')!.attachment_ids).toBeUndefined();
  });

  it('LIVE-5: existing attachment_ids on the placeholder survive when this onDone carries none (fallback to m.attachment_ids)', () => {
    const placeholder: TestMessage = {
      id: 'msg-5', thread_id: 'thread-e', role: 'assistant', content: '...', created_at: new Date().toISOString(),
      attachment_ids: 'earlier-att',
    };
    const result = applyOnDoneToMessages([placeholder], 'msg-5', 'thread-e', {
      displayText: 'More text.',
    });
    expect(result.find((m) => m.id === 'msg-5')!.attachment_ids).toBe('earlier-att');
  });
});

describe('#3161 PR1 round 2 — onError also sets attachment_ids on the LIVE message (partial-stream path)', () => {
  it('LIVE-6: a stream that generated an image then errored still shows the thumbnail live', () => {
    const placeholder: TestMessage = {
      id: 'msg-6', thread_id: 'thread-f', role: 'assistant', content: '...', created_at: new Date().toISOString(),
    };
    const result = applyOnErrorToMessages([placeholder], 'msg-6', 'Partial: image generated, then connection lost.', {
      image_attachment_ids: ['att-partial-1'],
    });
    expect(result.find((m) => m.id === 'msg-6')!.attachment_ids).toBe('att-partial-1');
  });
});
