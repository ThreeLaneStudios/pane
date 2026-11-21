/**
 * #2587 (C4 follow-up) — Agents tab must exclude deactivated agents
 *
 * `migrate_v83` deactivates legacy agents left over from the retired
 * `runtime_type IS NULL` population (e.g. seed rows like Bob/Murphy) by
 * setting `is_active = 0` on them. That population is already excluded from
 * every picker surface (`filterConnectedAgents`'s `is_active` gate, #2588
 * B5/C4) — but the Agents tab in Settings.svelte read straight off the raw
 * `agentsList` with no `is_active` filter, so a deactivated row was still
 * the one remaining surface where it rendered.
 *
 * Settings.svelte requires the full Tauri window context and cannot be
 * DOM-rendered directly (same constraint documented in Settings.test.ts /
 * Settings.ctx-window-override.test.ts / Settings.oc-create-gate.test.ts).
 * This file follows that established pattern: (1) source-level assertions
 * pin the exact filter is present and wired into both the roster grouping
 * and the header count, and (2) a pure extraction of the filter predicate
 * is unit-tested directly against seeded agent fixtures — including the
 * exact "deactivated legacy agent" shape (`runtime_type: null, is_active:
 * false`) the bug report describes.
 *
 * Harness: Vitest (jsdom) — matches existing pattern in src/lib/components/*.test.ts.
 */

import { describe, it, expect } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import type { Agent } from '$lib/types';

const source = readFileSync(resolve(__dirname, 'Settings.svelte'), 'utf-8');

// ---------------------------------------------------------------------------
// Direct extraction of Settings.svelte's activeAgentsList filter — mirrors
// `const activeAgentsList = $derived(agentsList.filter(a => a.is_active));`
// ---------------------------------------------------------------------------
function activeAgentsList(agents: Pick<Agent, 'is_active'>[]): Pick<Agent, 'is_active'>[] {
  return agents.filter((a) => a.is_active);
}

function mkAgent(overrides: Partial<Agent> = {}): Agent {
  return {
    id: 'a1',
    name: 'Test Agent',
    slug: 'test-agent',
    role: 'main',
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

describe('Settings.svelte — source-level: Agents tab is_active filter (#2587)', () => {
  it('declares an activeAgentsList derived that filters agentsList on is_active', () => {
    expect(source).toMatch(/const activeAgentsList = \$derived\(agentsList\.filter\(a => a\.is_active\)\)/);
  });

  it('agentGroups is built from activeAgentsList, not the raw agentsList', () => {
    expect(source).toMatch(/const agentGroups = \$derived\.by\(\(\) => \{\s*const groups = new Map<string, Agent\[\]>\(\);\s*for \(const agent of activeAgentsList\)/);
  });

  it('the header count reads activeAgentsList.length, not agentsList.length', () => {
    expect(source).toMatch(/AGENTS \(\{activeAgentsList\.length\}\)/);
    expect(source).not.toMatch(/AGENTS \(\{agentsList\.length\}\)/);
  });
});

describe('activeAgentsList (extracted predicate) — #2587', () => {
  it('excludes a deactivated legacy agent (the migrate_v83 shape: runtime_type null, is_active false)', () => {
    const agents = [
      mkAgent({ id: 'legacy-bob', name: 'Bob', runtime_type: null, is_active: false }),
    ];
    expect(activeAgentsList(agents)).toEqual([]);
  });

  it('excludes a deactivated legacy agent even when it has a runtime_type set', () => {
    const agents = [
      mkAgent({ id: 'legacy-murphy', name: 'Murphy', runtime_type: 'openrouter', is_active: false }),
    ];
    expect(activeAgentsList(agents)).toEqual([]);
  });

  it('includes an active agent', () => {
    const agents = [mkAgent({ id: 'a1', is_active: true })];
    expect(activeAgentsList(agents).map((a: any) => a.id)).toEqual(['a1']);
  });

  it('mixed roster: only active agents pass through, deactivated legacy rows are absent', () => {
    const active1 = mkAgent({ id: 'active-1', name: 'Brit Freed', is_active: true });
    const active2 = mkAgent({ id: 'active-2', name: 'Research', is_active: true, runtime_type: 'openrouter' });
    const deactivatedBob = mkAgent({ id: 'legacy-bob', name: 'Bob', runtime_type: null, is_active: false });
    const deactivatedMurphy = mkAgent({ id: 'legacy-murphy', name: 'Murphy', runtime_type: null, is_active: false });

    const result = activeAgentsList([active1, deactivatedBob, active2, deactivatedMurphy]);
    expect(result.map((a: any) => a.id)).toEqual(['active-1', 'active-2']);
  });

  it('returns an empty array when every agent is deactivated', () => {
    const agents = [
      mkAgent({ id: 'legacy-bob', is_active: false }),
      mkAgent({ id: 'legacy-murphy', is_active: false }),
    ];
    expect(activeAgentsList(agents)).toEqual([]);
  });

  it('returns an empty array for an empty roster', () => {
    expect(activeAgentsList([])).toEqual([]);
  });
});
