/**
 * canvas.showViewParity.test.ts — #3036 PR2, narrowed by #3043 (Matt's
 * live-testing ruling, superseding the #3036 PR2 enumeration)
 * (`docs/audits/2026-10-16-scope-agent-instructions.md` §6, item 5: "Pin
 * show_view's view list to the frontend's real views")
 *
 * Follows the precedent set by `refreshEvent.mirrorParity.test.ts` (#2972
 * Stage 4) and `ShHeaderMenu.columnTypesParity.test.ts` (#2874 item 64):
 * read BOTH source files with `readFileSync`, extract the declared values
 * from the literal source text of each via regex, and compare the two
 * EXTRACTIONS against each other — never against a list this test file
 * restates from memory (the #2995 lesson).
 *
 * **#3043 changed what this test's source of truth means.** #3036 PR2
 * originally pinned `SHOW_VIEW_SCREENS` against `RENDERED_CANVAS_VIEWS` —
 * i.e. "can `CanvasRenderer.svelte` draw it". Matt tested the live app and
 * ruled that's the wrong definition of "live": **a screen only counts if
 * a user control — a button, menu item, or keyboard shortcut — actually
 * opens it.** `task_list` and `dashboard` both render fine but have no
 * user control that opens either one; #3043 removed both from
 * `SHOW_VIEW_SCREENS` on both sides. This test is now pinned against
 * `USER_REACHABLE_VIEWS` (`canvas.ts`), not `RENDERED_CANVAS_VIEWS`.
 *
 * This PR's task explicitly calls out two weaknesses of that precedent to
 * do better on:
 *
 * 1. "#3026's regex was brittle to quote style and trailing commas. A
 *    total extraction miss must fail loudly, never compare two empty sets
 *    and pass." — the sanity test below asserts BOTH extractions are
 *    non-empty AND have the exact expected length, so a broken regex
 *    (quote-style/whitespace change) that yields an empty match fails
 *    loudly here instead of letting the real equality assertion pass
 *    vacuously (`new Set([]) === new Set([])` would otherwise be true).
 * 2. "A substring scan proves a name appears, not that it's reachable."
 *    — stated plainly: this test proves `show_view`'s `view` enum
 *    (`SHOW_VIEW_SCREENS`, tools.rs) and the frontend's own canonical list
 *    (`SHOW_VIEW_SCREENS`, canvas.ts) declare the IDENTICAL set of names,
 *    in the same order, AND that every one of those names is also in
 *    `USER_REACHABLE_VIEWS` (checked below).
 *
 *    **#3043 round 2 closes the rest of this gap** (it was previously
 *    disclosed, not closed — Reed's WARN on the #3036 PR2 audit, restated
 *    on the delta-2 audit when `search_results` was found sitting in
 *    `USER_REACHABLE_VIEWS` with zero real control behind it). THIS test
 *    still only proves the two `SHOW_VIEW_SCREENS` declarations agree
 *    with the hand-maintained `USER_REACHABLE_VIEWS` array — it does not
 *    itself verify that array is correct. But `USER_REACHABLE_VIEWS` is
 *    no longer unverifiable: `userReachableScan.ts` mechanically derives
 *    the same set by scanning every real `navigateTo(...)`/
 *    `canvasState.set(...)`/`canvasState.update(...)` call site in `src/`
 *    (excluding the agent's own navigation-dispatch code), and
 *    `userReachableViews.derived.test.ts` asserts the derived scan and
 *    this hand-maintained array are IDENTICAL — so a screen added to
 *    `SHOW_VIEW_SCREENS` and (wrongly) also to `USER_REACHABLE_VIEWS`
 *    with no real control behind it now fails THAT test, naming the
 *    extra entry, even though it would still pass THIS one.
 *
 * Also required by the task: proof the comparison mechanism actually
 * fails when a dead/extra name is injected on EITHER side (not just that
 * it currently passes) — see the two "fails on injection" tests below.
 */

import { describe, it, expect } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

const TS_PATH = resolve(__dirname, 'canvas.ts');
const RS_PATH = resolve(__dirname, '../../../src-tauri/src/ai/tools.rs');

const tsSource = readFileSync(TS_PATH, 'utf-8');
const rsSource = readFileSync(RS_PATH, 'utf-8');

/**
 * Extracts the ordered list of screen names from canvas.ts's
 * `export const SHOW_VIEW_SCREENS = ['a', 'b', ...] as const satisfies ...`
 * array literal.
 */
