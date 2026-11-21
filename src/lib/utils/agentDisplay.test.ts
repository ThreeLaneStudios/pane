/**
 * agentDisplay.test.ts — Unit tests for agentDisplay.ts (#2563 B2 fix).
 *
 * B2 replaced two functions that read the retired `ai_settings.provider`
 * global scalar (`agentProviderLabel(agent, activeProvider)`,
 * `filterInClassAgents(agents, activeProvider)`) with per-agent-identity /
 * per-runtime-connectivity equivalents. Pins the exact contradiction from
 * Matt's DMG repro: an OC-bound agent must resolve identically ("OpenClaw")
 * regardless of what any global provider setting says, and must only be
 * excluded from pickers when its OWN runtime is disconnected.
 */

import { describe, it, expect } from 'vitest';
import {
  agentProviderLabel,
  agentRuntimeType,
  filterConnectedAgents,
  filterOutWorkshopSessions,
  agentInitials,
  isAgentModelEditable,
  groupAgentsByProvider,
  parseCrewAgentIds,
  capCrewChips,
} from './agentDisplay';
import type { Agent, Session } from '$lib/types';
import type { RuntimeConnectionInfo } from '$lib/api';
import { WORKSHOP_SESSION_SCOPE_ID } from '$lib/constants';

function mkAgent(overrides: Partial<Agent> = {}): Agent {
  return {
    id: 'a1',
    name: 'Test Agent',
    slug: 'test-agent',
    role: 'sub',
    description: null,
    model: 'claude-sonnet-4-5',
    is_default: false,
    is_active: true,
    workspace_path: '/tmp/a1',
    soul_hash: null,
    memory_hash: null,
    config_json: '{}',
    owner_id: null,
    runtime_ref: null,
    runtime_type: 'anthropic',
    source: 'local',
    created_at: '2026-01-01T00:00:00Z',
    updated_at: '2026-01-01T00:00:00Z',
    ...overrides,
  };
}

function mkConnection(overrides: Partial<RuntimeConnectionInfo> = {}): RuntimeConnectionInfo {
  return {
    runtime_type: 'anthropic',
    connected: true,
    masked_credential: null,
    endpoint: null,
    error: null,
    agents: [],
    ...overrides,
  };
}

describe('agentProviderLabel', () => {
  it('returns "OpenClaw" for an OC-imported agent (runtime_ref non-null)', () => {
    const agent = mkAgent({ runtime_ref: 'oc-123', runtime_type: 'openclaw' });
    expect(agentProviderLabel(agent)).toBe('OpenClaw');
  });

  it('returns "OpenClaw" for an OC-imported agent regardless of any hypothetical global state', () => {
    // Pin: the exact contradiction from Matt's DMG — an OC-bound agent must
    // show "OpenClaw" purely from its own identity, with no second
    // parameter to disagree with.
    const agent = mkAgent({ runtime_ref: 'oc-123', runtime_type: 'openclaw' });
    expect(agentProviderLabel(agent)).toBe('OpenClaw');
    // Calling it twice for the same agent must be stable — no hidden state.
    expect(agentProviderLabel(agent)).toBe(agentProviderLabel(agent));
  });

  it('returns the runtime_type display label for a locally-created, pinned agent', () => {
    expect(agentProviderLabel(mkAgent({ runtime_ref: null, runtime_type: 'openrouter' }))).toBe('OpenRouter');
    expect(agentProviderLabel(mkAgent({ runtime_ref: null, runtime_type: 'anthropic' }))).toBe('Anthropic');
    expect(agentProviderLabel(mkAgent({ runtime_ref: null, runtime_type: 'ollama' }))).toBe('Ollama');
  });

  it('falls back to "Local" for an unpinned legacy agent (runtime_type null, D4.3)', () => {
    expect(agentProviderLabel(mkAgent({ runtime_ref: null, runtime_type: null }))).toBe('Local');
  });

  it('falls back to the raw runtime_type string for an unrecognized value', () => {
    expect(agentProviderLabel(mkAgent({ runtime_ref: null, runtime_type: 'future-runtime' }))).toBe('future-runtime');
  });
});

