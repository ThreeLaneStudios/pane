/**
 * #2604 RC-B family — ChatSidebar.svelte greeting agent-name resolution tests
 *
 * Both canned greeting templates in ChatSidebar.svelte (home greeting ~line 282,
 * project greeting ~line 975) previously called `get_main_agent` (the app-wide
 * DEFAULT agent) instead of resolving the session's BOUND agent
 * (`$sessionStore.activeSession?.agent_id`). Matt's repro: Biff is the default
 * agent, he creates a session bound to Blue — the canned welcome says "I'm
 * Biff." Because that greeting text sits in the conversation context, Blue
 * then believes it IS Biff until challenged. Same RC-B family as the onDone
 * agent-attribution fix pinned in ChatSidebar.onDone.test.ts.
 *
 * ChatSidebar.svelte is a large, API/session-integrated component with no
 * existing component-render test harness (see ChatSidebar.onDone.test.ts's
 * docstring). Per that same established pattern, this test exercises the
 * EXACT `resolveGreetingAgentName()` fallback-chain logic from
 * ChatSidebar.svelte as a standalone function, rather than mounting the full
 * component.
 *
 * Fallback chain under test: session's BOUND agent (activeSession.agent_id)
 * → default/main agent (get_main_agent) → 'Pane'.
 */
import { describe, it, expect, vi } from 'vitest';

interface TestAgent {
  id: string;
  name: string;
}

interface TestSessionState {
  activeSession: { agent_id: string | null } | null;
}

/**
 * Mirrors ChatSidebar.svelte's `resolveGreetingAgentName()` verbatim: resolves
 * the session's bound agent first, falls back to the default agent, and
 * finally falls back to the literal string 'Pane' — logging a diagnostic
 * `console.warn` at that final fallback naming which lookup path failed
 * (#2794).
 */
async function resolveGreetingAgentName(
  sessionState: TestSessionState,
  getAgents: () => Promise<TestAgent[]>,
  getMainAgent: () => Promise<{ name: string } | null>
): Promise<string> {
  const boundAgentId = sessionState.activeSession?.agent_id ?? null;
  if (boundAgentId) {
    try {
      const agents = await getAgents();
      const bound = agents.find((a) => a.id === boundAgentId);
      if (bound && bound.name && bound.name.trim()) {
        return bound.name.trim();
      }
    } catch {
      // fall through to default-agent resolution
    }
  }

  const agentResult = await getMainAgent().catch(() => null);
  if (agentResult && agentResult.name && agentResult.name.trim()) {
    return agentResult.name.trim();
  }
  console.warn('[GREETING] Falling back to "Pane" — bound agent lookup: %s, get_main_agent: %s',
    boundAgentId ? 'miss (agent_id bound but not found or empty name)' : 'skip (no agent_id on session)',
    agentResult ? 'miss (returned but name empty/whitespace)' : 'miss (returned null or threw)'
  );
  return 'Pane';
}

describe('#2604 RC-B family — ChatSidebar greeting resolves the BOUND agent, not the default', () => {
  it('Matt repro: Biff is default, session is bound to Blue — greeting says "Blue", not "Biff"', async () => {
    const sessionState: TestSessionState = { activeSession: { agent_id: 'agent-blue' } };
    const getAgents = vi.fn().mockResolvedValue([
      { id: 'agent-biff', name: 'Biff' },
      { id: 'agent-blue', name: 'Blue' },
    ]);
    const getMainAgent = vi.fn().mockResolvedValue({ name: 'Biff' });

    const name = await resolveGreetingAgentName(sessionState, getAgents, getMainAgent);

    expect(name).toBe('Blue');
    expect(name).not.toBe('Biff');
    // Bound-agent path must not need to consult the default agent at all.
    expect(getMainAgent).not.toHaveBeenCalled();
  });

  it('unbound session (agent_id null) falls back to the default agent', async () => {
    const sessionState: TestSessionState = { activeSession: { agent_id: null } };
    const getAgents = vi.fn().mockResolvedValue([{ id: 'agent-biff', name: 'Biff' }]);
    const getMainAgent = vi.fn().mockResolvedValue({ name: 'Biff' });

    const name = await resolveGreetingAgentName(sessionState, getAgents, getMainAgent);

    expect(name).toBe('Biff');
    expect(getAgents).not.toHaveBeenCalled();
  });

  it('no active session at all falls back to the default agent', async () => {
    const sessionState: TestSessionState = { activeSession: null };
    const getAgents = vi.fn().mockResolvedValue([{ id: 'agent-biff', name: 'Biff' }]);
    const getMainAgent = vi.fn().mockResolvedValue({ name: 'Biff' });

    const name = await resolveGreetingAgentName(sessionState, getAgents, getMainAgent);

    expect(name).toBe('Biff');
  });

  it('bound agent_id set but agent not found in list (stale/deleted) falls back to default agent', async () => {
    const sessionState: TestSessionState = { activeSession: { agent_id: 'agent-ghost' } };
    const getAgents = vi.fn().mockResolvedValue([{ id: 'agent-biff', name: 'Biff' }]);
    const getMainAgent = vi.fn().mockResolvedValue({ name: 'Biff' });

    const name = await resolveGreetingAgentName(sessionState, getAgents, getMainAgent);

    expect(name).toBe('Biff');
  });

  it('getAgents throws (e.g. IPC failure) — falls back to default agent, does not reject', async () => {
    const sessionState: TestSessionState = { activeSession: { agent_id: 'agent-blue' } };
    const getAgents = vi.fn().mockRejectedValue(new Error('IPC failure'));
    const getMainAgent = vi.fn().mockResolvedValue({ name: 'Biff' });

    const name = await resolveGreetingAgentName(sessionState, getAgents, getMainAgent);

    expect(name).toBe('Biff');
  });

  it('both bound and default resolution fail/empty — falls back to literal "Pane"', async () => {
    const sessionState: TestSessionState = { activeSession: { agent_id: null } };
    const getAgents = vi.fn().mockResolvedValue([]);
    const getMainAgent = vi.fn().mockResolvedValue(null);

    const name = await resolveGreetingAgentName(sessionState, getAgents, getMainAgent);

    expect(name).toBe('Pane');
  });

  it('bound agent name is whitespace-only — falls back to default agent (matches trim() guard)', async () => {
    const sessionState: TestSessionState = { activeSession: { agent_id: 'agent-blank' } };
    const getAgents = vi.fn().mockResolvedValue([{ id: 'agent-blank', name: '   ' }]);
    const getMainAgent = vi.fn().mockResolvedValue({ name: 'Biff' });

    const name = await resolveGreetingAgentName(sessionState, getAgents, getMainAgent);

    expect(name).toBe('Biff');
  });
});

