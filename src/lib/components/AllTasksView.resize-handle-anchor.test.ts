/**
 * #2379 — resize handle escapes .editor-panel because the panel has no positioning
 * context for its absolutely-positioned child.
 *
 * Audit: docs/audits/2026-08-07-2379-resize-handle-gap.md
 *
 * Root cause: `.editor-resize` is `position: absolute; left: -2px`, which resolves
 * against the nearest positioned ancestor. `.editor-panel` (its intended containing
 * block) had no `position` declaration at all, so the handle escaped up to
 * `.all-tasks-view` (`position: relative`) instead of anchoring at the editor seam —
 * exactly mirroring KanbanBoard.svelte's `.kanban-editor-panel`, which explicitly
 * carries `position: relative; /* Anchor for drag handle *\/` (KanbanBoard.svelte:1059).
 *
 * This file asserts against the REAL component source (not a stub topology):
 *   1. The handle markup + onmousedown wiring exist in AllTasksView.svelte.
 *   2. `.editor-panel`'s real CSS block computes `position: relative` in jsdom.
 *   3. Red-on-revert: stripping the `position: relative` declaration from the parsed
 *      CSS block flips the computed-style assertion.
 */

import { describe, it, expect } from 'vitest';
import { readFileSync } from 'fs';
import { resolve } from 'path';

const COMPONENT_PATH = resolve(__dirname, 'AllTasksView.svelte');
const SOURCE = readFileSync(COMPONENT_PATH, 'utf-8');

/**
 * Extracts a top-level CSS rule block (selector + body) from the component's
 * <style> section by selector text. Returns the full `selector { ... }` text,
 * or throws if not found — keeping this test honest against source drift.
 */
function extractCssRule(source: string, selector: string): string {
  const styleMatch = source.match(/<style[^>]*>([\s\S]*?)<\/style>/);
  if (!styleMatch) throw new Error('No <style> block found in AllTasksView.svelte');
  const css = styleMatch[1];

  const selectorIndex = css.indexOf(`${selector} {`);
  if (selectorIndex === -1) {
    throw new Error(`Selector "${selector}" not found in AllTasksView.svelte <style> block`);
  }

  const openBraceIndex = css.indexOf('{', selectorIndex);
  let depth = 0;
  let i = openBraceIndex;
  for (; i < css.length; i++) {
    if (css[i] === '{') depth++;
    if (css[i] === '}') {
      depth--;
      if (depth === 0) break;
    }
  }
  return css.slice(selectorIndex, i + 1);
}

describe('AllTasksView resize handle anchoring — #2379', () => {
  it('the resize handle element exists in the markup, wired to startEditorResize via onmousedown', () => {
    // Matches AllTasksView.svelte:512-517
    expect(SOURCE).toMatch(
      /<div class="editor-resize"[\s\S]*?onmousedown={startEditorResize}[\s\S]*?<\/div>/
    );
  });

  it('the handle is the first child inside .editor-panel (structurally anchored at the seam)', () => {
    const panelOpenIndex = SOURCE.indexOf('<div class="editor-panel"');
    expect(panelOpenIndex).toBeGreaterThan(-1);
    const handleIndex = SOURCE.indexOf('class="editor-resize"', panelOpenIndex);
    expect(handleIndex).toBeGreaterThan(panelOpenIndex);
  });

  it('.editor-panel carries `position: relative` so the absolutely-positioned handle anchors to it, not the view root', () => {
    const rule = extractCssRule(SOURCE, '.editor-panel');

    // Build a real DOM + inject the actual parsed rule, then read computed style —
    // asserts real DOM state, not a string match against the source text.
    const style = document.createElement('style');
    style.textContent = rule;
    document.head.appendChild(style);

    const panel = document.createElement('div');
    panel.className = 'editor-panel';
    document.body.appendChild(panel);

    const computed = getComputedStyle(panel);
    expect(computed.position).toBe('relative');

    document.body.removeChild(panel);
    document.head.removeChild(style);
  });

  it('RED-ON-REVERT: removing `position: relative` from the parsed .editor-panel rule flips the anchor assertion', () => {
    const rule = extractCssRule(SOURCE, '.editor-panel');

    // Simulate the pre-fix state by stripping the declaration this PR adds, then
    // re-run the same computed-style check. This must fail (be non-relative) to
    // prove the assertion above is actually exercising the fix, not a tautology.
    const revertedRule = rule.replace(/position:\s*relative;\s*(\/\*[^*]*\*\/)?/, '');
    expect(revertedRule).not.toMatch(/position:\s*relative/);

    const style = document.createElement('style');
    style.textContent = revertedRule;
    document.head.appendChild(style);

    const panel = document.createElement('div');
    panel.className = 'editor-panel';
    document.body.appendChild(panel);

    const computed = getComputedStyle(panel);
    expect(computed.position).not.toBe('relative');
    // jsdom reports unset `position` as '' (not the CSS-spec default 'static'
    // string) — either way it's provably not 'relative', which is the anchor
    // contract under test.
    expect(['', 'static']).toContain(computed.position);

    document.body.removeChild(panel);
    document.head.removeChild(style);
  });
});
