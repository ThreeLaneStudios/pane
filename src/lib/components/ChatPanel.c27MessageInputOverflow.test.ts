/**
 * ChatPanel.c27MessageInputOverflow.test.ts — C27, third diagnosis (#3147,
 * live test #5).
 *
 * SOURCE-TEXT PIN ONLY — PROVES NOTHING ABOUT THE REAL SYMPTOM.
 *
 * Reed's audit confirmed `.message-input`'s rule sets `overflow-y: auto`
 * but never declares `overflow-x` at all — per the CSS Overflow Module's
 * own coupling rule (Level 3 §3), a box with exactly one of
 * `overflow-x`/`overflow-y` set to a non-`visible` value computes the
 * OTHER axis to `auto` too, instead of staying at its apparent default.
 * So `overflow-x` was silently `auto` here, never declared, never
 * reviewed. Setting `overflow-x: hidden` explicitly removes the entire
 * CLASS of horizontal-scrollbar-on-this-element failures.
 *
 * This is the THIRD C27 theory shipped this scope (after #3144's
 * viewport-units and #3145's `display: contents` attempts — both of
 * which targeted `.app`, never this element, and neither of which was
 * ever the actual cause of the composer scrollbar Matt saw). Reed tried
 * two different Playwright proxies standing in for native WKWebView
 * `pageZoom` (CSS `zoom` on `<body>`, and Chromium's fractional
 * `deviceScaleFactor` override) and could NOT reproduce the ~1px
 * discrepancy with either. The fix below is mechanism-agnostic — it
 * closes the "overflow-x silently computes to auto" class of bug
 * regardless of what triggers it — not mechanism-confirmed.
 *
 * A jsdom source-text pin has no real layout engine and cannot catch a
 * real-WKWebView scrollbar regression by construction. A green result
 * below means the literal `overflow-x: hidden` text is present in
 * `.message-input`'s rule, and nothing more. It is not evidence the
 * composer never shows a stray horizontal scrollbar on real WKWebView at
 * any zoom level. The only thing that could confirm that is Matt's own
 * live re-walk at Cmd+ zoom (`4d-skip: Matt`).
 *
 * Mutation: removing `overflow-x: hidden` from `.message-input`'s rule
 * reddens — the literal is gone. Proves the literal changed; proves
 * nothing about whether a stray scrollbar actually disappears on real
 * WKWebView.
 */

import { describe, it, expect } from 'vitest';
import { readFileSync } from 'fs';
import { resolve } from 'path';

const source = readFileSync(resolve(__dirname, 'ChatPanel.svelte'), 'utf-8');

function extractMessageInputRule(): string {
  const ruleStart = source.indexOf('.message-input {');
  expect(ruleStart).toBeGreaterThan(-1);
  const ruleEnd = source.indexOf('\n  }', ruleStart);
  const raw = source.slice(ruleStart, ruleEnd);
  // Strip the block comment (this rule carries an explanatory doc
  // comment that itself quotes the literal "overflow-x: hidden" in
  // prose) so the assertions below check the actual CSS declaration,
  // not a prose mention of it — otherwise a mutation that reverted the
  // real declaration while leaving the comment's own quoted text in
  // place would NOT redden, defeating the point of the pin.
  return raw.replace(/\/\*[\s\S]*?\*\//g, '');
}

describe('C27 — .message-input sets overflow-x: hidden explicitly', () => {
  it('the .message-input rule contains overflow-x: hidden', () => {
    const rule = extractMessageInputRule();
    expect(rule).toMatch(/overflow-x:\s*hidden/);
  });

  it('the .message-input rule still sets overflow-y: auto (unchanged by this fix)', () => {
    const rule = extractMessageInputRule();
    expect(rule).toMatch(/overflow-y:\s*auto/);
  });
});
