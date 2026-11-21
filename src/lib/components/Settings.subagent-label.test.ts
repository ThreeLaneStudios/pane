/**
 * #2567 B6 — subagent-label conditional: role='local' must NOT render the label
 *
 * Settings.svelte's agents-roster row renders a "subagent" badge next to an
 * agent's name when:
 *
 *   {#if agent.role === 'sub' && !agent.is_default && agent.runtime_ref != null}
 *     <span class="subagent-label">subagent</span>
 *   {/if}
 *
 * Matt's ruling (#2567 B6): a user-created agent is never a subagent,
 * structurally. Locally-created agents now birth with role='local' (was
 * role='sub' + runtime_ref=NULL workaround prior to this fix — see
 * migrate_v81 for the one-time data repair of pre-existing rows).
 *
 * This test is explicitly scoped to NEW test coverage only — B6's scope
 * excludes touching Settings.svelte's conditional logic itself (only new
 * tests exercising the existing, unmodified behavior). Pattern mirrors
 * Settings.oc-create-gate.test.ts: extract the exact template condition into
 * a pure function, test it in isolation with Vitest (Settings.svelte requires
 * full Tauri window context and cannot be DOM-rendered directly, per the same
 * constraint documented in Settings.test.ts / Settings.ctx-window-override.test.ts).
 *
 * Harness: Vitest (jsdom) — matches existing pattern in src/lib/components/*.test.ts.
 */

import { describe, it, expect } from 'vitest';

interface AgentRoleShape {
  role: string;
  is_default: boolean;
  runtime_ref: string | null;
}

/**
 * Direct extraction of Settings.svelte's subagent-label condition
 * (agent.role === 'sub' && !agent.is_default && agent.runtime_ref != null).
 */
function showsSubagentLabel(agent: AgentRoleShape): boolean {
  return agent.role === 'sub' && !agent.is_default && agent.runtime_ref != null;
}

describe("Settings.svelte — #2567 B6 subagent-label conditional", () => {
  it("role='local' agent (locally-created, post-B6 birth label) does NOT render the subagent label", () => {
    const agent: AgentRoleShape = { role: 'local', is_default: false, runtime_ref: null };
    expect(showsSubagentLabel(agent)).toBe(false);
  });

  it("role='local' default agent does NOT render the subagent label", () => {
    const agent: AgentRoleShape = { role: 'local', is_default: true, runtime_ref: null };
    expect(showsSubagentLabel(agent)).toBe(false);
  });

  it("role='sub' agent with runtime_ref set (real OC subagent) still renders the label — unaffected by B6", () => {
    const agent: AgentRoleShape = { role: 'sub', is_default: false, runtime_ref: 'oc-sub-1' };
    expect(showsSubagentLabel(agent)).toBe(true);
  });

  it("role='sub' default agent does NOT render the label (is_default excludes it) — unaffected by B6", () => {
    const agent: AgentRoleShape = { role: 'sub', is_default: true, runtime_ref: 'oc-sub-1' };
    expect(showsSubagentLabel(agent)).toBe(false);
  });

  it("pre-migration shape (role='sub', runtime_ref=null — the old birth mislabel) does NOT render the label", () => {
    // This is the exact shape create_local_agent_impl wrote before B6's fix, and the
    // exact shape migrate_v81 repairs to role='local'. Confirms the B5 workaround
    // conjunct (runtime_ref != null) already covered this case pre-migration too —
    // the label was never mistakenly shown for locally-created agents, migrated or not.
    const agent: AgentRoleShape = { role: 'sub', is_default: false, runtime_ref: null };
    expect(showsSubagentLabel(agent)).toBe(false);
  });

  it("role='main' agent does NOT render the label", () => {
    const agent: AgentRoleShape = { role: 'main', is_default: true, runtime_ref: 'main' };
    expect(showsSubagentLabel(agent)).toBe(false);
  });
});