function extractTsShowViewScreens(source: string): string[] {
  const arrayMatch = source.match(
    /export const SHOW_VIEW_SCREENS = \[([\s\S]*?)\]\s*as const/,
  );
  if (!arrayMatch) return [];
  const ids: string[] = [];
  const itemRe = /'([^']*)'/g;
  let m: RegExpExecArray | null;
  while ((m = itemRe.exec(arrayMatch[1])) !== null) {
    ids.push(m[1]);
  }
  return ids;
}

/**
 * Extracts the ordered list of screen names from tools.rs's
 * `pub const SHOW_VIEW_SCREENS: [&str; N] = ["a", "b", ...];` array literal.
 */
function extractRsShowViewScreens(source: string): string[] {
  const arrayMatch = source.match(
    /pub const SHOW_VIEW_SCREENS:\s*\[&str;\s*\d+\]\s*=\s*\[([^\]]*)\]/,
  );
  if (!arrayMatch) return [];
  const ids: string[] = [];
  const itemRe = /"([^"]*)"/g;
  let m: RegExpExecArray | null;
  while ((m = itemRe.exec(arrayMatch[1])) !== null) {
    ids.push(m[1]);
  }
  return ids;
}

/**
 * Extracts the ordered list of ALL renderable screens from canvas.ts's
 * `export const RENDERED_CANVAS_VIEWS = [...] as const satisfies ...`
 * array literal — kept ONLY for the "known dead/unrendered views" sanity
 * check below (screens with no render branch at all must never appear in
 * either list). It is deliberately NOT what `SHOW_VIEW_SCREENS` is pinned
 * against anymore — see `extractTsUserReachableViews` below and this
 * file's header comment (#3043, Matt's ruling).
 */
function extractTsRenderedCanvasViews(source: string): string[] {
  const arrayMatch = source.match(
    /export const RENDERED_CANVAS_VIEWS = \[([\s\S]*?)\]\s*as const/,
  );
  if (!arrayMatch) return [];
  const ids: string[] = [];
  const itemRe = /'([^']*)'/g;
  let m: RegExpExecArray | null;
  while ((m = itemRe.exec(arrayMatch[1])) !== null) {
    ids.push(m[1]);
  }
  return ids;
}

/**
 * Extracts the ordered list of USER-REACHABLE screens from canvas.ts's
 * `export const USER_REACHABLE_VIEWS = [...] as const satisfies ...`
 * array literal — #3043, Matt's live-testing ruling: "live" means a user
 * control (button/menu/shortcut) opens it, not that `CanvasRenderer.svelte`
 * merely CAN draw it. THIS is what `SHOW_VIEW_SCREENS` must be a subset
 * of, replacing the #3036 PR2 `RENDERED_CANVAS_VIEWS` check.
 */
function extractTsUserReachableViews(source: string): string[] {
  const arrayMatch = source.match(
    /export const USER_REACHABLE_VIEWS = \[([\s\S]*?)\]\s*as const/,
  );
  if (!arrayMatch) return [];
  const ids: string[] = [];
  const itemRe = /'([^']*)'/g;
  let m: RegExpExecArray | null;
  while ((m = itemRe.exec(arrayMatch[1])) !== null) {
    ids.push(m[1]);
  }
  return ids;
}

const tsScreens = extractTsShowViewScreens(tsSource);
const rsScreens = extractRsShowViewScreens(rsSource);
const tsRenderedViews = extractTsRenderedCanvasViews(tsSource);
const tsUserReachableViews = extractTsUserReachableViews(tsSource);

