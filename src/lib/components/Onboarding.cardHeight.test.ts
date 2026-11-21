/**
 * Onboarding.cardHeight.test.ts — regression coverage for #2792.
 *
 * Two independent bugs, one fix:
 *
 * 1. Provider cards (both "Connect Your AI" §6.2 and "Connect Smart Features"
 *    §6.4.1) rendered at a variable, oversized height (~100px) because
 *    Onboarding.svelte's single <style> block contained TWO conflicting
 *    `.provider-card` rule sets. The first (intended) rule used
 *    `flex-wrap` row layout with no fixed height; a second, later-in-source
 *    rule set — a leftover merge duplicate documented at Onboarding.svelte's
 *    `.connect-field` selector near the removed block — forced
 *    `flex-direction: column` and no height cap, and won the cascade purely
 *    by being later in source order (same specificity, same selector). That
 *    second block also carried a dead `.provider-card.selected` selector
 *    that no markup in this file ever applies (`class:active` is the only
 *    state class provider-card buttons use).
 *
 * 2. The "Connect Smart Features" cards (screen === 'smart-features-picker')
 *    never rendered a `.provider-desc` span at all — only `.provider-name` —
 *    unlike the "Connect Your AI" cards, which have always had descriptors.
 *
 * This suite asserts against the REAL component source (not a stub
 * topology), same pattern as AllTasksView.resize-handle-anchor.test.ts:
 *   A. Exactly one `.provider-card {` rule definition exists in the
 *      stylesheet (guards against the duplicate-block regression coming
 *      back) — RED-ON-REVERT by re-adding the dead second block.
 *   B. The single `.provider-card` rule computes a fixed `height` in jsdom,
 *      identical regardless of how much text an instance holds.
 *   C. Every Smart Features provider button (OpenRouter/Anthropic/Ollama)
 *      renders a `.provider-desc` span with the same descriptor text as the
 *      Connect Your AI card for that same provider.
 */
import { describe, it, expect } from 'vitest';
import { readFileSync } from 'fs';
import { resolve } from 'path';

const COMPONENT_PATH = resolve(__dirname, 'Onboarding.svelte');
const SOURCE = readFileSync(COMPONENT_PATH, 'utf-8');

/**
 * Extracts a top-level CSS rule block (selector + body) from the component's
 * <style> section by selector text. Returns the full `selector { ... }` text
 * for the FIRST match, or throws if not found — keeping this test honest
 * against source drift. Mirrors AllTasksView.resize-handle-anchor.test.ts.
 */