describe('agentRuntimeType', () => {
  it('returns "openclaw" for an OC-imported agent', () => {
    expect(agentRuntimeType(mkAgent({ runtime_ref: 'oc-1', runtime_type: 'openclaw' }))).toBe('openclaw');
  });

  it('returns the agent\'s own runtime_type for a locally-created agent', () => {
    expect(agentRuntimeType(mkAgent({ runtime_ref: null, runtime_type: 'openrouter' }))).toBe('openrouter');
  });

  it('returns "none" for an unpinned legacy agent', () => {
    expect(agentRuntimeType(mkAgent({ runtime_ref: null, runtime_type: null }))).toBe('none');
  });
});

describe('filterConnectedAgents', () => {
  it('includes an active agent whose runtime_type is connected', () => {
    const agents = [mkAgent({ id: 'a1', runtime_type: 'anthropic', is_active: true })];
    const connections = [mkConnection({ runtime_type: 'anthropic', connected: true })];
    expect(filterConnectedAgents(agents, connections).map(a => a.id)).toEqual(['a1']);
  });

  it('excludes an agent whose runtime is disconnected', () => {
    const agents = [mkAgent({ id: 'a1', runtime_type: 'anthropic', is_active: true })];
    const connections = [mkConnection({ runtime_type: 'anthropic', connected: false })];
    expect(filterConnectedAgents(agents, connections)).toEqual([]);
  });

  it('excludes an inactive agent even on a connected runtime (is_active gate)', () => {
    const agents = [mkAgent({ id: 'a1', runtime_type: 'anthropic', is_active: false })];
    const connections = [mkConnection({ runtime_type: 'anthropic', connected: true })];
    expect(filterConnectedAgents(agents, connections)).toEqual([]);
  });

  it('excludes an unpinned legacy agent (runtime_type null) unconditionally', () => {
    const agents = [mkAgent({ id: 'a1', runtime_type: null, is_active: true })];
    const connections = [mkConnection({ runtime_type: 'anthropic', connected: true })];
    expect(filterConnectedAgents(agents, connections)).toEqual([]);
  });

  it('mixed scenario: OC agent + OpenRouter agent, both runtimes connected → both included', () => {
    const agents = [
      // role: 'main' — a non-sub OC-imported agent (#2588: default fixture role='sub'
      // would now be misread as a real subagent and wrongly excluded).
      mkAgent({ id: 'oc-1', role: 'main', runtime_ref: 'oc-1', runtime_type: 'openclaw', is_active: true }),
      mkAgent({ id: 'or-1', runtime_ref: null, runtime_type: 'openrouter', is_active: true }),
    ];
    const connections = [
      mkConnection({ runtime_type: 'openclaw', connected: true }),
      mkConnection({ runtime_type: 'openrouter', connected: true }),
    ];
    const result = filterConnectedAgents(agents, connections).map(a => a.id).sort();
    expect(result).toEqual(['oc-1', 'or-1']);
  });

  it('disconnected runtime excludes its agents but other runtime\'s agents still shown', () => {
    const agents = [
      mkAgent({ id: 'oc-1', runtime_ref: 'oc-1', runtime_type: 'openclaw', is_active: true }),
      mkAgent({ id: 'or-1', runtime_ref: null, runtime_type: 'openrouter', is_active: true }),
    ];
    const connections = [
      mkConnection({ runtime_type: 'openclaw', connected: false }),
      mkConnection({ runtime_type: 'openrouter', connected: true }),
    ];
    expect(filterConnectedAgents(agents, connections).map(a => a.id)).toEqual(['or-1']);
  });

  it('returns an empty array when no connections are connected', () => {
    const agents = [mkAgent({ id: 'a1', runtime_type: 'anthropic', is_active: true })];
    const connections = [
      mkConnection({ runtime_type: 'openclaw', connected: false }),
      mkConnection({ runtime_type: 'openrouter', connected: false }),
      mkConnection({ runtime_type: 'anthropic', connected: false }),
      mkConnection({ runtime_type: 'ollama', connected: false }),
    ];
    expect(filterConnectedAgents(agents, connections)).toEqual([]);
  });

  it('returns an empty array for an empty agent list', () => {
    expect(filterConnectedAgents([], [mkConnection({ connected: true })])).toEqual([]);
  });

  // #2588 C4 (Matt's ruling: subagent sessions are always-no) — same
  // predicate as DefaultAgentPickerModal's `eligibleAgents` filter (#2567/
  // #2577 B5) and the backend `set_default_agent_impl` guard: exclude only
  // when role='sub' AND runtime_ref is set (a real OC-imported subagent).
  it('excludes a real subagent (role=sub, runtime_ref set) even on a connected, active runtime', () => {
    const agents = [mkAgent({ id: 'sub-1', role: 'sub', runtime_ref: 'oc-sub-1', runtime_type: 'openclaw', is_active: true })];
    const connections = [mkConnection({ runtime_type: 'openclaw', connected: true })];
    expect(filterConnectedAgents(agents, connections)).toEqual([]);
  });

  it('includes a locally-created agent that shares role=sub but has runtime_ref null', () => {
    const agents = [mkAgent({ id: 'local-1', role: 'sub', runtime_ref: null, runtime_type: 'anthropic', is_active: true })];
    const connections = [mkConnection({ runtime_type: 'anthropic', connected: true })];
    expect(filterConnectedAgents(agents, connections).map(a => a.id)).toEqual(['local-1']);
  });

  it('includes a non-sub agent that has a runtime_ref set', () => {
    const agents = [mkAgent({ id: 'oc-1', role: 'local', runtime_ref: 'oc-1', runtime_type: 'openclaw', is_active: true })];
    const connections = [mkConnection({ runtime_type: 'openclaw', connected: true })];
    expect(filterConnectedAgents(agents, connections).map(a => a.id)).toEqual(['oc-1']);
  });

  it('mixed roster: subagents absent, local agents and non-sub runtime_ref agents present', () => {
    const subAg = mkAgent({ id: 'sub-1', role: 'sub', runtime_ref: 'oc-sub-1', runtime_type: 'openclaw', is_active: true });
    const localAg = mkAgent({ id: 'local-1', role: 'local', runtime_ref: null, runtime_type: 'openrouter', is_active: true });
    const legacyLocalAg = mkAgent({ id: 'local-2', role: 'sub', runtime_ref: null, runtime_type: 'anthropic', is_active: true });
    const nonSubRuntimeRefAg = mkAgent({ id: 'oc-1', role: 'local', runtime_ref: 'oc-1', runtime_type: 'openclaw', is_active: true });
    const connections = [
      mkConnection({ runtime_type: 'openclaw', connected: true }),
      mkConnection({ runtime_type: 'openrouter', connected: true }),
      mkConnection({ runtime_type: 'anthropic', connected: true }),
    ];
    const result = filterConnectedAgents([subAg, localAg, legacyLocalAg, nonSubRuntimeRefAg], connections).map(a => a.id).sort();
    expect(result).toEqual(['local-1', 'local-2', 'oc-1']);
  });
});

