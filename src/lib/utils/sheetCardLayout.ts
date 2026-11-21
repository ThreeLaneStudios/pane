// #2339 PR 15a — Sheet card face, compact-variant breakpoint logic.
//
// Pure, DOM-free (same convention as newSheetButtonLayout.ts / sheetResize.ts
// / sheetView.ts / sheetCellRender.ts). Keyed to the CARD's own rendered
// width (via a ResizeObserver Card.svelte attaches to its own root element),
// not the viewport — matches the "width-not-viewport" convention
// newSheetButtonLayout.ts established for the header-width breakpoint. See
// docs/design/sheets/README.md §4 / build constraint §1: "below ~190px of
// card width the data rows drop and the card is glyph plus header strip."
//
//   >= 190px   full: glyph header row + 2 micro-grid data rows
//   <  190px   compact: glyph header row only, no data rows
//
// The 176px narrow-card design frame demonstrates this breakpoint; it is
// not a board column width (the live board's columns never go below the
// kit's 220px minimum) — see README build constraint §1.

export type SheetCardMode = 'full' | 'compact';

/** Below this rendered card width, the sheet card's micro-grid data rows
 *  drop and only the glyph + type-glyph header strip remain. */
export const SHEET_CARD_COMPACT_THRESHOLD = 190;

/**
 * Resolves the sheet card's full/compact mode from the card's own rendered
 * width. The boundary is inclusive on the full side, matching the design's
 * "below ~190px" phrasing — exactly 190px is still full.
 */
export function resolveSheetCardMode(cardWidth: number): SheetCardMode {
  return cardWidth >= SHEET_CARD_COMPACT_THRESHOLD ? 'full' : 'compact';
}