function extractCssRule(source: string, selector: string): string {
  const styleMatch = source.match(/<style[^>]*>([\s\S]*?)<\/style>/);
  if (!styleMatch) throw new Error('No <style> block found in Onboarding.svelte');
  const css = styleMatch[1];

  const selectorIndex = css.indexOf(`${selector} {`);
  if (selectorIndex === -1) {
    throw new Error(`Selector "${selector}" not found in Onboarding.svelte <style> block`);
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

/** Counts every occurrence of an exact top-level selector definition
 *  (e.g. `.provider-card {`) within the <style> block, including ones
 *  buried behind pseudo-classes like `.provider-card:hover {` would NOT
 *  match here — this only counts the bare selector, which is what a
 *  reintroduced duplicate block would add another instance of. */
function countCssRuleDefinitions(source: string, selector: string): number {
  const styleMatch = source.match(/<style[^>]*>([\s\S]*?)<\/style>/);
  if (!styleMatch) throw new Error('No <style> block found in Onboarding.svelte');
  const css = styleMatch[1];
  const re = new RegExp(`(^|\\s)${selector.replace(/\./g, '\\.')} \\{`, 'g');
  return (css.match(re) || []).length;
}

describe('A — no duplicate .provider-card rule (root cause of #2792 oversized cards)', () => {
  it('the stylesheet defines .provider-card exactly once', () => {
    // Pre-fix, this was 2 — a second `.provider-card { flex-direction: column;
    // ... }` block later in source order silently won the cascade over the
    // first, forcing full-width rows with no height cap.
    expect(countCssRuleDefinitions(SOURCE, '.provider-card')).toBe(1);
  });

  it('RED-ON-REVERT: reintroducing the dead duplicate block breaks the single-definition invariant', () => {
    const reintroduced = SOURCE.replace(
      '.provider-card:hover { border-color: #3F3F46; }\n  .provider-card.active { border-color: #00E4E4; background: rgba(0,228,228,0.05); }',
      '.provider-card:hover { border-color: #3F3F46; }\n  .provider-card.active { border-color: #00E4E4; background: rgba(0,228,228,0.05); }\n  .provider-card { flex-direction: column; }'
    );
    expect(reintroduced).not.toBe(SOURCE);
    expect(countCssRuleDefinitions(reintroduced, '.provider-card')).toBe(2);
  });

  it('no CSS rule or markup instance applies the dead `.provider-card.selected` state class (it must stay removed, not just unused)', () => {
    const styleMatch = SOURCE.match(/<style[^>]*>([\s\S]*?)<\/style>/);
    expect(styleMatch).toBeTruthy();
    expect(styleMatch![1]).not.toContain('.provider-card.selected');
    expect(SOURCE).not.toContain('class="provider-card selected');
    expect(SOURCE).not.toContain("class:selected");
  });
});

describe('B — .provider-card computes a fixed, uniform height', () => {
  it('the .provider-card rule declares an explicit pixel `height` (not left to content/padding to determine box size)', () => {
    const rule = extractCssRule(SOURCE, '.provider-card');
    expect(rule).toMatch(/height:\s*\d+px/);
  });

  it('.provider-name and .provider-desc both truncate with an ellipsis instead of wrapping — the mechanism that keeps every card the same height regardless of text length', () => {
    const nameRule = extractCssRule(SOURCE, '.provider-name');
    const descRule = extractCssRule(SOURCE, '.provider-desc');
    for (const rule of [nameRule, descRule]) {
      expect(rule).toMatch(/white-space:\s*nowrap/);
      expect(rule).toMatch(/overflow:\s*hidden/);
      expect(rule).toMatch(/text-overflow:\s*ellipsis/);
    }
  });

  it('a short-text card and a long-text card compute the SAME explicit height in jsdom (static declaration, not layout-dependent)', () => {
    const rule = extractCssRule(SOURCE, '.provider-card');
    const nameRule = extractCssRule(SOURCE, '.provider-name');
    const descRule = extractCssRule(SOURCE, '.provider-desc');

    const style = document.createElement('style');
    style.textContent = `${rule}\n${nameRule}\n${descRule}`;
    document.head.appendChild(style);

    const short = document.createElement('button');
    short.className = 'provider-card';
    short.innerHTML = '<span class="provider-name">Ollama</span><span class="provider-desc">Local models</span>';
    document.body.appendChild(short);

    const long = document.createElement('button');
    long.className = 'provider-card';
    long.innerHTML = '<span class="provider-name">OpenRouter</span><span class="provider-desc">Access a large catalog of many different hosted models across many different providers</span>';
    document.body.appendChild(long);

    const shortHeight = getComputedStyle(short).height;
    const longHeight = getComputedStyle(long).height;

    expect(shortHeight).toBe(longHeight);
    expect(shortHeight).toBe('52px');

    document.body.removeChild(short);
    document.body.removeChild(long);
    document.head.removeChild(style);
  });

  it('RED-ON-REVERT: removing the `height` declaration means the card no longer computes a fixed pixel height at all', () => {
    const rule = extractCssRule(SOURCE, '.provider-card');
    const revertedRule = rule.replace(/height:\s*\d+px;\s*/, '');
    expect(revertedRule).not.toContain('height:');

    const style = document.createElement('style');
    style.textContent = revertedRule;
    document.head.appendChild(style);

    const card = document.createElement('button');
    card.className = 'provider-card';
    document.body.appendChild(card);

    const computedHeight = getComputedStyle(card).height;
    expect(computedHeight).not.toBe('52px');

    document.body.removeChild(card);
    document.head.removeChild(style);
  });
});

describe('C — Connect Smart Features cards carry the same descriptors as Connect Your AI', () => {
  const smartFeaturesBlockMatch = SOURCE.match(
    /screen === 'smart-features-picker'[\s\S]*?<div class="skip-row"/
  );

  it('the smart-features-picker markup block exists', () => {
    expect(smartFeaturesBlockMatch).toBeTruthy();
  });

  const block = smartFeaturesBlockMatch ? smartFeaturesBlockMatch[0] : '';

  it.each([
    ['OpenRouter', 'Access multiple models'],
    ['Anthropic', 'Claude direct'],
    ['Ollama', 'Local models'],
  ])('%s card renders provider-desc "%s"', (name, desc) => {
    // Find this provider's <button class="provider-card" ...>...</button>
    // within the smart-features-picker block and assert its provider-desc
    // span text.
    const nameIndex = block.indexOf(`<span class="provider-name">${name}</span>`);
    expect(nameIndex).toBeGreaterThan(-1);
    const afterName = block.slice(nameIndex, nameIndex + 200);
    expect(afterName).toContain(`<span class="provider-desc">${desc}</span>`);
  });

  it('RED-ON-REVERT: a smart-features card without provider-desc fails the descriptor check', () => {
    const strippedBlock = block.replace(
      '<span class="provider-name">Ollama</span>\n          <span class="provider-desc">Local models</span>',
      '<span class="provider-name">Ollama</span>'
    );
    expect(strippedBlock).not.toBe(block);
    const nameIndex = strippedBlock.indexOf('<span class="provider-name">Ollama</span>');
    const afterName = strippedBlock.slice(nameIndex, nameIndex + 200);
    expect(afterName).not.toContain('provider-desc');
  });

  it('the Connect Your AI (§6.2) cards use the SAME descriptor text for these three providers, keeping the two pickers in sync', () => {
    // Connect Your AI's descriptors are the source of truth this ruling
    // copies from — assert they still match so the two pickers can't drift
    // apart silently in a future edit.
    const connectAiBlockMatch = SOURCE.match(
      /screen === 'connect'[\s\S]*?class="connect-providers"[\s\S]*?<\/div>\s*<\/div>/
    );
    expect(connectAiBlockMatch).toBeTruthy();
    const connectAiBlock = connectAiBlockMatch![0];

    for (const [name, desc] of [
      ['OpenRouter', 'Access multiple models'],
      ['Anthropic', 'Claude direct'],
      ['Ollama', 'Local models'],
    ]) {
      const nameIndex = connectAiBlock.indexOf(`<span class="provider-name">${name}</span>`);
      expect(nameIndex).toBeGreaterThan(-1);
      const afterName = connectAiBlock.slice(nameIndex, nameIndex + 200);
      expect(afterName).toContain(`<span class="provider-desc">${desc}</span>`);
    }
  });
});
