/**
 * #2604 RC-B — ChatSidebar.svelte onDone agent attribution tests
 *
 * ChatSidebar.svelte is a large, API/session-integrated component with no
 * existing component-render test harness (unlike ChatPanel.test.ts). Per the
 * same pattern this codebase already uses for streamManager.ts's done-event
 * handling (see streamManager.test.ts's `simulateDoneEventToOnDone`), these
 * tests exercise the EXACT `allMessages` update logic from ChatSidebar.svelte's
 * `onDone` callback (~line 653-702) as a standalone function — verifying that
 * `agent_id`/`agent_name` are set on the live message object exactly the way
 * `model` already is (FIX #384), instead of being left `undefined` until a
 * DB reload repopulates them via the `agent_name` LEFT JOIN.
 *
 * This pins Root Cause B's fix from
 * docs/audits/2026-08-20-c8-session-binding-investigation.md:
 *   "Set agent_id/agent_name on the live message object in ChatSidebar.svelte's
 *    onDone callback, mirroring the way model is already set."
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
}

/**
 * Mirrors ChatSidebar.svelte's onDone `allMessages` update logic verbatim
 * (both the "exists" branch and the "placeholder was lost" branch) — the
 * exact two code paths that set `model` and now also set `agent_id`/`agent_name`.
 */
function applyOnDoneToMessages(
  allMessages: TestMessage[],
  msgId: string,
  threadIdForStream: string,
  data: { displayText: string; model?: string; agentId?: string; agentName?: string; toolCalls?: any[]; toolResults?: string[] }
): TestMessage[] {
  const { displayText, model, agentId, agentName, toolCalls, toolResults } = data;
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
    },
  ];
}

describe('#2604 RC-B — ChatSidebar onDone sets agent_id/agent_name on the live message', () => {
  it('Matt repro 3: Biff-bound session, Blue as default — live message labels "Biff" immediately', () => {
    // The placeholder assistant message exists (created at send time), has no
    // agent_name yet — exactly the failure window Root Cause B describes.
    const placeholder: TestMessage = {
      id: 'msg-1',
      thread_id: 'thread-biff-session',
      role: 'assistant',
      content: '...',
      created_at: new Date().toISOString(),
    };

    const updated = applyOnDoneToMessages([placeholder], 'msg-1', 'thread-biff-session', {
      displayText: 'Here is your answer.',
      model: 'openrouter/some-model',
      // Backend resolved the TRUE dispatching agent (Biff, the session's own
      // bound agent) — NOT Blue, even though Blue is the current app-wide default.
      agentId: 'agent-biff',
      agentName: 'Biff',
    });

    const msg = updated.find((m) => m.id === 'msg-1')!;
    expect(msg.agent_name).toBe('Biff');
    expect(msg.agent_id).toBe('agent-biff');
    // Must never be the default agent's name.
    expect(msg.agent_name).not.toBe('Blue');
  });

  it('placeholder was lost (message not in allMessages) — new message still carries agent attribution', () => {
    const updated = applyOnDoneToMessages([], 'msg-2', 'thread-biff-session', {
      displayText: 'Recovered response.',
      model: 'openrouter/some-model',
      agentId: 'agent-biff',
      agentName: 'Biff',
    });

    expect(updated).toHaveLength(1);
    expect(updated[0].agent_id).toBe('agent-biff');
    expect(updated[0].agent_name).toBe('Biff');
  });

  it('model badge behavior is unchanged — still set immediately regardless of agent attribution (RC-A receipt)', () => {
    // #2604 explicitly must NOT change model-badge behavior (smart-routed
    // messages must keep showing which model actually answered).
    const placeholder: TestMessage = {
      id: 'msg-3',
      thread_id: 'thread-1',
      role: 'assistant',
      content: '...',
      created_at: new Date().toISOString(),
    };

    const updated = applyOnDoneToMessages([placeholder], 'msg-3', 'thread-1', {
      displayText: 'Smart-routed answer.',
      model: 'anthropic/claude-haiku-4-5', // the model that ACTUALLY ran
      agentId: 'agent-biff',
      agentName: 'Biff',
    });

    const msg = updated.find((m) => m.id === 'msg-3')!;
    expect(msg.model).toBe('anthropic/claude-haiku-4-5');
    expect(msg.agent_name).toBe('Biff');
  });

  it('missing agentId/agentName from a legacy/pre-fix backend payload does not clobber existing values', () => {
    // If a message already had agent_name set (e.g. re-entrant onDone call, or
    // partial update), an undefined incoming agentName must not blank it out.
    const existing: TestMessage = {
      id: 'msg-4',
      thread_id: 'thread-1',
      role: 'assistant',
      content: '...',
      created_at: new Date().toISOString(),
      agent_id: 'agent-biff',
      agent_name: 'Biff',
    };

    const updated = applyOnDoneToMessages([existing], 'msg-4', 'thread-1', {
      displayText: 'Final text.',
      // agentId/agentName omitted — simulates an old backend that hasn't
      // shipped the #2604 fix yet.
      agentId: undefined,
      agentName: undefined,
    });

    const msg = updated.find((m) => m.id === 'msg-4')!;
    expect(msg.agent_name).toBe('Biff');
    expect(msg.agent_id).toBe('agent-biff');
  });

  it('assistant message with no agent attribution at all stays undefined (never defaults to a fabricated name)', () => {
    const placeholder: TestMessage = {
      id: 'msg-5',
      thread_id: 'thread-1',
      role: 'assistant',
      content: '...',
      created_at: new Date().toISOString(),
    };

    const updated = applyOnDoneToMessages([placeholder], 'msg-5', 'thread-1', {
      displayText: 'Answer with no attribution available.',
    });

    const msg = updated.find((m) => m.id === 'msg-5')!;
    expect(msg.agent_name).toBeUndefined();
    expect(msg.agent_id).toBeUndefined();
  });
});