function mkSession(overrides: Partial<Session> = {}): Session {
  return {
    id: 'sess-1',
    project_id: 'workspace',
    title: '',
    origin: 'user',
    created_at: '2026-01-01T00:00:00Z',
    last_active_at: '2026-01-01T00:00:00Z',
    hidden_at: null,
    is_active: false,
    agent_id: 'agent-1',
    ...overrides,
  };
}

describe('filterOutWorkshopSessions (#2618 C4-adjacent filter)', () => {
  it('excludes sessions scoped to WORKSHOP_SESSION_SCOPE_ID', () => {
    const workshopSession = mkSession({ id: 'sess-workshop', project_id: WORKSHOP_SESSION_SCOPE_ID });
    const realSession = mkSession({ id: 'sess-real', project_id: 'workspace' });
    const result = filterOutWorkshopSessions([workshopSession, realSession]);
    expect(result.map(s => s.id)).toEqual(['sess-real']);
  });

  it('is a no-op when no sessions are workshop-scoped', () => {
    const sessions = [mkSession({ id: 's1' }), mkSession({ id: 's2', project_id: 'project-abc' })];
    expect(filterOutWorkshopSessions(sessions).map(s => s.id)).toEqual(['s1', 's2']);
  });

  it('returns an empty array (not a throw) when every session is workshop-scoped', () => {
    const sessions = [
      mkSession({ id: 's1', project_id: WORKSHOP_SESSION_SCOPE_ID }),
      mkSession({ id: 's2', project_id: WORKSHOP_SESSION_SCOPE_ID }),
    ];
    expect(filterOutWorkshopSessions(sessions)).toEqual([]);
  });

  it('never confuses the notes-only WORKSHOP_SCOPE_ID sentinel with the session sentinel', () => {
    // A session whose project_id happens to be the notes-only sentinel ("__workshop__")
    // should NOT be excluded — only the distinct WORKSHOP_SESSION_SCOPE_ID
    // ("__workshop_session__") is a session-list exclusion target. In practice
    // no session is ever created with project_id="__workshop__" (createSession
    // is only ever called with WORKSHOP_SESSION_SCOPE_ID for workshop chats),
    // but this pins the two sentinels are treated as genuinely distinct values.
    const session = mkSession({ id: 'sess-notes-scope', project_id: '__workshop__' });
    expect(filterOutWorkshopSessions([session])).toEqual([session]);
  });
});

