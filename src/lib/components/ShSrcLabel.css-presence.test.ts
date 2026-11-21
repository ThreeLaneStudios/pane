/**
 * #2339 PR 15b-beta follow-up \u2014 `.sh-src*` CSS-presence regression test.
 *
 * `ShSrcLabel.svelte` renders `.sh-src` / `.sh-src--sheet` / `.sh-src--agent`
 * / `.sh-src--restore` / `.sh-src__ico` / `.sh-src__n` class names but has no
 * `<style>` block of its own \u2014 the rules were ported into `src/app.css` as
 * globals (same pattern as `.sh-pop`, see that block's doc comment). This
 * test is a source-text-extraction check, matching the house pattern
 * established by `Card.board-width-regression.test.ts`: it reads
 * `src/app.css` as a plain string and asserts each `.sh-src*` selector is
 * actually present in shipped styles.
 *
 * Without this test, a component could reference class names with no
 * backing CSS anywhere and still pass green \u2014 exactly the bug this PR
 * fixes. This guards against that regressing.
 */

import { describe, it, expect } from 'vitest';
import { readFileSync } from 'fs';
import { resolve } from 'path';

const APP_CSS_PATH = resolve(__dirname, '../../app.css');
const APP_CSS = readFileSync(APP_CSS_PATH, 'utf-8');

function extractCssRule(css: string, selector: string): string {
  const selectorIndex = css.indexOf(`${selector} {`);
  if (selectorIndex === -1) {
    throw new Error(`Selector "${selector}" not found`);
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

describe('#2339 \u2014 .sh-src* CSS is actually shipped in src/app.css (ShSrcLabel has no <style> of its own)', () => {
  it('.sh-src base rule exists', () => {
    const rule = extractCssRule(APP_CSS, '.sh-src');
    expect(rule).toMatch(/display:\s*inline-flex/);
    expect(rule).toMatch(/background:\s*var\(--surface-2\)/);
  });

  it('.sh-src__ico rule exists (the glyph-span substitution for the design\'s `svg` descendant selector)', () => {
    const rule = extractCssRule(APP_CSS, '.sh-src__ico');
    expect(rule).toMatch(/flex-shrink:\s*0/);
  });

  it('the design\'s original `.sh-src svg` selector is NOT what shipped \u2014 ShSrcLabel renders a span, not an svg', () => {
    expect(APP_CSS).not.toMatch(/\.sh-src\s+svg\s*\{/);
  });

  it('.sh-src--sheet variant exists', () => {
    const rule = extractCssRule(APP_CSS, '.sh-src--sheet');
    expect(rule).toMatch(/background:\s*var\(--accent-soft\)/);
    expect(rule).toMatch(/color:\s*var\(--accent\)/);
  });

  it('.sh-src--agent variant exists', () => {
    const rule = extractCssRule(APP_CSS, '.sh-src--agent');
    expect(rule).toMatch(/background:\s*rgba\(139,\s*92,\s*246,\s*0\.14\)/);
    expect(rule).toMatch(/color:\s*#a78bfa/);
  });

  it('.sh-src--restore variant exists', () => {
    const rule = extractCssRule(APP_CSS, '.sh-src--restore');
    expect(rule).toMatch(/background:\s*rgba\(245,\s*158,\s*11,\s*0\.14\)/);
    expect(rule).toMatch(/color:\s*var\(--status-warning\)/);
  });

  it('.sh-src__n rule exists', () => {
    const rule = extractCssRule(APP_CSS, '.sh-src__n');
    expect(rule).toMatch(/font-family:\s*var\(--font-mono\)/);
    expect(rule).toMatch(/opacity:\s*0\.8/);
  });
});