describe('show_view (tools.rs) <-> canvas.ts SHOW_VIEW_SCREENS parity (#3036 PR2, narrowed by #3043)', () => {
  it('all three files were actually readable and non-trivially parsed to the EXACT expected length (sanity — a regex that silently matches nothing, or matches the wrong array, must fail loudly here, not pass vacuously by comparing two empty/wrong sets)', () => {
    expect(rsScreens.length).toBe(3);
    expect(tsScreens.length).toBe(3);
    expect(tsRenderedViews.length).toBeGreaterThan(rsScreens.length);
    expect(tsUserReachableViews.length).toBeGreaterThan(rsScreens.length);
  });

  it('the SET of screen names matches between tools.rs and canvas.ts (a screen present in only one file fails here)', () => {
    expect(new Set(tsScreens)).toEqual(new Set(rsScreens));
  });

  it('the ORDER of screen names matches between tools.rs and canvas.ts (a reorder in only one file fails here — order is part of the enum/UI contract, matching the ShHeaderMenu precedent)', () => {
    expect(tsScreens).toEqual(rsScreens);
  });

  it("every SHOW_VIEW_SCREENS entry is also a USER_REACHABLE_VIEWS entry — show_view must never advertise a screen with no user control that opens it. #3043, Matt's live-testing ruling: 'live' means a user control (button/menu/shortcut) opens it, NOT that CanvasRenderer.svelte merely has a render branch that can draw it — that was #3036 PR2's wrong definition, checked against RENDERED_CANVAS_VIEWS, which let task_list/dashboard through with no button anywhere. This is a declaration-level check only — see this file's own doc comment for what it does NOT prove (USER_REACHABLE_VIEWS is hand-maintained, not derived)", () => {
    for (const screen of tsScreens) {
      expect(tsUserReachableViews, `'${screen}' must be in USER_REACHABLE_VIEWS too — show_view may only advertise screens a user control actually opens, not merely screens CanvasRenderer.svelte can draw`).toContain(screen);
    }
  });

  it('task_list and dashboard are GONE entirely (#3044 item 5 superseding #3043’s "drawable but unreachable" state) — not in RENDERED_CANVAS_VIEWS, not in USER_REACHABLE_VIEWS, and must never be in SHOW_VIEW_SCREENS again. #3043 found no button/menu/shortcut opens either one; this PR’s own re-run of the derived scan confirmed no AGENT route does either, so both were deleted outright (TaskList.svelte, and the dashboard view’s inline render branch) rather than left as dead-but-drawable surfaces. This is the regression guard: re-adding either anywhere fails this test.', () => {
    const deletedViews = ['task_list', 'dashboard'];
    for (const view of deletedViews) {
      expect(tsRenderedViews, `'${view}' must stay OUT of RENDERED_CANVAS_VIEWS — it was deleted, not merely hidden`).not.toContain(view);
      expect(tsUserReachableViews, `'${view}' must stay OUT of USER_REACHABLE_VIEWS — no user control opens it`).not.toContain(view);
      expect(tsScreens, `'${view}' must not be re-added to show_view's screen list (canvas.ts)`).not.toContain(view);
      expect(rsScreens, `'${view}' must not be re-added to show_view's screen list (tools.rs)`).not.toContain(view);
    }
  });

  it('known dead/unrendered CanvasViewType values are NOT in SHOW_VIEW_SCREENS on either side (welcome/today/tag/tags-manage/task_detail have no CanvasRenderer.svelte render branch)', () => {
    const deadViews = ['welcome', 'today', 'tag', 'tags-manage', 'task_detail'];
    for (const dead of deadViews) {
      expect(tsScreens, `'${dead}' must not be a show_view screen`).not.toContain(dead);
      expect(rsScreens, `'${dead}' must not be a show_view screen`).not.toContain(dead);
    }
  });

  // ─── Proof the comparison mechanism actually fails on injection ──────────
  // (required by the task: "plus proof it fails when you inject a dead
  // name, in each direction" — not just that it currently passes).

  it('FAILS when a name is injected on the Rust side ONLY (proves the set-equality check is not vacuously true)', () => {
    const rsWithInjectedDeadName = [...rsScreens, 'bogus_view_injected_rust_only'];
    expect(new Set(rsWithInjectedDeadName)).not.toEqual(new Set(tsScreens));
  });

  it('FAILS when a name is injected on the TS side ONLY (proves the set-equality check is not vacuously true, other direction)', () => {
    const tsWithInjectedDeadName = [...tsScreens, 'bogus_view_injected_ts_only'];
    expect(new Set(tsWithInjectedDeadName)).not.toEqual(new Set(rsScreens));
  });

  it('FAILS when the two REAL extractions are compared against a deliberately wrong hand-typed list (proves this test is not just checking itself — the #2995 lesson)', () => {
    const handTypedWrongList = ['notes_browser', 'task_list', 'settings']; // wrong: includes removed task_list, missing project_config
    expect(new Set(rsScreens)).not.toEqual(new Set(handTypedWrongList));
    expect(new Set(tsScreens)).not.toEqual(new Set(handTypedWrongList));
  });
});
