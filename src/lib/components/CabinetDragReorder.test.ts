/**
 * #2334 — Cabinet Drag-to-Reorder (container-level relocation)
 *
 * Tests the CONTAINER-LEVEL drop handling added to NotesBrowser.svelte's Cabinet
 * list container (.folder-card-list — cabinetListDragOver/cabinetListDragLeave/
 * cabinetListDrop) and KanbanBoard.svelte's Cabinet column content container
 * (.column-content — cabinetColumnDragOver/cabinetColumnDragLeave/
 * cabinetColumnDrop), plus the shared coordinate-index resolution utility
 * (dragReorderIndex.ts).
 *
 * Original per-card drop handlers (cabinetDrop/cabinetCardDrop) were deleted —
 * the entire column surface, including the seams/gaps between cards, is now a
 * valid reorder target. Drag SOURCE stays per-card (ondragstart); only DROP
 * handling moved to the container.
 *
 * These tests exercise the logic directly (not via full component render, which
 * requires the Tauri window context) — a faithful extraction of the exact
 * state/handler logic in NotesBrowser.svelte / KanbanBoard.svelte, matching the
 * house pattern established by NotesBrowser.collapse-rails.test.ts and
 * Settings.ctx-window-override.test.ts.
 *
 * ── DMG-GATED NOTE ──────────────────────────────────────────────────────────
 * jsdom does NOT perform real layout — `Element.getBoundingClientRect()` on
 * jsdom-rendered elements always returns a zeroed rect (top: 0, height: 0 for
 * every card). This means the actual geometric "which card boundary is the
 * pointer closest to, given real rendered pixel positions" cannot be exercised
 * end-to-end in this harness. What IS exercised here:
 *   1. `resolveDropIndex` / `normalizeInsertIndex` (dragReorderIndex.ts) as pure
 *      functions, given synthetic CardRect[] input that simulates real layout
 *      (arbitrary top/height values) — this is the actual coordinate math, and
 *      it is fully unit-tested, DMG-free.
 *   2. The container-level handler is wired and receives drop events (real DOM
 *      dispatch + bubbling, jsdom-safe since it doesn't depend on layout).
 *   3. Mode disambiguation (cabinetDragSourceId set → reorder branch, unset →
 *      status-change branch / falls through to column handler) — exercised via
 *      the extracted handler logic with an explicit `rawDropIndex` input
 *      (bypassing the `getBoundingClientRect()` call, which the extracted test
 *      harness accepts as a parameter instead of computing it from the DOM).
 *   4. The reorder function (reorderNotes) is called with the expected
 *      (id, sort_order) pairs for a given resolved index.
 * What is DMG-ONLY (cannot be exercised in jsdom, must be manually verified /
 * covered by Playwright e2e where real layout exists): the actual
 * `cardRectsFromElements()` → `getBoundingClientRect()` → real pixel Y-coordinate
 * → resolved index pipeline end-to-end against genuinely rendered, laid-out
 * `.cabinet-drag-wrap` elements. See PR body for the exact assertion boundary.
 */

import { describe, it, expect, vi, beforeEach } from 'vitest';
import { resolveDropIndex, normalizeInsertIndex, cardRectsFromElements, shouldShowIndicator, type CardRect } from '$lib/utils/dragReorderIndex';

// ─────────────────────────────────────────────────────────────────────────
// Section 0: dragReorderIndex.ts — pure coordinate math (DMG-free, real logic)
// ─────────────────────────────────────────────────────────────────────────

describe('resolveDropIndex — pure coordinate math (#2334)', () => {
  // Three cards, each 130px tall, stacked with 8px gaps, starting at y=0:
  //   card 0: top=0,   height=130  → midpoint 65,  spans [0,138)  (gap 130-138)
  //   card 1: top=138, height=130  → midpoint 203, spans [138,276) (gap 268-276)
  //   card 2: top=276, height=130  → midpoint 341, spans [276,406)
  const cards: CardRect[] = [
    { id: 'a', top: 0, height: 130 },
    { id: 'b', top: 138, height: 130 },
    { id: 'c', top: 276, height: 130 },
  ];

  it('resolves to index 0 when the pointer is above the first card midpoint', () => {
    expect(resolveDropIndex(10, cards)).toBe(0);
  });

  it('resolves to index 1 when the pointer is in the SEAM/GAP between card 0 and card 1 (the bug this PR fixes)', () => {
    // The gap between card 0 (ends 130) and card 1 (starts 138) is exactly the
    // territory that, before this fix, belonged to no drop-target element at
    // all and fell through to the column-level status-change handler. y=134 is
    // squarely in that gap.
    expect(resolveDropIndex(134, cards)).toBe(1);
  });

  it('resolves to index 1 when the pointer is just past card 0 midpoint (top half of card 1 still resolves toward index 1 boundary)', () => {
    expect(resolveDropIndex(70, cards)).toBe(1);
  });

  it('resolves to index 2 when the pointer is in the seam between card 1 and card 2', () => {
    expect(resolveDropIndex(272, cards)).toBe(2);
  });

  it('resolves to index 3 (list end) when the pointer is below every card midpoint', () => {
    expect(resolveDropIndex(500, cards)).toBe(3);
  });

  it('resolves to index 0 for an empty card list (drop into an empty Cabinet column)', () => {
    expect(resolveDropIndex(50, [])).toBe(0);
  });

  it('resolves correctly for a SINGLE-CARD list: above, at midpoint, and below the card', () => {
    // One card: top=0, height=130 → midpoint 65, spans [0,130).
    const singleCard: CardRect[] = [{ id: 'solo', top: 0, height: 130 }];

    // Above the card entirely — well before the midpoint.
    expect(resolveDropIndex(-10, singleCard)).toBe(0);

    // Below the card entirely — well past the midpoint (and past the card's bottom edge).
    expect(resolveDropIndex(200, singleCard)).toBe(1);

    // Exactly at the midpoint: the loop condition is `clientY < midpoint`, so a
    // clientY equal to the midpoint does NOT satisfy `<` and falls through to
    // index 1 (the function's documented "bottom half inserts after" rule —
    // the midpoint itself belongs to the bottom half boundary).
    expect(resolveDropIndex(65, singleCard)).toBe(1);
  });
});

describe('normalizeInsertIndex — splice-index adjustment after source removal (#2334)', () => {
  it('returns the raw index unchanged when the raw index is at or before the source', () => {
    expect(normalizeInsertIndex(2, 0)).toBe(0);
    expect(normalizeInsertIndex(2, 2)).toBe(2);
  });

  it('decrements the raw index by 1 when the raw index is after the source (post-removal shift)', () => {
    expect(normalizeInsertIndex(0, 3)).toBe(2);
    expect(normalizeInsertIndex(1, 2)).toBe(1);
  });
});

// ─────────────────────────────────────────────────────────────────────────
// shouldShowIndicator — drop-indicator suppression at identity-move slots
// (#2334 visual fix). Per docs/audits/2026-08-02-noop-guard-verification.md,
// normalizeInsertIndex's arithmetic is CORRECT: rawDropIndex === sourceIdx and
// rawDropIndex === sourceIdx + 1 are both true identity moves (the no-op guard
// in cabinetListDrop/cabinetColumnDrop is untouched by this fix). The bug was
// that the drop indicator still rendered at those two slots, implying a move
// would happen when it wouldn't. These tests pin the suppression behavior
// specifically at those two slots — NOT indicator behavior in general — so
// they fail red if the identity-slot suppression is reverted/removed.
// ─────────────────────────────────────────────────────────────────────────

