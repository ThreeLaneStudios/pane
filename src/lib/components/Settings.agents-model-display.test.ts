/**
 * #2587 (D9 follow-up) — Agents tab must never show a model string for subagents
 *
 * Subagents (`role === 'sub'` with a real OC `runtime_ref`, e.g. Lane, Reed,
 * Stia, Pane Support) are dispatched gateway-side — `write_back_agent_model_if_changed`
 * (D9) never fires for them, so their `ai_agents.model` column stays whatever
 * `migrate_v83` seeded it as (the `'openclaw-pending'` sentinel) forever.
 * Showing that column on the Agents tab would mean showing the raw internal
 * sentinel string permanently instead of a truthful "no model to show" state.
 *
 * Display rule: `role === 'sub' && runtime_ref != null` → hide the model
 * segment entirely. Every other agent (including "Brit Freed", the main OC
 * agent — `role='main'`/`role='local'` with `runtime_ref` set — and any
 * Pane-dispatched non-sub agent) keeps showing its real `model` string.
 *
 * Settings.svelte requires the full Tauri window context and cannot be
 * DOM-rendered directly (same constraint documented in Settings.test.ts /
 * Settings.subagent-label.test.ts). This file follows that established
 * pattern: source-level assertions pin the exact template condition, and a
 * pure extraction of the same condition is unit-tested against fixtures.
 *
 * Harness: Vitest (jsdom) — matches existing pattern in src/lib/components/*.test.ts.
 */

import { describe, it, expect } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

const source = readFileSync(resolve(__dirname, 'Settings.svelte'), 'utf-8');

// ---------------------------------------------------------------------------
// Direct extraction of Settings.svelte's sublabel model-display condition:
//   {agent.slug} · {group.label}{(agent.role === 'sub' && agent.runtime_ref != null)
//     ? '' : (agent.model ? ` · ${agent.model}` : '')}
// ---------------------------------------------------------------------------
interface AgentModelDisplayShape {
  role: string;
  runtime_ref: string | null;
  model: string | null;
}

function renderedModelSegment(agent: AgentModelDisplayShape): string {
  const hideModel = agent.role === 'sub' && agent.runtime_ref != null;
  if (hideModel) return '';
  return agent.model ? ` · ${agent.model}` : '';
}

describe('Settings.svelte — source-level: subagent model-display suppression (#2587)', () => {
  it('the sublabel model segment is conditioned on role===\'sub\' && runtime_ref != null', () => {
    expect(source).toMatch(
      /\{agent\.slug\} · \{group\.label\}\{\(agent\.role === 'sub' && agent\.runtime_ref != null\) \? '' : \(agent\.model \? ` · \$\{agent\.model\}` : ''\)\}/,
    );
  });

  it('does not unconditionally interpolate agent.model into the sublabel anymore', () => {
    expect(source).not.toMatch(/\{agent\.slug\} · \{group\.label\}\{agent\.model \? ` · \$\{agent\.model\}` : ''\}/);
  });
});

describe('renderedModelSegment (extracted predicate) — #2587', () => {
  it('hides the model for a real subagent (role=sub, runtime_ref set) even with a resolved model', () => {
    const agent: AgentModelDisplayShape = { role: 'sub', runtime_ref: 'oc-sub-lane', model: 'claude-sonnet-4-5' };
    expect(renderedModelSegment(agent)).toBe('');
  });

  it('hides the model for a subagent still carrying the openclaw-pending sentinel', () => {
    const agent: AgentModelDisplayShape = { role: 'sub', runtime_ref: 'oc-sub-reed', model: 'openclaw-pending' };
    expect(renderedModelSegment(agent)).toBe('');
  });

  it('hides the model for a subagent with a null model too (defense-in-depth, same outcome)', () => {
    const agent: AgentModelDisplayShape = { role: 'sub', runtime_ref: 'oc-sub-stia', model: null };
    expect(renderedModelSegment(agent)).toBe('');
  });

  it('shows the real model string for the main OC agent ("Brit Freed" shape: role=main, runtime_ref set)', () => {
    const agent: AgentModelDisplayShape = { role: 'main', runtime_ref: 'main', model: 'anthropic/claude-sonnet-4-6' };
    expect(renderedModelSegment(agent)).toBe(' · anthropic/claude-sonnet-4-6');
  });

  it('shows the real model string for a Pane-dispatched non-sub agent with a runtime_ref', () => {
    const agent: AgentModelDisplayShape = { role: 'local', runtime_ref: 'oc-1', model: 'gpt-5' };
    expect(renderedModelSegment(agent)).toBe(' · gpt-5');
  });

  it('shows the real model string for a locally-created agent that shares role=sub but has runtime_ref null', () => {
    // #2567/#2588 B5: locally-created agents can share role='sub' pre-migration but are
    // never real subagents when runtime_ref is null — must not be swept up by this gate.
    const agent: AgentModelDisplayShape = { role: 'sub', runtime_ref: null, model: 'claude-sonnet-4-5' };
    expect(renderedModelSegment(agent)).toBe(' · claude-sonnet-4-5');
  });

  it('shows nothing (empty segment) for a non-sub agent with no model set, unrelated to the hide rule', () => {
    const agent: AgentModelDisplayShape = { role: 'local', runtime_ref: null, model: null };
    expect(renderedModelSegment(agent)).toBe('');
  });

  it('mixed roster: subagent model hidden, main agent and local agent models shown', () => {
    const mainAgent: AgentModelDisplayShape = { role: 'main', runtime_ref: 'main', model: 'anthropic/claude-sonnet-4-6' };
    const subAgent: AgentModelDisplayShape = { role: 'sub', runtime_ref: 'oc-sub-lane', model: 'openclaw-pending' };
    const localAgent: AgentModelDisplayShape = { role: 'local', runtime_ref: null, model: 'gpt-5' };

    expect(renderedModelSegment(mainAgent)).toBe(' · anthropic/claude-sonnet-4-6');
    expect(renderedModelSegment(subAgent)).toBe('');
    expect(renderedModelSegment(localAgent)).toBe(' · gpt-5');
  });
});