describe('isAgentModelEditable (#3074 gate predicate)', () => {
  // The six cases Matt's ruling names explicitly: four editable API runtimes,
  // plus the two deliberate exclusions (openclaw, legacy-unpinned null).

  it('is editable for runtime_type "anthropic"', () => {
    expect(isAgentModelEditable(mkAgent({ runtime_type: 'anthropic' }))).toBe(true);
  });

  it('is editable for runtime_type "openrouter"', () => {
    expect(isAgentModelEditable(mkAgent({ runtime_type: 'openrouter' }))).toBe(true);
  });

  it('is editable for runtime_type "venice"', () => {
    expect(isAgentModelEditable(mkAgent({ runtime_type: 'venice' }))).toBe(true);
  });

  it('is editable for runtime_type "ollama"', () => {
    expect(isAgentModelEditable(mkAgent({ runtime_type: 'ollama' }))).toBe(true);
  });

  it('is NOT editable for runtime_type "openclaw" — write_back_agent_model_if_changed (ADR-0063 D9) would revert any manual edit on the next completed turn', () => {
    expect(isAgentModelEditable(mkAgent({ runtime_type: 'openclaw' }))).toBe(false);
  });

  it('is NOT editable for runtime_type null — the legacy-unpinned migration state (ADR-0063 D4.3) has no runtime to fetch models from yet', () => {
    expect(isAgentModelEditable(mkAgent({ runtime_type: null }))).toBe(false);
  });
});

describe('agentInitials (unchanged, sanity check)', () => {
  it('returns two-letter initials for a full name', () => {
    expect(agentInitials('Brit Freed')).toBe('BF');
  });

  it('returns first two letters for a single-word name', () => {
    expect(agentInitials('Pane')).toBe('PA');
  });
});

describe('groupAgentsByProvider (#3050 PR 10 — extracted from AgentPickerRows.svelte, byte-identical)', () => {
  it('groups by provider label, default-first then name within each group', () => {
    const venice1 = mkAgent({ id: 'v1', name: 'Vernon', runtime_type: 'anthropic' });
    const veniceDefault = mkAgent({ id: 'v2', name: 'Valencia', runtime_type: 'anthropic', is_default: true });
    const openrouter1 = mkAgent({ id: 'o1', name: 'Astro', runtime_type: 'openrouter' });
    const groups = groupAgentsByProvider([venice1, openrouter1, veniceDefault]);
    const anthropicGroup = groups.find(([label]) => label === 'Anthropic');
    expect(anthropicGroup).toBeTruthy();
    // Default-first even though it was pushed second.
    expect(anthropicGroup![1].map(a => a.id)).toEqual(['v2', 'v1']);
    expect(groups.find(([label]) => label === 'OpenRouter')![1].map(a => a.id)).toEqual(['o1']);
  });

  it('preserves first-seen group order', () => {
    const oc = mkAgent({ id: 'oc-1', name: 'Brit', runtime_ref: 'oc-1', runtime_type: 'openclaw' });
    const venice = mkAgent({ id: 'v-1', name: 'Vernon', runtime_type: 'anthropic' });
    const groups = groupAgentsByProvider([oc, venice]);
    expect(groups.map(([label]) => label)).toEqual(['OpenClaw', 'Anthropic']);
  });

  // Mutation: dropped the `is_default` comparator term from the sort
  // comparator, leaving only `a.name.localeCompare(b.name)`. Result: red —
  // the first test above failed (`['v1', 'v2']` instead of `['v2', 'v1']`,
  // since 'Valencia' < 'Vernon' alphabetically would already pass by name
  // alone in THIS fixture — re-ran with names swapped so the mutation is
  // actually exercised: 'Zelda' (default) vs 'Aaron' (not default) proves
  // default-first wins over alphabetical). Reverted; confirmed green.
  it('default-first wins over alphabetical ordering within a group', () => {
    const notDefault = mkAgent({ id: 'a1', name: 'Aaron', runtime_type: 'anthropic', is_default: false });
    const defaultAgent = mkAgent({ id: 'z1', name: 'Zelda', runtime_type: 'anthropic', is_default: true });
    const groups = groupAgentsByProvider([notDefault, defaultAgent]);
    expect(groups[0][1].map(a => a.id)).toEqual(['z1', 'a1']);
  });
});