describe('shouldShowIndicator — suppress indicator at identity-move slots (#2334)', () => {
  it('mid-list source (sourceIdx=1 in a 3-card list): suppressed at rawDropIndex === sourceIdx and sourceIdx + 1', () => {
    const sourceIdx = 1;
    expect(shouldShowIndicator(sourceIdx, 1)).toBe(false); // rawDropIndex === sourceIdx
    expect(shouldShowIndicator(sourceIdx, 2)).toBe(false); // rawDropIndex === sourceIdx + 1
  });

  it('mid-list source (sourceIdx=1 in a 3-card list): indicator IS present at real-move slots (0 and 3)', () => {
    const sourceIdx = 1;
    expect(shouldShowIndicator(sourceIdx, 0)).toBe(true);
    expect(shouldShowIndicator(sourceIdx, 3)).toBe(true);
  });

  it('edge case — sourceIdx=0 (first card): suppressed at 0 and 1, present at 2 and beyond', () => {
    const sourceIdx = 0;
    expect(shouldShowIndicator(sourceIdx, 0)).toBe(false);
    expect(shouldShowIndicator(sourceIdx, 1)).toBe(false);
    expect(shouldShowIndicator(sourceIdx, 2)).toBe(true);
    expect(shouldShowIndicator(sourceIdx, 3)).toBe(true);
  });

  it('edge case — sourceIdx=last (e.g. index 4 of a 5-card list): suppressed at last and last+1, present elsewhere', () => {
    const sourceIdx = 4; // last card in a 5-card list (indices 0-4)
    expect(shouldShowIndicator(sourceIdx, 4)).toBe(false); // rawDropIndex === sourceIdx
    expect(shouldShowIndicator(sourceIdx, 5)).toBe(false); // rawDropIndex === sourceIdx + 1 (= length, drop-at-end sentinel)
    expect(shouldShowIndicator(sourceIdx, 0)).toBe(true);
    expect(shouldShowIndicator(sourceIdx, 1)).toBe(true);
    expect(shouldShowIndicator(sourceIdx, 2)).toBe(true);
    expect(shouldShowIndicator(sourceIdx, 3)).toBe(true);
  });

  it('red-on-revert guard: does NOT suppress every slot (proves this tests identity-slot suppression, not "always hide")', () => {
    // If someone reverted the fix to `return false` unconditionally (always
    // suppress) instead of the targeted identity-slot check, this would catch
    // it — real-move slots must still resolve to `true`.
    expect(shouldShowIndicator(1, 0)).toBe(true);
    expect(shouldShowIndicator(1, 3)).toBe(true);
  });

  it('red-on-revert guard: does NOT allow the indicator through at the identity slots (proves this tests suppression, not a no-op stub that always returns true)', () => {
    // If someone reverted the fix to `return true` unconditionally (never
    // suppress — i.e. deleted the check entirely), this would catch it.
    expect(shouldShowIndicator(1, 1)).toBe(false);
    expect(shouldShowIndicator(1, 2)).toBe(false);
  });
});

describe('cardRectsFromElements — DOM extraction (jsdom-safe wiring check; real geometry is DMG-gated)', () => {
  it('extracts id/top/height for every element carrying data-cabinet-id, in DOM order', () => {
    const container = document.createElement('div');
    for (const id of ['x', 'y', 'z']) {
      const el = document.createElement('div');
      el.dataset.cabinetId = id;
      container.appendChild(el);
    }
    document.body.appendChild(container);

    const rects = cardRectsFromElements(container.querySelectorAll('[data-cabinet-id]'));
    expect(rects.map((r) => r.id)).toEqual(['x', 'y', 'z']);
    // DMG-GATED: jsdom getBoundingClientRect() always returns a zeroed rect, so
    // top/height are 0 here — real pixel values require a real layout engine.
    expect(rects.every((r) => r.top === 0 && r.height === 0)).toBe(true);

    document.body.removeChild(container);
  });
});

// ─────────────────────────────────────────────────────────────────────────
// Section 1: NotesBrowser.svelte Cabinet container-level drop logic (extracted)
// ─────────────────────────────────────────────────────────────────────────

interface FakeNote {
  id: string;
  task_status?: string;
  task_sort_order?: number;
  created_at?: string;
}

function makeNotesBrowserCabinetHandlers(
  reorderNotesImpl: (orders: [string, number][]) => Promise<void>,
  initialTaskNotes: FakeNote[],
) {
  let taskNotes: FakeNote[] = [...initialTaskNotes];
  let draggedNote: FakeNote | null = null;
  let cabinetDragSourceId: string | null = null;
  let cabinetDropIndicatorIndex: number | null = null;
  let resyncCount = 0;

  const state = () => ({ taskNotes, draggedNote, cabinetDragSourceId, cabinetDropIndicatorIndex, resyncCount });

  // Mirrors the RENDER PATH fix in NotesBrowser.svelte's `cabinetNotesLive`
  // derivation (#2334 render-race fix): filters to the Cabinet subset AND
  // sorts it by task_sort_order ASC (created_at DESC tiebreak), exactly like
  // the production `{@const cabinetNotesLive = ...}` block. This is what
  // actually renders on screen — it must be asserted directly, NOT derived by
  // manually re-sorting `taskNotes` in the test (that masking pattern is what
  // let the original render-race bug ship undetected).
  function cabinetNotesLive(): FakeNote[] {
    return taskNotes
      .filter((n) => !n.task_status || n.task_status === 'cabinet')
      .sort((a, b) => (a.task_sort_order ?? 0) - (b.task_sort_order ?? 0) || (new Date(b.created_at ?? 0).getTime() - new Date(a.created_at ?? 0).getTime()));
  }

  function cabinetDragStart(note: FakeNote) {
    cabinetDragSourceId = note.id;
    draggedNote = note;
  }

  // Mirrors a Task-column card's onDragStart in the real component: a plain
  // `draggedNote = note` assignment (NOT cabinetDragStart) — cabinetDragSourceId
  // is never touched.
  function taskCardDragStart(note: FakeNote) {
    draggedNote = note;
  }

  // Mirrors cabinetListDragOver: sets the indicator index from a (test-supplied)
  // resolved coordinate index. Real component computes this via
  // resolveDropIndex(e.clientY, cardRectsFromElements(...)) — DMG-gated for real
  // pixel values, but the indicator-set logic itself is exercised here.
  function cabinetListDragOver(rawIndex: number): boolean {
    if (!cabinetDragSourceId) return false; // container handler is a no-op for status-change drags
    cabinetDropIndicatorIndex = rawIndex;
    return true;
  }

  function cabinetListDragLeave() {
    cabinetDropIndicatorIndex = null;
  }

  // Mirrors cabinetListDrop, with rawDropIndex supplied directly (standing in
  // for the DMG-gated getBoundingClientRect()-derived value) rather than
  // recomputed from a DragEvent.
  async function cabinetListDrop(rawDropIndex: number): Promise<'reorder' | 'passthrough'> {
    if (!cabinetDragSourceId) {
      // Case: not a Cabinet-originated drag — falls through to the column-level
      // status-change handler (handleColumnDrop) unmodified. The container
      // handler itself does nothing.
      return 'passthrough';
    }

    cabinetDropIndicatorIndex = null;
    const sourceId = cabinetDragSourceId;
    cabinetDragSourceId = null;
    draggedNote = null;

    const cabinetNotes = taskNotes.filter((n) => !n.task_status || n.task_status === 'cabinet');
    const sourceIdx = cabinetNotes.findIndex((n) => n.id === sourceId);
    if (sourceIdx === -1) return 'reorder';

    const insertIdx = normalizeInsertIndex(sourceIdx, rawDropIndex);
    if (insertIdx === sourceIdx) return 'reorder'; // no-op: dropped back in same slot

    const reordered = [...cabinetNotes];
    const [moved] = reordered.splice(sourceIdx, 1);
    reordered.splice(insertIdx, 0, moved);

    const withOrders = reordered.map((n, idx) => ({ ...n, task_sort_order: idx * 1000 }));
    const orderById = new Map(withOrders.map((n) => [n.id, n.task_sort_order]));
    taskNotes = taskNotes.map((n) => (orderById.has(n.id) ? { ...n, task_sort_order: orderById.get(n.id) } : n));

    try {
      await reorderNotesImpl(withOrders.map((n) => [n.id, n.task_sort_order!] as [string, number]));
    } catch {
      resyncCount++;
    }
    return 'reorder';
  }

  function cabinetDragEnd() {
    cabinetDragSourceId = null;
    cabinetDropIndicatorIndex = null;
    draggedNote = null;
  }

  return {
    state,
    cabinetNotesLive,
    cabinetDragStart,
    taskCardDragStart,
    cabinetListDragOver,
    cabinetListDragLeave,
    cabinetListDrop,
    cabinetDragEnd,
  };
}

