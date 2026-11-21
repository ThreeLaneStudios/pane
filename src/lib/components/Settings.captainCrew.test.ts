/**
 * Settings.captainCrew.test.ts — §36.17.4 (#3050 PR 10, Captain & Crew UI),
 * Rulings 2 & item 5.
 *
 * Settings.svelte requires the full Tauri window context and cannot be
 * DOM-rendered directly (same constraint documented in Settings.test.ts /
 * Settings.ctx-window-override.test.ts / Settings.agents-tab-active-filter.test.ts).
 * This file follows that established pattern: source-level assertions pin
 * the exact badge/helper markup, and a pure extraction of the badge
 * predicate is unit-tested directly.
 *
 * Ruling 2 (Matt, 2026-10-29): "The Settings 'Main' badge and the sheet's
 * chip both read DEFAULT CAPTAIN, from one shared string constant so they
 * can't drift. 'Main' goes away." — this file pins the Settings side;
 * NewSessionSheet.test.ts pins the sheet side; both import the SAME
 * `DEFAULT_CAPTAIN_LABEL` constant.
 */

import { describe, it, expect } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { DEFAULT_CAPTAIN_LABEL } from '$lib/constants';

const source = readFileSync(resolve(__dirname, 'Settings.svelte'), 'utf-8');

describe('Settings.svelte — source-level: DEFAULT CAPTAIN badge (Ruling 2, #3050 PR 10)', () => {
  it('imports DEFAULT_CAPTAIN_LABEL from $lib/constants — not a second hardcoded string', () => {
    expect(source).toMatch(/import \{ DEFAULT_CAPTAIN_LABEL \} from '\$lib\/constants'/);
  });

  it('the agents-main-badge renders the constant, not a literal "Main"/"DEFAULT CAPTAIN" string', () => {
    expect(source).toMatch(/<span class="badge badge-teal agents-main-badge">\{DEFAULT_CAPTAIN_LABEL\}<\/span>/);
    // The old literal must be gone from this exact badge line.
    expect(source).not.toMatch(/agents-main-badge">Main</);
  });

  it('the badge condition is still exactly {#if agent.is_default} — only the label text changed, not the source/condition', () => {
    expect(source).toMatch(/\{#if agent\.is_default\}\s*<span class="badge badge-teal agents-main-badge">\{DEFAULT_CAPTAIN_LABEL\}<\/span>\s*\{\/if\}/);
  });
});

describe('Settings.svelte — source-level: Agents tab helper line (item 5, #3050 PR 10)', () => {
  it('renders the exact helper copy once, near the top of the Agents tab content', () => {
    expect(source).toContain('Any agent can be a captain (runs a session) or crew (takes jobs from a captain)');
  });

  it('the helper line appears exactly once in the whole file (not once per group)', () => {
    const matches = source.match(/Any agent can be a captain \(runs a session\) or crew \(takes jobs from a captain\)/g) ?? [];
    expect(matches).toHaveLength(1);
  });

  it('the helper paragraph sits above the agentGroups iteration (top of tab content), not inside the per-group loop', () => {
    const helperIdx = source.indexOf('Any agent can be a captain');
    const groupsLoopIdx = source.indexOf('{#each agentGroups as group, groupIdx');
    expect(helperIdx).toBeGreaterThan(-1);
    expect(groupsLoopIdx).toBeGreaterThan(-1);
    expect(helperIdx).toBeLessThan(groupsLoopIdx);
  });
});

// ---------------------------------------------------------------------------
// Pure extraction of the badge-show predicate — mirrors
// `{#if agent.is_default}` exactly. Source confirmed above as unchanged.
// ---------------------------------------------------------------------------
function showsDefaultCaptainBadge(agent: { is_default: boolean }): boolean {
  return agent.is_default;
}

describe('showsDefaultCaptainBadge (extracted predicate) — follows is_default specifically', () => {
  it('shows the badge when is_default is true', () => {
    expect(showsDefaultCaptainBadge({ is_default: true })).toBe(true);
  });

  it('does NOT show the badge when is_default is false', () => {
    expect(showsDefaultCaptainBadge({ is_default: false })).toBe(false);
  });

  // Mutation: hardcoded the predicate to always return `true`. Result:
  // red — the "does NOT show" test above failed (expected false, got
  // true). Reverted.
  //
  // A second predicate — "follows is_default specifically, not some
  // other derived flag like role==='main'" — was considered (per the
  // scope audit's Commit 6 test guidance) but is NOT written here: the
  // current Agent/Settings data model has no reachable state where
  // `role === 'main'` and `is_default === false` diverge from what
  // `is_default` alone already decides (role is a classification axis,
  // is_default is a separate boolean flag set by
  // `set_default_agent_impl` — nothing in that command or its callers
  // ties the two together, and no fixture in this repo's existing test
  // suites constructs that combination as a meaningful state to assert
  // against). Saying so explicitly here rather than fabricating an
  // impossible-to-justify fixture, per Reed's scope audit's own caution.
});

describe('DEFAULT_CAPTAIN_LABEL — the shared constant itself', () => {
  it('is the literal "DEFAULT CAPTAIN"', () => {
    expect(DEFAULT_CAPTAIN_LABEL).toBe('DEFAULT CAPTAIN');
  });
});
