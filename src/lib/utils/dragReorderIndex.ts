/**
 * #2334 — Container-level Cabinet reorder drop-index resolution.
 *
 * Pure coordinate math shared by NotesBrowser.svelte's Cabinet column
 * (.folder-card-list) and KanbanBoard.svelte's Cabinet column (.column-content).
 * One source of truth for "where will this land" — both the drop-index
 * resolution AND the drop indicator's render position derive from this same
 * function, per #2334's "single source of truth" requirement.
 *
 * DMG-gated note: jsdom does not perform real layout, so `getBoundingClientRect()`
 * on jsdom-rendered elements always returns a zeroed rect. This function itself
 * (given synthetic CardRect[] input) is fully unit-testable in jsdom; wiring it
 * up to real `Element.getBoundingClientRect()` calls at drop-time is DMG-only —
 * see CabinetDragReorder.test.ts and the PR body for the exact boundary.
 */

export interface CardRect {
  id: string;
  /** Top edge of the card, in the same coordinate space as the pointer Y (e.g. viewport px via getBoundingClientRect). */
  top: number;
  /** Rendered height of the card. */
  height: number;
}

/**
 * Given the pointer's Y position and the ordered list of rendered card rects,
 * resolve the insertion index: which "slot" (0..cards.length) the drop should
 * land in.
 *
 * Boundary rule: the pointer lands "above" a card once it crosses that card's
 * vertical midpoint. This matches the border-top seam-indicator convention
 * already used for the per-card highlight this feature replaces — dropping in
 * the top half of a card inserts before it; dropping in the bottom half (or in
 * the gap below it) inserts after it. Returns `cards.length` when the pointer
 * is below every card's midpoint (drop at list end).
 */
export function resolveDropIndex(clientY: number, cards: CardRect[]): number {
  for (let i = 0; i < cards.length; i++) {
    const midpoint = cards[i].top + cards[i].height / 2;
    if (clientY < midpoint) return i;
  }
  return cards.length;
}

/**
 * Given a source index and a raw resolved drop index (from resolveDropIndex,
 * computed against the list INCLUDING the dragged item), return the index the
 * item should actually be spliced into once it's removed from its original
 * position. Mirrors the splice-then-insert semantics used by the existing
 * splice(sourceIdx,1)+splice(targetIdx,0,moved) reorder pattern, but expressed
 * for a raw coordinate-resolved index rather than a target-card id.
 */
export function normalizeInsertIndex(sourceIdx: number, rawDropIndex: number): number {
  return rawDropIndex > sourceIdx ? rawDropIndex - 1 : rawDropIndex;
}

/**
 * #2334 (visual fix) — whether the drop indicator should render at a given
 * raw resolved drop index for a drag with the given source index.
 *
 * `normalizeInsertIndex` correctly treats `rawDropIndex === sourceIdx` and
 * `rawDropIndex === sourceIdx + 1` as identity moves (verified in
 * docs/audits/2026-08-02-noop-guard-verification.md — both slots round-trip
 * to the item's own current position under splice-remove-then-insert
 * semantics, so the no-op guard in cabinetListDrop/cabinetColumnDrop is
 * correct and must NOT change). The bug is purely visual: the indicator was
 * still being rendered at those two slots, implying to the user that a move
 * would happen when the drop is actually a silent no-op. This function is
 * the single source of truth both cabinet views use to suppress the
 * indicator at those two identity slots.
 */
export function shouldShowIndicator(sourceIdx: number, rawDropIndex: number): boolean {
  return rawDropIndex !== sourceIdx && rawDropIndex !== sourceIdx + 1;
}

/** Extracts a CardRect[] from an ordered list of DOM elements carrying a `data-cabinet-id` attribute. */
export function cardRectsFromElements(elements: ArrayLike<Element>): CardRect[] {
  const out: CardRect[] = [];
  for (const el of Array.from(elements)) {
    const id = (el as HTMLElement).dataset?.cabinetId;
    if (!id) continue;
    const rect = el.getBoundingClientRect();
    out.push({ id, top: rect.top, height: rect.height });
  }
  return out;
}