describe('parseCrewAgentIds (#3050 PR 10 — the one Session.crew_agent_ids parse site)', () => {
  it('returns [] for null', () => {
    expect(parseCrewAgentIds(null)).toEqual([]);
  });

  it('returns [] for undefined', () => {
    expect(parseCrewAgentIds(undefined)).toEqual([]);
  });

  it('returns [] for empty string', () => {
    expect(parseCrewAgentIds('')).toEqual([]);
  });

  it('parses a valid JSON array of ids', () => {
    expect(parseCrewAgentIds('["a1","a2"]')).toEqual(['a1', 'a2']);
  });

  it('fails closed on invalid JSON — returns [], never throws', () => {
    expect(() => parseCrewAgentIds('not json')).not.toThrow();
    expect(parseCrewAgentIds('not json')).toEqual([]);
  });

  it('fails closed on valid JSON that is not an array', () => {
    expect(parseCrewAgentIds('{"a":1}')).toEqual([]);
  });

  it('filters out non-string entries from a malformed array', () => {
    expect(parseCrewAgentIds('["a1", 42, null, "a2"]')).toEqual(['a1', 'a2']);
  });
});

describe('capCrewChips (#3050 PR 10 — shared by SessionRow.svelte drawer rows and the header crew button)', () => {
  function mkCrewAgent(id: string, name: string) {
    return mkAgent({ id, name });
  }

  it('0 crew: shown=[], overflowCount=0, no tooltip text', () => {
    const result = capCrewChips([]);
    expect(result).toEqual({ shown: [], overflowCount: 0, tooltipText: '' });
  });

  it('1-3 crew: all shown, no overflow', () => {
    const crew = [mkCrewAgent('c1', 'Vanessa'), mkCrewAgent('c2', 'Vernon')];
    const result = capCrewChips(crew);
    expect(result.shown.map(a => a.id)).toEqual(['c1', 'c2']);
    expect(result.overflowCount).toBe(0);
    expect(result.tooltipText).toBe('Crew: Vanessa, Vernon');
  });

  it('4+ crew: exactly 3 shown, overflowCount = count - 3, tooltip lists ALL names (not just overflow)', () => {
    const crew = [
      mkCrewAgent('c1', 'Valencia Tool Test'),
      mkCrewAgent('c2', 'Vanessa'),
      mkCrewAgent('c3', 'Vernon'),
      mkCrewAgent('c4', 'Vernon Reed'),
    ];
    const result = capCrewChips(crew);
    expect(result.shown.map(a => a.id)).toEqual(['c1', 'c2', 'c3']);
    expect(result.overflowCount).toBe(1);
    expect(result.tooltipText).toBe('Crew: Valencia Tool Test, Vanessa, Vernon, Vernon Reed');
  });

  // Mutation: tooltip built from `shown` instead of the full `crew` array
  // (`shown.map(c => c.name).join(', ')`). Result: red — the 4+ test above
  // failed: `tooltipText` became "Crew: Valencia Tool Test, Vanessa,
  // Vernon" (missing "Vernon Reed", the overflowed member) instead of
  // listing all four. This is exactly the bug Reed's scope audit flagged
  // as worth a dedicated assertion ("tooltip lists only the overflowed
  // names" would be the WRONG fix too — must list ALL names). Reverted.
  it('tooltip always derives from the full crew list, not the capped shown list', () => {
    const crew = Array.from({ length: 5 }, (_, i) => mkCrewAgent(`c${i}`, `Agent${i}`));
    const result = capCrewChips(crew);
    expect(result.tooltipText).toContain('Agent4'); // the 5th, overflowed member
    expect(result.overflowCount).toBe(2);
  });
});
