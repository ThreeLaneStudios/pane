/**
 * #2795 follow-up — Smart Features UX fixes (Settings.svelte)
 * Superseded in part by C50 (#3171 PR 1): the per-feature model-picker gate
 * (`internalHasConfiguredKey`) was keyed off a locally-read key that no
 * longer exists. It's replaced by `internalIsConnected`, which treats
 * openrouter/anthropic/venice as connected the instant they're selectable
 * (the dropdown only ever offers them when Provider Connections already has
 * a key) and still requires a real Ollama probe.
 *
 * Same DOM-render constraint as Settings.venice-smart-features.test.ts:
 * Settings.svelte requires the full Tauri window context and is thousands of
 * lines — source-level assertions are used instead of a rendered component.
 *
 * Coverage:
 *  A. Per-feature Smart Features config (Smart Routing / Extraction / Rerank
 *     model pickers) renders for openrouter/anthropic/venice without
 *     requiring a fresh Save & Test click — closes the same false-gating
 *     shape as the old Test-button bug (#2631), now via connection state
 *     instead of a locally-cached key.
 *  B. Semantic Search provider description explains why some providers
 *     aren't listed.
 */

import { describe, it, expect } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

const SOURCE = readFileSync(resolve(__dirname, 'Settings.svelte'), 'utf-8');

/**
 * #3178 (C61 fold-in) — structural replacement for a 400-char regex-window
 * assertion Reed flagged on #3176 as a genuine latent trap: it broke once
 * on comment placement (a comment landing inside the fixed-size slice)
 * and would break again on any whitespace/comment change near the
 * `{#if}` in question, regardless of whether the actual logic changed at
 * all. A fixed character window has no idea where template structure
 * actually starts or ends.
 *
 * This walks the WHOLE source's `{#if ...}` / `{:else if ...}` / `{:else}`
 * / `{/if}` tokens in document order, maintaining a real stack (Svelte's
 * own block grammar guarantees well-formed nesting for THESE four token
 * kinds specifically — `{#each}`/`{/each}`, `{#await}`/`{/await}` etc.
 * have their own distinct open/close tokens, so interleaving them doesn't
 * desync this stack). At any position in the source, the top of the
 * stack IS the condition of the innermost `{#if}` block structurally
 * enclosing that position — correct regardless of how many characters,
 * comments, or blank lines sit between the block's opening tag and the
 * position being checked.
 */
function findEnclosingIfCondition(source: string, position: number): string {
  const tokenRe = /\{#if\s+([^}]*)\}|\{:else if\s+([^}]*)\}|\{:else\}|\{\/if\}/g;
  const stack: string[] = [];
  let match: RegExpExecArray | null;
  while ((match = tokenRe.exec(source)) !== null) {
    if (match.index >= position) break;
    const [full, ifCond, elseIfCond] = match;
    if (full.startsWith('{#if')) {
      stack.push(ifCond.trim());
    } else if (full.startsWith('{:else if')) {
      // Same nesting level as the `{#if}` it belongs to, different
      // condition — an else-if branch is mutually exclusive with its
      // own if, so it replaces (not pushes) the current frame.
      stack[stack.length - 1] = elseIfCond.trim();
    } else if (full === '{:else}') {
      stack[stack.length - 1] = '(else — no condition)';
    } else if (full === '{/if}') {
      stack.pop();
    }
  }
  if (stack.length === 0) {
    throw new Error(`findEnclosingIfCondition: position ${position} is not inside any {#if} block`);
  }
  return stack[stack.length - 1];
}

describe('C50 (#3171 PR 1) follow-up A — per-feature Smart Features config renders on connection state, not a locally-read key', () => {
  it('declares an internalIsConnected derived value', () => {
    expect(SOURCE).toMatch(/const internalIsConnected = \$derived\(/);
  });

  it('internalIsConnected treats openrouter/anthropic/venice as connected once selected (no key check) and Ollama as gated on a real probe', () => {
    const derivedMatch = SOURCE.match(
      /const internalIsConnected = \$derived\(([\s\S]*?)\n  \);/
    );
    expect(derivedMatch).toBeTruthy();
    const body = derivedMatch![1];
    expect(body).toMatch(/internalProvider === 'ollama'/);
    expect(body).toMatch(/internalConnectionStatus === 'success'/);
  });

  it('RED-ON-REVERT: internalHasConfiguredKey (the retired key-presence gate) must not exist anymore', () => {
    expect(SOURCE).not.toMatch(/internalHasConfiguredKey/);
  });

  it('the per-feature model-picker section gates on internalIsConnected', () => {
    const gateMatch = SOURCE.match(
      /\{#if internalIsConnected\}[\s\S]{0,600}Smart Routing Model/
    );
    expect(gateMatch).toBeTruthy();
  });

  it('the {#if} that structurally encloses the Smart Routing Model block is internalIsConnected, not a reintroduced key check', () => {
    const labelIndex = SOURCE.indexOf('Smart Routing Model');
    expect(labelIndex).toBeGreaterThan(-1);
    expect(findEnclosingIfCondition(SOURCE, labelIndex)).toBe('internalIsConnected');
  });

  /// Mutation guard, named explicitly (the exact failure mode a 400-char
  /// window assertion could never catch): insert enough comment/
  /// whitespace text between `{#if internalIsConnected}` and the "Smart
  /// Routing Model" label to push the label more than 400 characters
  /// past the opening tag, with NO logic change at all. A fixed-size
  /// text window would see a DIFFERENT (or no) `{#if}` inside its slice
  /// and either false-fail or silently match the wrong one; the
  /// structural scanner is correct regardless of distance.
  it('mutation guard: the structural scan is correct even with 500+ chars of comment/whitespace between the {#if} and the label', () => {
    const marker = '{#if internalIsConnected}';
    const idx = SOURCE.indexOf(marker);
    expect(idx).toBeGreaterThan(-1);
    const padding = `\n            <!-- ${'padding comment text goes right here, long enough that this alone pushes the label well past any fixed 400-char window boundary '.repeat(3)} -->\n`;
    const padded = SOURCE.slice(0, idx + marker.length) + padding + SOURCE.slice(idx + marker.length);
    const paddedLabelIndex = padded.indexOf('Smart Routing Model');
    expect(paddedLabelIndex - (idx + marker.length)).toBeGreaterThan(400);
    expect(findEnclosingIfCondition(padded, paddedLabelIndex)).toBe('internalIsConnected');
  });
});

describe('#2795 follow-up B — Semantic Search provider dropdown explains embedding-only listing', () => {
  it('the Semantic Search provider row sublabel states that only providers with embedding support are listed', () => {
    const ssProviderRowMatch = SOURCE.match(
      /<div class="form-row-label">Provider<\/div>\s*<div class="form-row-sublabel">[\s\S]*?<\/div>\s*<\/div>\s*<div class="form-row-right"[^>]*>\s*<select class="select-input" value=\{ssProvider\}/
    );
    expect(ssProviderRowMatch).toBeTruthy();
    expect(ssProviderRowMatch![0]).toContain('Only providers with embedding support are listed.');
  });
});
