/**
 * ChatSidebar.errorCaseNoScreenChange.test.ts — #3043 item 1
 *
 * The task's exact requirement: "an unknown view shows the error banner
 * AND the screen doesn't change. Assert both — a test checking only for
 * the error would pass while the view still shifted."
 *
 * The two existing halves of this proof:
 * - `navigationDispatch.test.ts` proves `decideNavigationAction` returns
 *   `{ kind: 'error' }` for a backend-rejected/unrecognized view (the
 *   DECISION is correct).
 * - `ChatPanel.navError.test.ts` mounts the REAL `ChatPanel.svelte` and
 *   proves a `navError` string renders as visible DOM text (the BANNER
 *   is correct).
 *
 * Neither proves the "screen doesn't change" half. `ChatSidebar.svelte`
 * is a large, API/session-integrated component with no existing
 * component-render test harness (same reason `ChatSidebar.onDone.test.ts`
 * gives for testing its logic as an extracted function rather than
 * mounting the whole component) — and `handleNavigateFromStream` isn't
 * exported. So this test reads the REAL source of its `case 'error':`
 * branch (the one the real component calls, not a hand-copy — the #2995
 * lesson) and asserts, structurally, that it:
 *   1. sets the visible `navError` state (the banner's data source), AND
 *   2. `return`s BEFORE reaching any `navigateTo(`/`canvasState` call —
 * i.e. the branch that shows the banner is, by construction, a dead end
 * for navigation. This is the SAME "extract the real literal text, then
 * assert on its structure" discipline `canvas.showViewParity.test.ts`
 * already applies to array literals, applied here to a control-flow
 * block instead.
 */
import { describe, it, expect } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

const SRC_PATH = resolve(__dirname, 'ChatSidebar.svelte');
const source = readFileSync(SRC_PATH, 'utf-8');

/** Extracts the literal text of `handleNavigateFromStream`'s `case 'error': { ... }` block — the real switch arm, not a restated copy. */
function extractErrorCaseBlock(src: string): string {
  const fnMatch = src.match(/async function handleNavigateFromStream\([\s\S]*?\n {2}\}\n/);
  if (!fnMatch) {
    throw new Error('Could not find handleNavigateFromStream in ChatSidebar.svelte — source structure changed.');
  }
  const fnBody = fnMatch[0];
  const caseMatch = fnBody.match(/case 'error':\s*\{([\s\S]*?)\n\s*\}/);
  if (!caseMatch) {
    throw new Error("Could not find case 'error' inside handleNavigateFromStream — source structure changed.");
  }
  return caseMatch[1];
}

describe("#3043 item 1 — handleNavigateFromStream's real case 'error' branch shows the banner AND never changes the screen", () => {
  const errorBlock = extractErrorCaseBlock(source);

  it('sets navError — the exact state ChatPanel.navError.test.ts proves renders as a visible banner', () => {
    expect(errorBlock).toMatch(/navError\s*=\s*decision\.message/);
  });

  it("returns — doesn't fall through to any later case or to code that could still navigate", () => {
    expect(errorBlock.trim()).toMatch(/return;\s*$/);
  });

  it('contains NO navigateTo(...) call — the screen cannot change on this path', () => {
    expect(errorBlock).not.toMatch(/navigateTo\(/);
  });

  it('contains NO canvasState mutation (.set(/.update(/direct assignment) — nothing touches the canvas on this path', () => {
    expect(errorBlock).not.toMatch(/canvasState/);
  });

  it('sanity: the extracted block is non-trivial (a regex that silently matched an empty/wrong span must fail loudly, not pass vacuously)', () => {
    expect(errorBlock.trim().length).toBeGreaterThan(10);
  });
});
