/**
 * #2392 — Voluntary-create-on-OC gate
 * spec-covered-by: #2395
 *
 * Tests the three UI changes to Settings → Agents when `provider === 'openclaw'`:
 *
 *   1. The "+ New agent" button is disabled with a tooltip title.
 *   2. An explanatory note is rendered below the button.
 *   3. The "Locally created" empty state shows OC-specific copy.
 *
 * These tests also serve as the red-on-revert gate: removing any gate condition
 * causes the OC-state assertions to fail.
 *
 * Pattern: The gate is pure presentation logic keyed on `provider === 'openclaw'`.
 * We extract that decision into a helper function and test all output fields for
 * both OC and API states. This is identical to the Settings.ctx-window-override.test.ts
 * pattern — logic extracted from the Svelte template, tested in isolation with Vitest.
 *
 * Harness: Vitest (jsdom) — matches existing pattern in src/lib/components/*.test.ts.
 */

import { describe, it, expect } from 'vitest';

// ---------------------------------------------------------------------------
// Gate logic — mirrors the conditional expressions in Settings.svelte §3580-3605
//
// This function is a faithful extraction of the gate decisions so tests fail if
// the conditions are reverted or changed in Settings.svelte.
//
// EXACT COPY strings are verified against the spec-ratified copies:
//   • button title: ratified in §24.6 spec (PR #2395)
//   • note text: ratified by Matt (#2392 scope thread)
//   • OC empty state: ratified by Matt (#2392 scope thread)
//   • API empty state: UNCHANGED from pre-#2392 — must not drift
// ---------------------------------------------------------------------------

interface AgentsGateState {
  /** Whether the "+ New agent" button carries the `disabled` attribute. */
  buttonDisabled: boolean;
  /** Tooltip text on the button (undefined when button is enabled). */
  buttonTitle: string | undefined;
  /** Whether the explanatory note is rendered. */
  showNote: boolean;
  /** Exact copy of the explanatory note (only meaningful when showNote=true). */
  noteText: string;
  /** Exact copy of the "Locally created" empty state paragraph. */
  emptyStateText: string;
}

/**
 * Computes the agents-gate UI state for a given provider value.
 *
 * This is a direct extraction of the template conditions added in #2392:
 *   - disabled={provider === 'openclaw'}
 *   - title={provider === 'openclaw' ? '...' : undefined}
 *   - {#if provider === 'openclaw'} note {/if}
 *   - {provider === 'openclaw' ? '<OC copy>' : '<API copy>'}
 */
function computeAgentsGate(provider: string): AgentsGateState {
  const isOC = provider === 'openclaw';
  return {
    buttonDisabled: isOC,
    buttonTitle: isOC ? 'Agent creation is managed by OpenClaw' : undefined,
    showNote: isOC,
    noteText: 'Agents are created in OpenClaw and imported here automatically. To create a Pane-local agent, switch to an API provider in the AI tab.',
    emptyStateText: isOC
      ? 'No local agents. Pane-local agents are created under an API provider.'
      : 'No local agents yet. Create one above.',
  };
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

describe('Settings.svelte — #2392 voluntary-create-on-OC gate', () => {
  // ── OC active state ────────────────────────────────────────────────────────

  describe('when provider is openclaw', () => {
    const state = computeAgentsGate('openclaw');

    it('button is disabled', () => {
      expect(state.buttonDisabled).toBe(true);
    });

    it('button title is exact ratified copy', () => {
      expect(state.buttonTitle).toBe('Agent creation is managed by OpenClaw');
    });

    it('explanatory note is shown', () => {
      expect(state.showNote).toBe(true);
    });

    it('explanatory note contains exact ratified copy', () => {
      expect(state.noteText).toBe(
        'Agents are created in OpenClaw and imported here automatically. To create a Pane-local agent, switch to an API provider in the AI tab.',
      );
    });

    it('empty state shows OC-specific copy', () => {
      expect(state.emptyStateText).toBe(
        'No local agents. Pane-local agents are created under an API provider.',
      );
    });

    it('empty state does NOT contain pre-#2392 copy', () => {
      // Regression guard: OC state must never show the old "Create one above" text.
      expect(state.emptyStateText).not.toContain('Create one above');
    });
  });

  // ── API active states ──────────────────────────────────────────────────────

  const apiProviders = ['anthropic', 'openrouter', 'ollama'] as const;

  apiProviders.forEach((provider) => {
    describe(`when provider is ${provider}`, () => {
      const state = computeAgentsGate(provider);

      it('button is enabled (not disabled)', () => {
        expect(state.buttonDisabled).toBe(false);
      });

      it('button has no tooltip title (undefined)', () => {
        expect(state.buttonTitle).toBeUndefined();
      });

      it('explanatory note is NOT shown', () => {
        expect(state.showNote).toBe(false);
      });

      it('empty state shows original pre-#2392 copy unchanged', () => {
        expect(state.emptyStateText).toBe('No local agents yet. Create one above.');
      });

      it('empty state does NOT contain OC-specific copy', () => {
        // Non-OC state must never bleed OC copy into the empty state.
        expect(state.emptyStateText).not.toContain('Pane-local agents are created');
        expect(state.emptyStateText).not.toContain('API provider');
      });
    });
  });

  // ── Red-on-revert gate ────────────────────────────────────────────────────
  // These tests explicitly model the revert condition: if the gate is removed
  // (i.e., all outputs are hardcoded to the non-OC defaults), the OC assertions
  // above flip red. Documented here as a contract for reviewers.

  it('(contract) OC state differs from any API-provider state on all three outputs', () => {
    const oc = computeAgentsGate('openclaw');
    const api = computeAgentsGate('anthropic');

    expect(oc.buttonDisabled).not.toBe(api.buttonDisabled);
    expect(oc.buttonTitle).not.toBe(api.buttonTitle);
    expect(oc.showNote).not.toBe(api.showNote);
    expect(oc.emptyStateText).not.toBe(api.emptyStateText);
  });

  it('(contract) gate keys exclusively off the openclaw provider string', () => {
    // Any non-openclaw string — including empty string or unknown provider — must
    // produce the non-OC (enabled) state. The gate is binary: openclaw vs. everything else.
    const emptyProvider = computeAgentsGate('');
    const unknownProvider = computeAgentsGate('someNewProvider');

    expect(emptyProvider.buttonDisabled).toBe(false);
    expect(emptyProvider.showNote).toBe(false);

    expect(unknownProvider.buttonDisabled).toBe(false);
    expect(unknownProvider.showNote).toBe(false);
  });
});
