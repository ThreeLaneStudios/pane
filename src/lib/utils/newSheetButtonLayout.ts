// #2339 PR 15c — New Sheet button pair, narrow-collapse breakpoint logic.
//
// Pure, DOM-free (same convention as sheetResize.ts / sheetView.ts / sheetCellRender.ts).
// Keyed to the HOST HEADER's own rendered width (via a ResizeObserver each call site
// attaches to its own header element), not the viewport — see
// docs/design/sheets/NOTES.md §A and README.md "Addendum rulings" #7:
//
//   >= 900px        both New Sheet and New Note labelled
//   720px – 900px   New Sheet drops its label, keeps its square 32px box
//   < 720px         New Sheet folds into a caret menu beside New Note
//
// The primary button (New Note) never gives way — it keeps its label at every width.

export type HeaderButtonMode = 'full' | 'icon' | 'menu';

/** >= this width: both buttons show their label. */
export const HEADER_WIDE_THRESHOLD = 900;
/** >= this width (and below HEADER_WIDE_THRESHOLD): New Sheet is icon-only. */
export const HEADER_NARROW_THRESHOLD = 720;

/**
 * Resolves the New Sheet / New Note button-pair layout mode from a header's
 * rendered width. Boundaries are inclusive on the wide side of each step,
 * matching the design's own inclusive "≥" phrasing.
 */
export function resolveHeaderButtonMode(headerWidth: number): HeaderButtonMode {
  if (headerWidth >= HEADER_WIDE_THRESHOLD) return 'full';
  if (headerWidth >= HEADER_NARROW_THRESHOLD) return 'icon';
  return 'menu';
}