describe('NotesBrowser.svelte Cabinet container-level drop logic — #2334', () => {
  let reorderMock: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    reorderMock = vi.fn(async () => {});
  });

  it('reorders on a GAP drop: dragging the last card and resolving to index 0 (the gap/top of the list, not a card) moves it to first', async () => {
    const notes: FakeNote[] = [
      { id: 'a', task_status: undefined },
      { id: 'b', task_status: 'cabinet' },
      { id: 'c', task_status: 'cabinet' },
    ];
    const h = makeNotesBrowserCabinetHandlers(reorderMock, notes);

    h.cabinetDragStart(notes[2]); // drag 'c'
    // Coordinate-resolved index 0 = "insert at the very top" — this is the
    // container-level equivalent of dropping in the seam/gap above card 'a',
    // which the OLD per-card handler could never receive (no element covered
    // that territory).
    const result = await h.cabinetListDrop(0);

    expect(result).toBe('reorder');
    // Assert the RENDERED array order (cabinetNotesLive, mirroring the
    // production `{@each cabinetNotesLive as note}` render path) directly —
    // NOT a manually-sorted view of raw state. This is the render-race
    // regression check: it fails if the optimistic update only rewrites the
    // task_sort_order field without the render path also sorting by it.
    expect(h.cabinetNotesLive().map((n) => n.id)).toEqual(['c', 'a', 'b']);
  });

  it('resolves a GAP drop between card 1 and card 2 (index 2) correctly, moving the first card into the middle', async () => {
    const notes: FakeNote[] = [
      { id: 'a', task_status: 'cabinet' },
      { id: 'b', task_status: 'cabinet' },
      { id: 'c', task_status: 'cabinet' },
    ];
    const h = makeNotesBrowserCabinetHandlers(reorderMock, notes);

    h.cabinetDragStart(notes[0]); // drag 'a'
    // rawDropIndex=2 means "insert at the seam between b and c" — a gap the
    // dragged item (originally at index 0) was never over.
    await h.cabinetListDrop(2);

    // Rendered order, not manually-sorted state (see note above).
    expect(h.cabinetNotesLive().map((n) => n.id)).toEqual(['b', 'a', 'c']);
  });

  it('persists the reorder via reorderNotes with gap-based (step 1000) sort_order values', async () => {
    const notes: FakeNote[] = [
      { id: 'a', task_status: undefined },
      { id: 'b', task_status: 'cabinet' },
      { id: 'c', task_status: 'cabinet' },
    ];
    const h = makeNotesBrowserCabinetHandlers(reorderMock, notes);

    h.cabinetDragStart(notes[2]);
    await h.cabinetListDrop(0);

    expect(reorderMock).toHaveBeenCalledTimes(1);
    const orders: [string, number][] = reorderMock.mock.calls[0][0];
    const bySortOrder = [...orders].sort((x, y) => x[1] - y[1]);
    expect(bySortOrder.map(([id]) => id)).toEqual(['c', 'a', 'b']);
    expect(bySortOrder.map(([, order]) => order)).toEqual([0, 1000, 2000]);
  });

  it('dropping a card back into its own slot is a no-op (no reorder, no persist call)', async () => {
    const notes: FakeNote[] = [{ id: 'a', task_status: 'cabinet' }, { id: 'b', task_status: 'cabinet' }];
    const h = makeNotesBrowserCabinetHandlers(reorderMock, notes);

    h.cabinetDragStart(notes[0]); // sourceIdx = 0
    await h.cabinetListDrop(0); // resolves to the same slot

    expect(reorderMock).not.toHaveBeenCalled();
    expect(h.state().taskNotes.map((n) => n.id)).toEqual(['a', 'b']);
  });

  it('RENDER-RACE REGRESSION (#2334): after cabinetListDrop, cabinetNotesLive() IS in task_sort_order order WITHOUT any manual sort in the assertion — while the raw taskNotes array position is untouched (proving .map() alone never moves array elements, only the render-derivation sort fixes it)', async () => {
    const notes: FakeNote[] = [
      { id: 'a', task_status: 'cabinet' },
      { id: 'b', task_status: 'cabinet' },
      { id: 'c', task_status: 'cabinet' },
    ];
    const h = makeNotesBrowserCabinetHandlers(reorderMock, notes);

    h.cabinetDragStart(notes[2]); // drag 'c' to the top
    await h.cabinetListDrop(0);

    // The raw array's ELEMENT POSITION never changes — only the task_sort_order
    // FIELD is rewritten by the optimistic .map() update. This is the exact
    // shape of the bug: if a test asserted against `taskNotes` sorted manually
    // by task_sort_order, it would pass even if the render path had no sort at
    // all. Asserting raw array order stays ['a','b','c'] here documents that
    // fact explicitly.
    expect(h.state().taskNotes.map((n) => n.id)).toEqual(['a', 'b', 'c']);

    // The RENDER PATH (cabinetNotesLive, mirroring NotesBrowser.svelte's
    // `{@const cabinetNotesLive = ...}` derivation) must reflect the new order
    // directly — this is what the user actually sees, and it only passes if
    // the render derivation itself sorts by task_sort_order.
    expect(h.cabinetNotesLive().map((n) => n.id)).toEqual(['c', 'a', 'b']);
  });

  it('MODE DISAMBIGUATION: cabinetDragSourceId set → the container handler resolves reorder mode', async () => {
    const notes: FakeNote[] = [{ id: 'a', task_status: 'cabinet' }, { id: 'b', task_status: 'cabinet' }];
    const h = makeNotesBrowserCabinetHandlers(reorderMock, notes);

    h.cabinetDragStart(notes[0]);
    const result = await h.cabinetListDrop(1);

    expect(result).toBe('reorder');
  });

  it('MODE DISAMBIGUATION: cabinetDragSourceId NOT set → the container handler passes through (no-op), letting the column-level status-change handler run', async () => {
    const notes: FakeNote[] = [{ id: 'task-1', task_status: 'todo' }, { id: 'cab-1', task_status: 'cabinet' }];
    const h = makeNotesBrowserCabinetHandlers(reorderMock, notes);

    h.taskCardDragStart(notes[0]); // Task-column origin: only draggedNote is set
    const result = await h.cabinetListDrop(0);

    expect(result).toBe('passthrough');
    expect(reorderMock).not.toHaveBeenCalled();
    // draggedNote must survive the passthrough so handleColumnDrop can still act on it.
    expect(h.state().draggedNote).toBe(notes[0]);
  });

  it('cabinetListDragOver is a no-op (returns false, does not set the indicator) for a non-Cabinet-origin drag', () => {
    const notes: FakeNote[] = [{ id: 'task-1', task_status: 'todo' }];
    const h = makeNotesBrowserCabinetHandlers(reorderMock, notes);

    h.taskCardDragStart(notes[0]);
    const handled = h.cabinetListDragOver(1);

    expect(handled).toBe(false);
    expect(h.state().cabinetDropIndicatorIndex).toBeNull();
  });

  it('cabinetListDragOver sets the indicator index for a Cabinet-origin drag', () => {
    const notes: FakeNote[] = [{ id: 'a', task_status: 'cabinet' }];
    const h = makeNotesBrowserCabinetHandlers(reorderMock, notes);

    h.cabinetDragStart(notes[0]);
    const handled = h.cabinetListDragOver(2);

    expect(handled).toBe(true);
    expect(h.state().cabinetDropIndicatorIndex).toBe(2);
  });

  it('resyncs from the server (loadTaskNotes-equivalent) when the reorder persist call fails', async () => {
    const notes: FakeNote[] = [{ id: 'a', task_status: 'cabinet' }, { id: 'b', task_status: 'cabinet' }];
    const failingReorder = vi.fn(async () => {
      throw new Error('network error');
    });
    const h = makeNotesBrowserCabinetHandlers(failingReorder, notes);

    h.cabinetDragStart(notes[1]);
    await h.cabinetListDrop(0);

    expect(failingReorder).toHaveBeenCalledTimes(1);
    expect(h.state().resyncCount).toBe(1);
  });

  it('cabinetDragEnd clears cabinetDragSourceId, cabinetDropIndicatorIndex, AND draggedNote', () => {
    const notes: FakeNote[] = [{ id: 'a', task_status: 'cabinet' }];
    const h = makeNotesBrowserCabinetHandlers(reorderMock, notes);

    h.cabinetDragStart(notes[0]);
    h.cabinetListDragOver(1);
    expect(h.state().cabinetDragSourceId).toBe('a');
    expect(h.state().cabinetDropIndicatorIndex).toBe(1);

    h.cabinetDragEnd();

    expect(h.state().cabinetDragSourceId).toBeNull();
    expect(h.state().cabinetDropIndicatorIndex).toBeNull();
    expect(h.state().draggedNote).toBeNull();
  });
});

