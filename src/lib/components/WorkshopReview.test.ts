/**
 * WorkshopReview.test.ts — regression test for #2618
 * ("Session workshop does not exist" in the new-agent Memory/Soul chat).
 *
 * Root cause: WorkshopReview called ai_chat_stream with
 * context.projectId = WORKSHOP_SCOPE_ID ("__workshop__") and no sessionId.
 * Post-ADR-0062, ai_chat_stream's fallback path treats a missing sessionId's
 * resolved project_id as a literal session id — which never existed in
 * chat_sessions, so agents.rs::resolve_and_selfheal_session_agent threw
 * "Session __workshop__ does not exist".
 *
 * Fix: WorkshopReview now takes a required `sessionId` prop (created by the
 * caller via createWorkshopSession(), see workshopSession.test.ts) and
 * threads it into ai_chat_stream's top-level `sessionId` arg, bypassing the
 * project_id fallback path entirely.
 *
 * CI-testable scope:
 *   ✓ ai_chat_stream is called with sessionId === the sessionId prop
 *   ✓ ai_chat_stream is never called with sessionId === WORKSHOP_SCOPE_ID
 *     ("__workshop__") — the literal string that caused the original crash
 *
 * NOT tested here (live DMG tests — Matt):
 *   - Full onboarding/create-agent flows mounting this via Onboarding.svelte /
 *     AgentCreateOverlay.svelte (those components are too large to mount in
 *     this suite — see Onboarding.back-navigation.test.ts's it.skip note)
 *   - Streaming chunk rendering / edit_note live-reload behavior
 */
import { describe, it, expect, vi, beforeAll, afterEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/svelte/svelte5';
import WorkshopReview from './WorkshopReview.svelte';

const { invokeMock } = vi.hoisted(() => ({ invokeMock: vi.fn() }));

vi.mock('@tauri-apps/api/core', () => ({
  invoke: invokeMock,
}));

vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn().mockResolvedValue(() => {}),
}));

vi.mock('$lib/utils/markdown', () => ({
  renderMarkdown: (html: string) => html,
}));

beforeAll(() => {
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
  globalThis.ResizeObserver = class ResizeObserver {
    observe() {}
    unobserve() {}
    disconnect() {}
  } as any;
});

afterEach(() => {
  invokeMock.mockReset();
});

function baseProps(overrides: Partial<Record<string, unknown>> = {}) {
  return {
    soulContent: '',
    memoryContent: '',
    memoryNoteId: 'note-memory-1',
    soulNoteId: 'note-soul-1',
    scopeProjectId: '__workshop__',
    sessionId: 'sess-real-123',
    initialAssistantMessage: 'Here is your Memory.',
    onFinish: vi.fn(),
    ...overrides,
  };
}

describe('WorkshopReview — sessionId threading into ai_chat_stream (#2618)', () => {
  it('passes the sessionId prop through to ai_chat_stream, not the scopeProjectId sentinel', async () => {
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === 'ai_chat_stream') return 'stream-1';
      return null;
    });

    render(WorkshopReview, { props: baseProps({ sessionId: 'sess-real-123' }) });

    const textarea = await screen.findByPlaceholderText('Ask to edit or refine...');
    await fireEvent.input(textarea, { target: { value: 'Please tweak this.' } });
    await fireEvent.keyDown(textarea, { key: 'Enter' });

    await waitFor(() => {
      const call = invokeMock.mock.calls.find(([cmd]) => cmd === 'ai_chat_stream');
      expect(call).toBeTruthy();
    });

    const [, args] = invokeMock.mock.calls.find(([cmd]) => cmd === 'ai_chat_stream')!;
    expect(args.sessionId).toBe('sess-real-123');
    // The historical bug: sessionId was undefined and the backend fell back
    // to treating context.projectId ("__workshop__") as a literal session id.
    expect(args.sessionId).not.toBe('__workshop__');
    expect(args.sessionId).not.toBeUndefined();
    // context.projectId keeps carrying scopeProjectId (unrelated concept —
    // used for note scoping, not session resolution).
    expect(args.context.projectId).toBe('__workshop__');
  });

  it('threads a different sessionId value through unchanged (no hardcoding)', async () => {
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === 'ai_chat_stream') return 'stream-2';
      return null;
    });

    render(WorkshopReview, { props: baseProps({ sessionId: 'sess-another-456' }) });

    const textarea = await screen.findByPlaceholderText('Ask to edit or refine...');
    await fireEvent.input(textarea, { target: { value: 'Another message.' } });
    await fireEvent.keyDown(textarea, { key: 'Enter' });

    await waitFor(() => {
      const call = invokeMock.mock.calls.find(([cmd]) => cmd === 'ai_chat_stream');
      expect(call).toBeTruthy();
    });

    const [, args] = invokeMock.mock.calls.find(([cmd]) => cmd === 'ai_chat_stream')!;
    expect(args.sessionId).toBe('sess-another-456');
  });
});

describe('WorkshopReview — ChatPanel speaker label (#2604 PR #2681 delta audit regression)', () => {
  // Reed's delta audit of PR #2681 found that WorkshopReview's <ChatPanel>
  // mount site (onboarding + agent-create's shared Memory/Soul review step)
  // was never wired up with the new agentsLoadedProp introduced to close the
  // #2604 session-open race. Since WorkshopReview has no real agent roster
  // or session-binding concept to wait for (its messages are locally
  // synthesized workshop-chat turns, not backed by chat_messages_local.agent_id
  // or a bound agent), sessionAgentsLoaded defaulting to false there meant
  // the speaker label was PERMANENTLY stuck on the '.chat-sender--loading'
  // pulsing treatment instead of the correct 'Pane' literal (ADR-0059 D5
  // genuine nothing-to-attribute case) — for the entire lifetime of every
  // first-run onboarding and agent-creation chat session.
  //
  // Fix: WorkshopReview's <ChatPanel> mount now passes agentsLoadedProp={true}
  // explicitly, since 'Pane' is always the correct attribution on this surface.
  it("renders 'Pane' as the sender for the initial assistant message, not the loading indicator", async () => {
    invokeMock.mockImplementation(async () => null);

    const { container } = render(WorkshopReview, { props: baseProps() });

    const sender = await waitFor(() => {
      const el = container.querySelector('.chat-sender');
      if (!el) throw new Error('sender not rendered yet');
      return el;
    });

    expect(sender.textContent).toBe('Pane');
    expect(sender.classList.contains('chat-sender--loading')).toBe(false);
    expect(container.querySelector('.chat-sender--loading')).toBeNull();
  });
});
