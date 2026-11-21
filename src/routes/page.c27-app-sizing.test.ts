/**
 * C27 (#3144) — `.app` sizing, second attempt.
 *
 * DRIFT GUARD ONLY — PROVES NOTHING ABOUT LAYOUT.
 *
 * The first version of this file asserted the absence of `100vw`/`100vh`
 * in the `.app` rule — and it stayed green through the entire 1.0.72
 * regression (the shipped `width: 100%; height: 100%` value it required
 * is exactly what broke every session's scroll; no session scrolled at
 * all). A jsdom-based source-text pin has no real layout engine and
 * cannot catch that class of failure by construction — this file never
 * claimed otherwise, but nothing downstream of it ever closed the gap
 * its own docstring disclaimed, so a green run here was mistaken for
 * working layout. Restating it here, loudly, so a future reader doesn't
 * make the same mistake: a green result below means the literal text in
 * `+page.svelte` matches what's expected, and nothing more. It is not
 * evidence that `.app` fills the viewport or that `.messages-area`
 * scrolls on real WKWebView.
 *
 * The only things that actually confirm layout behavior are:
 *   - the local-only e2e test, `e2e/tests/tier1/c27-app-sizing-layout.spec.ts`
 *     (real WKWebView via the tauri-playwright socket bridge — run by
 *     hand, never in CI; see that file's own header for why), and
 *   - Matt's live re-walk of the built app (`4d-skip: Matt`).
 *
 * Mutation: reverting `.app`'s rule back to `position: static` with
 * `width: 100%; height: 100%` (or to `100vw`/`100vh`) reddens — the new
 * literal is gone and the old one is back. Proves the literal changed;
 * proves nothing about whether `.messages-area` actually scrolls.
 */

import { describe, it, expect } from 'vitest';
import { readFileSync } from 'fs';
import { resolve } from 'path';

const pageSource = readFileSync(resolve(__dirname, '+page.svelte'), 'utf-8');

function extractAppRule(): string {
  const appRuleStart = pageSource.indexOf('.app {');
  expect(appRuleStart).toBeGreaterThan(-1);
  const appRuleEnd = pageSource.indexOf('}', appRuleStart);
  return pageSource.slice(appRuleStart, appRuleEnd);
}

describe('C27 — .app is sized via position:fixed;inset:0, not percentage-of-ancestor or viewport units', () => {
  it('the .app rule contains position: fixed and inset: 0', () => {
    const appRule = extractAppRule();

    expect(appRule).toMatch(/position:\s*fixed/);
    expect(appRule).toMatch(/inset:\s*0/);
  });

  it('the .app rule does not contain width/height percentage or viewport-unit sizing', () => {
    const appRule = extractAppRule();

    expect(appRule).not.toContain('100vw');
    expect(appRule).not.toContain('100vh');
    expect(appRule).not.toContain('100dvw');
    expect(appRule).not.toContain('100dvh');
    expect(appRule).not.toMatch(/width:\s*100%/);
    expect(appRule).not.toMatch(/height:\s*100%/);
  });
});