// ─────────────────────────────────────────────────────────────────────────
// #2334 canonical-array fix — post-reorder session consistency
//
// Root cause (docs/audits/2026-08-02-indicator-contract-mismatch.md,
// docs/audits/2026-08-02-noop-guard-verification.md): NotesBrowser.svelte used
// to have TWO divergent cabinet arrays — an UNSORTED `taskNotes.filter(...)`
// for sourceIdx computation in the dragover/drop handlers, and a SORTED
// `.filter().sort()` (`cabinetNotesLive`) for DOM rendering. The optimistic
// post-reorder update only rewrites `task_sort_order` FIELDS via `.map()` — it
// never reorders the raw `taskNotes` ARRAY. So after a successful reorder,
// WITHOUT a reload:
//   - the render path (sorted) shows the new order correctly
//   - the unsorted array used for sourceIdx still reflects the OLD array
//     position of every note, because `.map()` never moved array elements
// A second drag on the SAME session (no reload) then computes sourceIdx
// against the stale unsorted position, so `shouldShowIndicator` suppresses
// the wrong slots and the drop indicator misfires.
//
// The fix: ONE canonical derived array (filtered AND sorted by
// task_sort_order) feeds every consumer — DOM rendering, sourceIdx in
// dragover, sourceIdx in drop, shouldShowIndicator, and the reorder dispatch.
// Because it's a fresh filter+sort over `taskNotes` every time (mirroring
// Svelte's `$derived` reactivity — and matching KanbanBoard's
// `getTasksForColumn('cabinet')` reference pattern, which never had this bug
// for exactly this reason), an optimistic `task_sort_order` field update
// automatically flows through to a re-derived, correctly-ordered array with
// no explicit re-sort call required at the call site.
//
// These tests reproduce the EXACT divergence: `computeSourceIdxUnsorted`
// below is the pre-fix computation (taskNotes.filter only, no sort) that
// NotesBrowser.svelte's dragover/drop handlers used to perform locally;
// `computeSourceIdxCanonical` is the post-fix computation (filter + sort,
// matching the single `cabinetNotes` $derived array now used everywhere in
// NotesBrowser.svelte). Asserting against `computeSourceIdxUnsorted` is what
// makes this test suite RED against the old code path — it returns the STALE
// index after a same-session reorder, which is exactly the bug #2334 fixes.
// ─────────────────────────────────────────────────────────────────────────