describe('#2794 — diagnostic log at the final "Pane" fallback', () => {
  it('no agent_id on session + get_main_agent returns null — logs "skip" / "miss (returned null or threw)"', async () => {
    const warnSpy = vi.spyOn(console, 'warn').mockImplementation(() => {});
    const sessionState: TestSessionState = { activeSession: { agent_id: null } };
    const getAgents = vi.fn().mockResolvedValue([]);
    const getMainAgent = vi.fn().mockResolvedValue(null);

    const name = await resolveGreetingAgentName(sessionState, getAgents, getMainAgent);

    expect(name).toBe('Pane');
    expect(warnSpy).toHaveBeenCalledTimes(1);
    expect(warnSpy).toHaveBeenCalledWith(
      '[GREETING] Falling back to "Pane" — bound agent lookup: %s, get_main_agent: %s',
      'skip (no agent_id on session)',
      'miss (returned null or threw)'
    );
    warnSpy.mockRestore();
  });

  it('bound agent_id present but not found in list + get_main_agent throws — logs "miss (agent_id bound...)" / "miss (returned null or threw)"', async () => {
    const warnSpy = vi.spyOn(console, 'warn').mockImplementation(() => {});
    const sessionState: TestSessionState = { activeSession: { agent_id: 'agent-ghost' } };
    const getAgents = vi.fn().mockResolvedValue([{ id: 'agent-other', name: 'Other' }]);
    const getMainAgent = vi.fn().mockRejectedValue(new Error('gateway down'));

    const name = await resolveGreetingAgentName(sessionState, getAgents, getMainAgent);

    expect(name).toBe('Pane');
    expect(warnSpy).toHaveBeenCalledTimes(1);
    expect(warnSpy).toHaveBeenCalledWith(
      '[GREETING] Falling back to "Pane" — bound agent lookup: %s, get_main_agent: %s',
      'miss (agent_id bound but not found or empty name)',
      'miss (returned null or threw)'
    );
    warnSpy.mockRestore();
  });

  it('no agent_id on session + get_main_agent returns empty/whitespace name — logs "skip" / "miss (returned but name empty/whitespace)"', async () => {
    const warnSpy = vi.spyOn(console, 'warn').mockImplementation(() => {});
    const sessionState: TestSessionState = { activeSession: null };
    const getAgents = vi.fn().mockResolvedValue([]);
    const getMainAgent = vi.fn().mockResolvedValue({ name: '   ' });

    const name = await resolveGreetingAgentName(sessionState, getAgents, getMainAgent);

    expect(name).toBe('Pane');
    expect(warnSpy).toHaveBeenCalledTimes(1);
    expect(warnSpy).toHaveBeenCalledWith(
      '[GREETING] Falling back to "Pane" — bound agent lookup: %s, get_main_agent: %s',
      'skip (no agent_id on session)',
      'miss (returned but name empty/whitespace)'
    );
    warnSpy.mockRestore();
  });

  it('successful bound-agent resolution never logs the fallback warning', async () => {
    const warnSpy = vi.spyOn(console, 'warn').mockImplementation(() => {});
    const sessionState: TestSessionState = { activeSession: { agent_id: 'agent-blue' } };
    const getAgents = vi.fn().mockResolvedValue([{ id: 'agent-blue', name: 'Blue' }]);
    const getMainAgent = vi.fn().mockResolvedValue({ name: 'Biff' });

    const name = await resolveGreetingAgentName(sessionState, getAgents, getMainAgent);

    expect(name).toBe('Blue');
    expect(warnSpy).not.toHaveBeenCalled();
    warnSpy.mockRestore();
  });

  it('successful default-agent fallback (bound miss but default agent resolves) never logs the fallback warning', async () => {
    const warnSpy = vi.spyOn(console, 'warn').mockImplementation(() => {});
    const sessionState: TestSessionState = { activeSession: { agent_id: null } };
    const getAgents = vi.fn().mockResolvedValue([]);
    const getMainAgent = vi.fn().mockResolvedValue({ name: 'Biff' });

    const name = await resolveGreetingAgentName(sessionState, getAgents, getMainAgent);

    expect(name).toBe('Biff');
    expect(warnSpy).not.toHaveBeenCalled();
    warnSpy.mockRestore();
  });
});
