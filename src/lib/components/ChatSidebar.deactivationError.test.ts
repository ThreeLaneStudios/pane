/**
 * ChatSidebar.deactivationError.test.ts — D22 (#2649 family)
 *
 * ChatSidebar.svelte is a large, API/session-integrated component with no
 * existing component-render test harness for its send-message error path
 * (unlike ChatSidebar.homeLockBanner.test.ts, which exercises a pure
 * mount/render DOM-presence bug). Per the same established pattern this file
 * family already uses (ChatSidebar.onDone.test.ts, ChatSidebar.greetingAgent
 * .test.ts), this test exercises the EXACT error-classification logic from
 * `handleSendMessage`'s catch block (ChatSidebar.svelte, ~line 1306-1330) as
 * a standalone function.
 *
 * Root cause pinned here: the backend's `check_bound_agent_active` guard
 * (agents.rs:366-403) refuses dispatch with a message naming the deactivated
 * agent, e.g. "Session is bound to agent 'X' which is currently inactive.
 * Start a new session or reconnect the agent's provider." Before this fix,
 * that message fell through the `isGateway` regex (which only matches
 * network-ish errors) into the generic branch and rendered as a raw
 * "⚠ Something went wrong: <error>" bubble — technically accurate but reads
 * as a crash, not an explainable lock state.
 */
import { describe, it, expect } from 'vitest';

/**
 * Mirrors ChatSidebar.svelte's `handleSendMessage` catch-block error
 * classification verbatim.
 */
function classifySendError(error: unknown): string {
  const errStr = String(error);
  // Checked BEFORE isGateway: the deactivation guard's own copy says
  // "reconnect the agent's provider", which would otherwise false-match the
  // isGateway regex's "connect" term.
  const isDeactivated = /currently inactive|has been deactivated|bound to .* which/i.test(errStr);
  const isGateway = !isDeactivated && /gateway|connect|ECONNREFUSED|network|timeout|fetch/i.test(errStr);
  const displayError = isDeactivated
    ? `\u26A0 ${errStr}`
    : isGateway
    ? '\u26A0 Couldn\'t reach the AI. Check your connection and try again.'
    : `\u26A0 Something went wrong: ${errStr}`;
  return displayError;
}

describe('D22 (#2649 family) — ChatSidebar handleSendMessage catch classifies deactivation errors', () => {
  it('check_bound_agent_active deactivation message renders the lock message, not the generic bubble', () => {
    const err = "Session is bound to agent 'Deactivated Agent' which is currently inactive. Start a new session or reconnect the agent's provider.";
    const displayError = classifySendError(err);

    expect(displayError).toBe(`\u26A0 ${err}`);
    expect(displayError).not.toContain('Something went wrong');
  });

  it('ai_chat_stream inline "has been deactivated" phrasing also classifies as deactivation, not generic', () => {
    const err = 'This session is bound to Deactivated Agent, which has been deactivated. Start a new session to continue.';
    const displayError = classifySendError(err);

    expect(displayError).toBe(`\u26A0 ${err}`);
    expect(displayError).not.toContain('Something went wrong');
  });

  it('gateway/network errors still classify as gateway, not deactivation (regression guard)', () => {
    const err = 'Error: gateway unreachable — ECONNREFUSED';
    const displayError = classifySendError(err);

    expect(displayError).toBe("\u26A0 Couldn't reach the AI. Check your connection and try again.");
  });

  it('unrelated errors still fall through to the generic "Something went wrong" bubble (regression guard)', () => {
    const err = 'Error: unexpected null pointer in tool_calls parser';
    const displayError = classifySendError(err);

    expect(displayError).toBe(`\u26A0 Something went wrong: ${err}`);
  });
});