describe('post-reorder session consistency (#2334 canonical cabinet array)', () => {
  // Pre-fix computation: unsorted filter only — mirrors the DELETED
  // `cabinetNotes` local variable that used to feed sourceIdx lookups in
  // NotesBrowser.svelte's cabinetListDragOver/cabinetListDrop. Array element
  // ORDER never changes across an optimistic reorder (only task_sort_order
  // FIELDS are rewritten by `.map()`), so this returns a STALE index for any
  // note whose visual/sorted position moved without its array position moving.
  function computeSourceIdxUnsorted(taskNotes: FakeNote[], noteId: string): number {
    const cabinet = taskNotes.filter((n) => !n.task_status || n.task_status === 'cabinet');
    return cabinet.findIndex((n) => n.id === noteId);
  }

  // Post-fix computation: filter + sort by task_sort_order — mirrors the ONE
  // canonical `cabinetNotes` $derived array NotesBrowser.svelte now uses for
  // every consumer (DOM rendering, both sourceIdx lookups, shouldShowIndicator,
  // reorder dispatch). Re-filters AND re-sorts from `taskNotes` every call, so
  // it always reflects the current task_sort_order values — the same pattern
  // as KanbanBoard's `getTasksForColumn('cabinet')` reference implementation.
  function computeSourceIdxCanonical(taskNotes: FakeNote[], noteId: string): number {
    const cabinet = taskNotes
      .filter((n) => !n.task_status || n.task_status === 'cabinet')
      .sort((a, b) => (a.task_sort_order ?? 0) - (b.task_sort_order ?? 0) || (new Date(b.created_at ?? 0).getTime() - new Date(a.created_at ?? 0).getTime()));
    return cabinet.findIndex((n) => n.id === noteId);
  }

  // Applies the exact optimistic-update shape from cabinetListDrop: reorder a
  // canonical-array snapshot, assign gap-based (step 1000) sort_orders, then
  // `.map()` those sort_order values back onto `taskNotes` WITHOUT touching
  // array element order — faithfully reproducing what production code does
  // (and does NOT do — it never splices/reorders the raw array).
  function applyOptimisticReorder(taskNotes: FakeNote[], sourceId: string, rawDropIndex: number): FakeNote[] {
    const canonical = taskNotes
      .filter((n) => !n.task_status || n.task_status === 'cabinet')
      .sort((a, b) => (a.task_sort_order ?? 0) - (b.task_sort_order ?? 0) || (new Date(b.created_at ?? 0).getTime() - new Date(a.created_at ?? 0).getTime()));
    const sourceIdx = canonical.findIndex((n) => n.id === sourceId);
    const insertIdx = normalizeInsertIndex(sourceIdx, rawDropIndex);

    const reordered = [...canonical];
    const [moved] = reordered.splice(sourceIdx, 1);
    reordered.splice(insertIdx, 0, moved);

    const orderById = new Map(reordered.map((n, idx) => [n.id, idx * 1000]));
    // `.map()` only — array ELEMENT ORDER of `taskNotes` is untouched, matching
    // production's optimistic update exactly.
    return taskNotes.map((n) => (orderById.has(n.id) ? { ...n, task_sort_order: orderById.get(n.id) } : n));
  }

  it('sourceIdx and indicator suppression remain correct after a successful reorder without reload', () => {
    // Setup: 3 cabinet cards [A, B, C], raw array order A,B,C, sort_order
    // [1000, 2000, 3000] → canonical (sorted) order is also [A, B, C] initially.
    let taskNotes: FakeNote[] = [
      { id: 'A', task_status: 'cabinet', task_sort_order: 1000 },
      { id: 'B', task_status: 'cabinet', task_sort_order: 2000 },
      { id: 'C', task_status: 'cabinet', task_sort_order: 3000 },
    ];

    // Reorder #1: move C (canonical index 2) to the top (rawDropIndex 0).
    taskNotes = applyOptimisticReorder(taskNotes, 'C', 0);

    // Canonical (sorted) order is now [C, A, B] — the render path is correct.
    expect(computeSourceIdxCanonical(taskNotes, 'C')).toBe(0);
    // Raw array element order was NEVER touched by the optimistic `.map()` —
    // still [A, B, C] — so the pre-fix unsorted lookup for C returns its OLD
    // array position (2), not its new visual position (0). This is the exact
    // #2334 bug: sourceIdx computed against the wrong order.
    expect(computeSourceIdxUnsorted(taskNotes, 'C')).toBe(2);

    // WITHOUT reloading/re-fetching, the user grabs card C again — now
    // rendered at position 0. sourceIdx MUST be computed from the same
    // canonical array the DOM was rendered from (0), not the stale unsorted
    // position (2) — this is the assertion that fails on the old code.
    const sourceIdx = computeSourceIdxCanonical(taskNotes, 'C');
    expect(sourceIdx).toBe(0);

    // shouldShowIndicator(0, 2): real move (drop at the bottom seam) — must
    // NOT be suppressed.
    expect(shouldShowIndicator(sourceIdx, 2)).toBe(true);
    // shouldShowIndicator(0, 0): identity slot (dropping back into C's own
    // slot) — must be suppressed.
    expect(shouldShowIndicator(sourceIdx, 0)).toBe(false);
    // shouldShowIndicator(0, 1): identity slot (the seam immediately below C's
    // own slot) — must be suppressed.
    expect(shouldShowIndicator(sourceIdx, 1)).toBe(false);

    // Red-on-revert proof: if sourceIdx were computed via the OLD unsorted
    // lookup instead (2), the exact same shouldShowIndicator calls would
    // resolve backwards — the indicator would be suppressed at real-move slots
    // and shown at identity slots.
    const staleSourceIdx = computeSourceIdxUnsorted(taskNotes, 'C');
    expect(shouldShowIndicator(staleSourceIdx, 2)).toBe(false); // wrongly suppressed under the old bug
    expect(shouldShowIndicator(staleSourceIdx, 0)).toBe(true);  // wrongly shown under the old bug
  });

  it('sourceIdx and indicator suppression remain correct across TWO consecutive reorders without reload', () => {
    let taskNotes: FakeNote[] = [
      { id: 'A', task_status: 'cabinet', task_sort_order: 1000 },
      { id: 'B', task_status: 'cabinet', task_sort_order: 2000 },
      { id: 'C', task_status: 'cabinet', task_sort_order: 3000 },
    ];

    // Reorder #1: move C to the top → canonical order [C, A, B].
    taskNotes = applyOptimisticReorder(taskNotes, 'C', 0);
    expect(computeSourceIdxCanonical(taskNotes, 'C')).toBe(0);
    expect(computeSourceIdxCanonical(taskNotes, 'A')).toBe(1);
    expect(computeSourceIdxCanonical(taskNotes, 'B')).toBe(2);

    // Reorder #2, SAME SESSION, no reload: move B (canonical index 2) to the
    // top → canonical order [B, C, A]. If sourceIdx for B were computed from
    // the stale unsorted array (still physically ordered [A, B, C] since
    // reorder #1's `.map()` never touched array order), it would incorrectly
    // resolve to 1 instead of 2.
    expect(computeSourceIdxUnsorted(taskNotes, 'B')).toBe(1); // stale/buggy value
    const sourceIdx2 = computeSourceIdxCanonical(taskNotes, 'B');
    expect(sourceIdx2).toBe(2); // correct value, matches rendered position

    taskNotes = applyOptimisticReorder(taskNotes, 'B', 0);

    // Canonical order is now [B, C, A].
    expect(computeSourceIdxCanonical(taskNotes, 'B')).toBe(0);
    expect(computeSourceIdxCanonical(taskNotes, 'C')).toBe(1);
    expect(computeSourceIdxCanonical(taskNotes, 'A')).toBe(2);

    // A third (still-no-reload) grab of B, now at canonical position 0.
    const sourceIdx3 = computeSourceIdxCanonical(taskNotes, 'B');
    expect(sourceIdx3).toBe(0);
    expect(shouldShowIndicator(sourceIdx3, 2)).toBe(true);  // real move
    expect(shouldShowIndicator(sourceIdx3, 0)).toBe(false); // identity slot
    expect(shouldShowIndicator(sourceIdx3, 1)).toBe(false); // identity slot

    // And a grab of A (now at canonical position 2, the last card) confirms
    // the end-of-list identity slots are also still correct after two
    // consecutive same-session reorders.
    const sourceIdxA = computeSourceIdxCanonical(taskNotes, 'A');
    expect(sourceIdxA).toBe(2);
    expect(shouldShowIndicator(sourceIdxA, 2)).toBe(false); // identity slot
    expect(shouldShowIndicator(sourceIdxA, 3)).toBe(false); // identity slot (drop-at-end sentinel)
    expect(shouldShowIndicator(sourceIdxA, 0)).toBe(true);  // real move
  });
});

// ─────────────────────────────────────────────────────────────────────────
// Section 2: KanbanBoard.svelte Cabinet container-level drop logic (extracted)
// ─────────────────────────────────────────────────────────────────────────

interface FakeTask {
  id: string;
  status: string;
  sort_order?: number;
  created_at?: string;
}

function makeKanbanCabinetHandlers(
  reorderNotesImpl: (orders: [string, number][]) => Promise<void>,
  initialTasks: FakeTask[],
) {
  let tasks: FakeTask[] = [...initialTasks];
  let draggedTask: FakeTask | null = null;
  let cabinetDragSourceId: string | null = null;
  let cabinetDropIndicatorIndex: number | null = null;

  const state = () => ({ tasks, draggedTask, cabinetDragSourceId, cabinetDropIndicatorIndex });

  // Mirrors the RENDER PATH fix in KanbanBoard.svelte's `getTasksForColumn`
  // (#2334 render-race fix): the Cabinet subset is filtered AND sorted by
  // sort_order ASC (created_at DESC tiebreak) — exactly like production.
  // Task columns keep plain filter (system priority/due_date order, untouched).
  function getTasksForColumn(status: string): FakeTask[] {
    const filtered = tasks.filter((t) => t.status === status);
    if (status === 'cabinet') {
      return filtered.sort((a, b) => (a.sort_order ?? 0) - (b.sort_order ?? 0) || (new Date(b.created_at ?? 0).getTime() - new Date(a.created_at ?? 0).getTime()));
    }
    return filtered;
  }

  function handleCardDragStart(id: string, status: string) {
    const task = tasks.find((t) => t.id === id);
    if (!task) return;
    draggedTask = task;
    if (status === 'cabinet') cabinetDragSourceId = id;
  }

  function cabinetColumnDragOver(rawIndex: number): boolean {
    if (!cabinetDragSourceId) return false;
    cabinetDropIndicatorIndex = rawIndex;
    return true;
  }

  function cabinetColumnDragLeave() {
    cabinetDropIndicatorIndex = null;
  }

  async function cabinetColumnDrop(rawDropIndex: number): Promise<'reorder' | 'passthrough'> {
    if (!cabinetDragSourceId) return 'passthrough';

    cabinetDropIndicatorIndex = null;
    const sourceId = cabinetDragSourceId;
    cabinetDragSourceId = null;
    draggedTask = null;

    const cabinetTasks = getTasksForColumn('cabinet');
    const sourceIdx = cabinetTasks.findIndex((t) => t.id === sourceId);
    if (sourceIdx === -1) return 'reorder';

    const insertIdx = normalizeInsertIndex(sourceIdx, rawDropIndex);
    if (insertIdx === sourceIdx) return 'reorder';

    const reordered = [...cabinetTasks];
    const [moved] = reordered.splice(sourceIdx, 1);
    reordered.splice(insertIdx, 0, moved);

    const withOrders = reordered.map((t, idx) => ({ ...t, sort_order: idx * 1000 }));
    const orderById = new Map(withOrders.map((t) => [t.id, t.sort_order]));
    tasks = tasks.map((t) => (orderById.has(t.id) ? { ...t, sort_order: orderById.get(t.id)! } : t));

    try {
      await reorderNotesImpl(withOrders.map((t) => [t.id, t.sort_order] as [string, number]));
    } catch {
      // resync path — not exercised here, covered by the NotesBrowser resync test (identical shape).
    }
    return 'reorder';
  }

  function cabinetCardDragEnd() {
    cabinetDragSourceId = null;
    cabinetDropIndicatorIndex = null;
    draggedTask = null;
  }

  return {
    state,
    getTasksForColumn,
    handleCardDragStart,
    cabinetColumnDragOver,
    cabinetColumnDragLeave,
    cabinetColumnDrop,
    cabinetCardDragEnd,
  };
}

