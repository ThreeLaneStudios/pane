/**
 * WikiLink.empty-state.test.ts — #2419 round-2 Finding 2 Part B (launch-blocking)
 *
 * Per Matt's ruling (2026-08-05): bare `[[` with zero typed characters must
 * render NO popup at all — there is nothing to say "not found" about yet.
 * "No notes found" is only meaningful once the user has typed a query and it
 * matched zero notes.
 *
 * Root cause: `renderWidget`'s empty-items branch (`WikiLink.ts`, previously
 * around line 50) checked ONLY `state.items.length === 0`, which is true both
 * for "just typed [[" (query is empty, items haven't been fetched yet) AND
 * "typed a real query with zero matches" — so both cases rendered the
 * identical "No notes found" message immediately on typing `[[`, which reads
 * as a confusing false negative before the user has even started typing.
 *
 * Fix: `renderWidget` now returns `null` (no popup at all) when
 * `state.items.length === 0 && (!state.query || state.query.length === 0)`.
 * Only when `state.query` is non-empty AND `state.items` is empty does the
 * "No notes found" message render.
 *
 * METHOD: `renderWidget` and `SuggestionState` are exported directly from
 * WikiLink.ts for unit testing (see export additions above `renderWidget`),
 * called with a minimal fake ProseMirror `view` stub (just enough to satisfy
 * `view.coordsAtPos` for widget positioning) and hand-constructed
 * `SuggestionState` objects representing each real transition:
 *   - bare `[[` just typed (active, empty query, empty items)
 *   - query typed, still loading (active, non-empty query, empty items — the
 *     transient state before fetchSuggestions resolves)
 *   - query typed, zero matches after resolution (active, non-empty query,
 *     empty items — post-fetch terminal state)
 *   - query typed, matches found (active, non-empty query, non-empty items)
 *
 * RED-ON-REVERT: removing the `(!state.query || state.query.length === 0)`
 * guard (reverting to the bare `state.items.length === 0` check) makes the
 * "bare [[ shows no popup" test red — it would render a popup with "No notes
 * found" instead of returning null.
 *
 * Scope guard: this PR does not touch QuickSwitcher, api.searchNotes, or
 * notes.rs. searchNotesByTitle's query-construction fix is covered separately
 * in api.searchNotesByTitle.test.ts.
 *
 * Relates to #2419
 * spec-skip: implements Matt's picker-semantics and empty-state rulings 2026-08-05
 */

import { describe, it, expect, beforeAll } from 'vitest';
import { JSDOM } from 'jsdom';
import { renderWidget, type SuggestionState } from './WikiLink';

beforeAll(() => {
  const dom = new JSDOM('<!DOCTYPE html><html><body></body></html>');
  (global as any).document = dom.window.document;
});

// Minimal fake ProseMirror EditorView — renderWidget only calls
// view.coordsAtPos(pos) to position the popup; nothing else on `view` is touched.
function fakeView() {
  return {
    coordsAtPos: (_pos: number) => ({ top: 0, bottom: 20, left: 0, right: 0 }),
  };
}

function baseState(overrides: Partial<SuggestionState>): SuggestionState {
  return {
    active: true,
    range: { from: 0, to: 2 },
    query: '',
    items: [],
    selectedIndex: 0,
    ...overrides,
  };
}

describe('renderWidget — empty-state semantics (#2419 round-2 Finding 2 Part B)', () => {
  it('TEST 5: bare "[[" with empty query and no items renders NOTHING (returns null)', () => {
    const state = baseState({ query: '', items: [] });
    const widget = renderWidget(fakeView(), state);
    expect(widget).toBeNull();
  });

  it('inactive state (no [[ trigger active) renders nothing, regardless of query/items', () => {
    const state = baseState({ active: false, range: null, query: '', items: [] });
    const widget = renderWidget(fakeView(), state);
    expect(widget).toBeNull();
  });

  it('TEST 6: non-empty query with zero matches renders a popup containing "No notes found"', () => {
    const state = baseState({ query: 'Zzzznomatch', items: [] });
    const widget = renderWidget(fakeView(), state);
    expect(widget).not.toBeNull();
    expect(widget!.textContent).toContain('No notes found');
  });

  it('non-empty query with matches renders the matched items, not "No notes found"', () => {
    const state = baseState({
      query: 'Define',
      items: [{ title: 'Define Target Customer', id: 'note-1' }],
    });
    const widget = renderWidget(fakeView(), state);
    expect(widget).not.toBeNull();
    expect(widget!.textContent).not.toContain('No notes found');
    expect(widget!.textContent).toContain('Define Target Customer');
  });

  it('single-character query with zero items still renders "No notes found" (not treated as empty)', () => {
    // Guards against an overly-broad fix that treats any short query as "empty" —
    // only a TRULY empty (zero-length) query should suppress the popup.
    const state = baseState({ query: 'Z', items: [] });
    const widget = renderWidget(fakeView(), state);
    expect(widget).not.toBeNull();
    expect(widget!.textContent).toContain('No notes found');
  });
});

describe('Revert guard — old behavior would show "No notes found" on bare [[', () => {
  it('confirms the bug scenario: state shape for bare "[[" is indistinguishable from "query with zero matches" by items.length alone', () => {
    // This test documents WHY the bug existed: both real-world states have
    // `items.length === 0`. The fix must additionally inspect `query` to tell
    // them apart — this test doesn't assert on renderWidget itself, it pins
    // the state-shape ambiguity that made the old single-condition check wrong.
    const bareTrigger = baseState({ query: '', items: [] });
    const zeroMatchQuery = baseState({ query: 'Zzz', items: [] });

    expect(bareTrigger.items.length).toBe(0);
    expect(zeroMatchQuery.items.length).toBe(0);
    // The only distinguishing field is `query` — proving the fix's added
    // condition is necessary, not just sufficient.
    expect(bareTrigger.query).not.toBe(zeroMatchQuery.query);

    // And the fixed renderWidget DOES correctly distinguish them:
    expect(renderWidget(fakeView(), bareTrigger)).toBeNull();
    expect(renderWidget(fakeView(), zeroMatchQuery)).not.toBeNull();
  });
});