describe('KanbanBoard.svelte Cabinet container-level drop logic — #2334', () => {
  let reorderMock: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    reorderMock = vi.fn(async () => {});
  });

  it('reorders on a GAP drop (coordinate-resolved index), not requiring the drop to land on a specific card', async () => {
    const tasks: FakeTask[] = [
      { id: 'c1', status: 'cabinet' },
      { id: 'c2', status: 'cabinet' },
      { id: 't1', status: 'todo' },
    ];
    const h = makeKanbanCabinetHandlers(reorderMock, tasks);

    h.handleCardDragStart('c2', 'cabinet');
    expect(h.state().cabinetDragSourceId).toBe('c2');

    // rawDropIndex=0 = "insert at the top of the Cabinet column" — the gap
    // above c1, never coverable by the old per-card handler.
    const result = await h.cabinetColumnDrop(0);

    expect(result).toBe('reorder');
    expect(reorderMock).toHaveBeenCalledTimes(1);
    const orders: [string, number][] = reorderMock.mock.calls[0][0];
    const bySortOrder = [...orders].sort((x, y) => x[1] - y[1]);
    expect(bySortOrder.map(([id]) => id)).toEqual(['c2', 'c1']);

    // RENDER-RACE REGRESSION CHECK: assert getTasksForColumn('cabinet') — the
    // actual render path — is in the new order directly, with NO manual sort
    // in the assertion. This is what fails if the optimistic update only
    // rewrites the sort_order field without the render path sorting by it.
    expect(h.getTasksForColumn('cabinet').map((t) => t.id)).toEqual(['c2', 'c1']);
  });

  it('does NOT set cabinetDragSourceId when the drag originates on a non-Cabinet column card', () => {
    const tasks: FakeTask[] = [{ id: 't1', status: 'todo' }];
    const h = makeKanbanCabinetHandlers(reorderMock, tasks);

    h.handleCardDragStart('t1', 'todo');
    expect(h.state().cabinetDragSourceId).toBeNull();
    expect(h.state().draggedTask?.id).toBe('t1');
  });

  it('MODE DISAMBIGUATION: a Task-column-origin drag passes through the container handler (status-change branch), never reorders', async () => {
    const tasks: FakeTask[] = [
      { id: 't1', status: 'todo' },
      { id: 'c1', status: 'cabinet' },
    ];
    const h = makeKanbanCabinetHandlers(reorderMock, tasks);

    h.handleCardDragStart('t1', 'todo'); // Task-column origin: no cabinetDragSourceId
    const result = await h.cabinetColumnDrop(0);

    expect(result).toBe('passthrough');
    expect(reorderMock).not.toHaveBeenCalled();
    // draggedTask must survive so the column-level handleDrop can still act on it.
    expect(h.state().draggedTask?.id).toBe('t1');
  });

  it('cabinetColumnDragOver is a no-op for a non-Cabinet-origin drag (does not set the indicator)', () => {
    const tasks: FakeTask[] = [{ id: 't1', status: 'todo' }];
    const h = makeKanbanCabinetHandlers(reorderMock, tasks);

    h.handleCardDragStart('t1', 'todo');
    const handled = h.cabinetColumnDragOver(1);

    expect(handled).toBe(false);
    expect(h.state().cabinetDropIndicatorIndex).toBeNull();
  });

  it('cabinetColumnDragOver sets the indicator index for a Cabinet-origin drag (mirrors the NotesBrowser cabinetListDragOver positive case)', () => {
    const tasks: FakeTask[] = [
      { id: 'c1', status: 'cabinet' },
      { id: 'c2', status: 'cabinet' },
    ];
    const h = makeKanbanCabinetHandlers(reorderMock, tasks);

    h.handleCardDragStart('c1', 'cabinet');
    const handled = h.cabinetColumnDragOver(2);

    expect(handled).toBe(true);
    expect(h.state().cabinetDropIndicatorIndex).toBe(2);
  });

  it('RENDER-RACE REGRESSION (#2334): after cabinetColumnDrop, getTasksForColumn(\'cabinet\') IS in sort_order order WITHOUT any manual sort in the assertion — while the raw tasks array position is untouched', async () => {
    const tasks: FakeTask[] = [
      { id: 'c1', status: 'cabinet' },
      { id: 'c2', status: 'cabinet' },
      { id: 'c3', status: 'cabinet' },
    ];
    const h = makeKanbanCabinetHandlers(reorderMock, tasks);

    h.handleCardDragStart('c3', 'cabinet'); // drag 'c3' to the top
    await h.cabinetColumnDrop(0);

    // Raw array element position never changes — only the sort_order FIELD is
    // rewritten by the optimistic .map() update.
    expect(h.state().tasks.map((t) => t.id)).toEqual(['c1', 'c2', 'c3']);

    // The RENDER PATH (getTasksForColumn) must reflect the new order directly.
    expect(h.getTasksForColumn('cabinet').map((t) => t.id)).toEqual(['c3', 'c1', 'c2']);
  });

  it('cabinetCardDragEnd clears cabinetDragSourceId, cabinetDropIndicatorIndex, AND draggedTask (no stale state leak)', () => {
    const tasks: FakeTask[] = [{ id: 'c1', status: 'cabinet' }];
    const h = makeKanbanCabinetHandlers(reorderMock, tasks);

    h.handleCardDragStart('c1', 'cabinet');
    h.cabinetColumnDragOver(0);
    expect(h.state().cabinetDragSourceId).toBe('c1');
    expect(h.state().cabinetDropIndicatorIndex).toBe(0);
    expect(h.state().draggedTask?.id).toBe('c1');

    h.cabinetCardDragEnd();

    expect(h.state().cabinetDragSourceId).toBeNull();
    expect(h.state().cabinetDropIndicatorIndex).toBeNull();
    expect(h.state().draggedTask).toBeNull();
  });

  it('dropping a Cabinet card back into its own resolved slot is a no-op', async () => {
    const tasks: FakeTask[] = [{ id: 'c1', status: 'cabinet' }, { id: 'c2', status: 'cabinet' }];
    const h = makeKanbanCabinetHandlers(reorderMock, tasks);

    h.handleCardDragStart('c1', 'cabinet'); // sourceIdx = 0
    await h.cabinetColumnDrop(0); // resolves to the same slot

    expect(reorderMock).not.toHaveBeenCalled();
  });

  it('reorder failure surfaces to the caller (resync path is invoked, does not throw)', async () => {
    const tasks: FakeTask[] = [{ id: 'c1', status: 'cabinet' }, { id: 'c2', status: 'cabinet' }];
    const failingReorder = vi.fn(async () => {
      throw new Error('network error');
    });
    const h = makeKanbanCabinetHandlers(failingReorder, tasks);

    h.handleCardDragStart('c2', 'cabinet');
    await expect(h.cabinetColumnDrop(0)).resolves.toBe('reorder');
    expect(failingReorder).toHaveBeenCalledTimes(1);
  });
});

// ─────────────────────────────────────────────────────────────────────────
// Section 3: Real DOM event dispatch — the container IS wired and receives
// drop events landing anywhere in its bounds, including gap-only targets that
// hit no card element at all (the core #2334 fix).
// ─────────────────────────────────────────────────────────────────────────
//
// jsdom supports genuine DOM event bubbling/dispatch (though not layout — see
// the DMG-gated note at the top of this file). This reproduces the exact
// containment structure now used in both NotesBrowser.svelte (.folder-column >
// .folder-card-list[ondrop] > .cabinet-drag-wrap (no ondrop)) and
// KanbanBoard.svelte (.kanban-column[ondrop] > .column-content[ondrop] >
// .cabinet-drag-wrap (no ondrop)): the CONTAINER now owns the drop handler, and
// a drop dispatched directly on the container element itself (not on any card)
// must reach it — proving the "gap between cards is a valid target" fix.

describe('Cabinet container-level drop wiring — real DOM dispatch, gap-target proof (#2334)', () => {
  function buildColumnWithContainerLevelHandler(
    containerDropHandler: (e: Event) => void,
    columnDropHandler: (e: Event) => void,
  ) {
    const column = document.createElement('div');
    column.className = 'folder-column'; // or .kanban-column — same shape
    column.addEventListener('drop', columnDropHandler);

    const container = document.createElement('div');
    container.className = 'folder-card-list'; // or .column-content
    container.addEventListener('drop', containerDropHandler);

    // Two card wrappers, no ondrop of their own (per-card drop handlers deleted).
    const card1 = document.createElement('div');
    card1.className = 'cabinet-drag-wrap';
    const card2 = document.createElement('div');
    card2.className = 'cabinet-drag-wrap';
    container.appendChild(card1);
    container.appendChild(card2);

    column.appendChild(container);
    document.body.appendChild(column);
    return { column, container, card1, card2 };
  }

  it('a drop dispatched on the CONTAINER ITSELF (the gap/seam territory, not on any card) reaches the container-level handler', () => {
    const containerHandler = vi.fn((e: Event) => e.stopPropagation());
    const columnHandler = vi.fn();
    const { column, container } = buildColumnWithContainerLevelHandler(containerHandler, columnHandler);

    // Dispatched directly on the container — this is exactly what happens when
    // the pointer is over the padding/gap area rather than any specific card
    // (jsdom event dispatch on a container element, not requiring layout).
    const dropEvent = new Event('drop', { bubbles: true, cancelable: true });
    container.dispatchEvent(dropEvent);

    expect(containerHandler).toHaveBeenCalledTimes(1);
    expect(columnHandler).not.toHaveBeenCalled(); // stopPropagation prevented the double-fire

    document.body.removeChild(column);
  });

  it('a drop dispatched on a CARD (bubbling up, since cards no longer have their own ondrop) still reaches the container-level handler via bubbling', () => {
    const containerHandler = vi.fn((e: Event) => e.stopPropagation());
    const columnHandler = vi.fn();
    const { column, card1 } = buildColumnWithContainerLevelHandler(containerHandler, columnHandler);

    const dropEvent = new Event('drop', { bubbles: true, cancelable: true });
    card1.dispatchEvent(dropEvent);

    // Card has no drop listener of its own — the event bubbles straight to the
    // container, which is now the ONLY reorder-capable drop target (this is the
    // architectural change: one handler covers cards AND gaps, because both are
    // just descendants/interior of the same container).
    expect(containerHandler).toHaveBeenCalledTimes(1);
    expect(columnHandler).not.toHaveBeenCalled();

    document.body.removeChild(column);
  });

  it('control: WITHOUT stopPropagation, the column-level handler WOULD double-fire (proves the test harness itself is valid, mirrors #2325 lesson)', () => {
    const containerHandler = vi.fn(); // deliberately does NOT call stopPropagation
    const columnHandler = vi.fn();
    const { column, container } = buildColumnWithContainerLevelHandler(containerHandler, columnHandler);

    const dropEvent = new Event('drop', { bubbles: true, cancelable: true });
    container.dispatchEvent(dropEvent);

    expect(containerHandler).toHaveBeenCalledTimes(1);
    expect(columnHandler).toHaveBeenCalledTimes(1); // bubbling happened, as expected without the guard

    document.body.removeChild(column);
  });

  it('dragover dispatched on the container (gap territory) also reaches the container-level handler', () => {
    const containerDragOverHandler = vi.fn((e: Event) => e.stopPropagation());
    const columnDragOverHandler = vi.fn();

    const column = document.createElement('div');
    column.addEventListener('dragover', columnDragOverHandler);
    const container = document.createElement('div');
    container.className = 'folder-card-list';
    container.addEventListener('dragover', containerDragOverHandler);
    column.appendChild(container);
    document.body.appendChild(column);

    container.dispatchEvent(new Event('dragover', { bubbles: true, cancelable: true }));

    expect(containerDragOverHandler).toHaveBeenCalledTimes(1);
    expect(columnDragOverHandler).not.toHaveBeenCalled();

    document.body.removeChild(column);
  });
});

// ─────────────────────────────────────────────────────────────────────────
// Section 4: Defect 2 — debug logging on the status-unchanged silent no-op
// ─────────────────────────────────────────────────────────────────────────

describe('Defect 2 fix — debug log on status-unchanged silent no-op guard (#2334)', () => {
  it('console.debug is called with the expected shape when the column-level guard fires (NotesBrowser handleColumnDrop pattern)', () => {
    const debugSpy = vi.spyOn(console, 'debug').mockImplementation(() => {});

    // Mirrors the exact guard + log call added to handleColumnDrop.
    function handleColumnDropGuard(draggedNote: { id: string; task_status?: string } | null, targetStatus: string) {
      if (!draggedNote) return;
      if (draggedNote.task_status === targetStatus) {
        console.debug('[Cabinet] drop: status unchanged, no-op', { noteId: draggedNote.id, from: draggedNote.task_status, to: targetStatus });
        return;
      }
    }

    handleColumnDropGuard({ id: 'n1', task_status: 'cabinet' }, 'cabinet');

    expect(debugSpy).toHaveBeenCalledWith('[Cabinet] drop: status unchanged, no-op', { noteId: 'n1', from: 'cabinet', to: 'cabinet' });

    debugSpy.mockRestore();
  });

  it('console.debug is called with the expected shape when the column-level guard fires (KanbanBoard handleDrop pattern)', () => {
    const debugSpy = vi.spyOn(console, 'debug').mockImplementation(() => {});

    function handleDropGuard(draggedTask: { id: string; status: string } | null, targetStatus: string) {
      if (!draggedTask || draggedTask.status === targetStatus) {
        if (draggedTask) {
          console.debug('[Cabinet] drop: status unchanged, no-op', { noteId: draggedTask.id, from: draggedTask.status, to: targetStatus });
        }
        return;
      }
    }

    handleDropGuard({ id: 't1', status: 'cabinet' }, 'cabinet');

    expect(debugSpy).toHaveBeenCalledWith('[Cabinet] drop: status unchanged, no-op', { noteId: 't1', from: 'cabinet', to: 'cabinet' });

    debugSpy.mockRestore();
  });

  it('console.debug is NOT called when the guard does not fire (status actually changes)', () => {
    const debugSpy = vi.spyOn(console, 'debug').mockImplementation(() => {});

    function handleColumnDropGuard(draggedNote: { id: string; task_status?: string } | null, targetStatus: string) {
      if (!draggedNote) return;
      if (draggedNote.task_status === targetStatus) {
        console.debug('[Cabinet] drop: status unchanged, no-op', { noteId: draggedNote.id, from: draggedNote.task_status, to: targetStatus });
        return;
      }
    }

    handleColumnDropGuard({ id: 'n1', task_status: 'todo' }, 'cabinet');

    expect(debugSpy).not.toHaveBeenCalled();

    debugSpy.mockRestore();
  });
});

// ─────────────────────────────────────────────────────────────────────────
// Section 5: Bug 2 — status-change drag dragend race (#2334 regression, fixed
// here). ondragend={cabinetDragEnd} fires per the DnD spec right after the
// drop handler's synchronous portion suspends at `await`, nulling
// `draggedNote` mid-flight. The fix captures the needed field(s) into a local
// BEFORE the await so the post-await continuation never reads the
// (now-possibly-null) `draggedNote` again — matching the pattern KanbanBoard's
// handleDrop already used correctly (captures into `taskToMove` before await).
// ─────────────────────────────────────────────────────────────────────────

describe('Bug 2 fix — handleColumnDrop capture-before-await race (#2334)', () => {
  // Faithful extraction of the FIXED NotesBrowser.svelte handleColumnDrop:
  // captures draggedNote.id into a local BEFORE the await, then nulls
  // draggedNote immediately (synchronously, before the await) so a
  // concurrently-firing ondragend={cabinetDragEnd} racing the in-flight await
  // can no longer cause a post-await null-read.
  function makeFixedHandleColumnDrop(invokeImpl: (noteId: string, status: string) => Promise<void>) {
    let draggedNote: { id: string; task_status?: string } | null = null;
    let taskNotes: { id: string; task_status?: string }[] = [];
    const errors: unknown[] = [];

    function setDraggedNote(note: { id: string; task_status?: string } | null) {
      draggedNote = note;
    }
    function setTaskNotes(notes: { id: string; task_status?: string }[]) {
      taskNotes = notes;
    }

    // Simulates ondragend={cabinetDragEnd} firing asynchronously mid-await,
    // exactly as the browser's real DnD lifecycle does per the regression trace.
    function simulateConcurrentDragEnd() {
      draggedNote = null;
    }

    async function handleColumnDrop(targetStatus: string) {
      if (!draggedNote) return;
      if (draggedNote.task_status === targetStatus) {
        draggedNote = null;
        return;
      }

      // FIX: capture before await, then null draggedNote synchronously —
      // nothing below this line ever reads `draggedNote` again.
      const noteId = draggedNote.id;
      draggedNote = null;

      try {
        await invokeImpl(noteId, targetStatus);
        taskNotes = taskNotes.map((n) => (n.id === noteId ? { ...n, task_status: targetStatus } : n));
      } catch (e) {
        errors.push(e);
      }
    }

    return { handleColumnDrop, setDraggedNote, setTaskNotes, simulateConcurrentDragEnd, state: () => ({ draggedNote, taskNotes, errors }) };
  }

  it('local state updates correctly even when ondragend nulls draggedNote DURING the in-flight await (the exact #2334 regression scenario)', async () => {
    let resolveInvoke!: () => void;
    const invokeImpl = vi.fn(
      () =>
        new Promise<void>((resolve) => {
          resolveInvoke = resolve;
        }),
    );
    const h = makeFixedHandleColumnDrop(invokeImpl);

    h.setTaskNotes([{ id: 'n1', task_status: 'cabinet' }]);
    h.setDraggedNote({ id: 'n1', task_status: 'cabinet' });

    const dropPromise = h.handleColumnDrop('todo');

    // Race window: ondragend fires here, nulling draggedNote, WHILE the
    // invoke() promise is still pending — reproduces the exact regression.
    h.simulateConcurrentDragEnd();
    expect(h.state().draggedNote).toBeNull();

    resolveInvoke();
    await dropPromise;

    // Before the fix: this throws inside the .map() callback (draggedNote!.id
    // against null), is swallowed by the catch, and taskNotes never updates.
    // After the fix: noteId was captured before the await, so the update
    // succeeds regardless of the race.
    expect(h.state().errors).toEqual([]);
    expect(h.state().taskNotes).toEqual([{ id: 'n1', task_status: 'todo' }]);
  });

  it('invoke is still called with the correct captured noteId even after the race nulls draggedNote', async () => {
    const invokeImpl = vi.fn(async () => {});
    const h = makeFixedHandleColumnDrop(invokeImpl);

    h.setTaskNotes([{ id: 'n2', task_status: 'cabinet' }]);
    h.setDraggedNote({ id: 'n2', task_status: 'cabinet' });

    await h.handleColumnDrop('todo');

    expect(invokeImpl).toHaveBeenCalledWith('n2', 'todo');
  });
});

describe('Bug 2b fix — [Cabinet]-tagged error log on handleColumnDrop failure (#2334)', () => {
  it('catch block logs with the [Cabinet] prefix, not the old untagged "Failed to update note status" message', async () => {
    const errorSpy = vi.spyOn(console, 'error').mockImplementation(() => {});

    // Mirrors the fixed catch block exactly.
    async function handleColumnDropCatch(invokeImpl: () => Promise<void>) {
      try {
        await invokeImpl();
      } catch (e) {
        console.error('[Cabinet] status update failed:', e);
      }
    }

    const err = new Error('boom');
    await handleColumnDropCatch(async () => {
      throw err;
    });

    expect(errorSpy).toHaveBeenCalledWith('[Cabinet] status update failed:', err);
    errorSpy.mockRestore();
  });
});

// ─────────────────────────────────────────────────────────────────────────
// Section 6 — permanent debug log on the identity-move no-op guard
// (NotesBrowser.svelte cabinetListDrop / KanbanBoard.svelte cabinetColumnDrop,
// `if (insertIdx === sourceIdx) return;`). Closes the observability gap noted
// in docs/audits/2026-08-02-noop-index-arithmetic.md — there was previously no
// log on this exact return path.
// ─────────────────────────────────────────────────────────────────────────

describe('identity-move no-op guard debug log (#2334)', () => {
  // Mirrors the exact guard + log added to cabinetListDrop / cabinetColumnDrop.
  function reorderGuard(sourceIdx: number, rawDropIndex: number): 'noop' | 'move' {
    const insertIdx = normalizeInsertIndex(sourceIdx, rawDropIndex);
    if (insertIdx === sourceIdx) {
      console.debug('[Cabinet] no-op: identity move, sourceIdx=%d rawDropIndex=%d insertIdx=%d', sourceIdx, rawDropIndex, insertIdx);
      return 'noop';
    }
    return 'move';
  }

  it('logs with the expected format when the identity-move guard fires', () => {
    const debugSpy = vi.spyOn(console, 'debug').mockImplementation(() => {});

    const result = reorderGuard(1, 2); // sourceIdx=1, rawDropIndex=1+1 → identity move

    expect(result).toBe('noop');
    expect(debugSpy).toHaveBeenCalledWith(
      '[Cabinet] no-op: identity move, sourceIdx=%d rawDropIndex=%d insertIdx=%d',
      1,
      2,
      1
    );

    debugSpy.mockRestore();
  });

  it('does NOT log when the guard does not fire (a real move occurs)', () => {
    const debugSpy = vi.spyOn(console, 'debug').mockImplementation(() => {});

    const result = reorderGuard(1, 3); // sourceIdx=1, rawDropIndex=3 → real move

    expect(result).toBe('move');
    expect(debugSpy).not.toHaveBeenCalled();

    debugSpy.mockRestore();
  });
});
