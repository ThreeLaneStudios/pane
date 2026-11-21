// sheetResize.ts — pure hit-zone/size math for column & row resize
// (#2339 PR 13, ShResize, §33.3.3, scope report
// `docs/audits/2026-09-04-sheets-pr13-resize-scope.md`).
//
// Pure, DOM-free, fully unit-tested (sheetResize.test.ts). No mutation of
// any input, no side effects — matches the established pattern of every
// other Sheets utility module (`sheetGridPivot.ts`, `sheetSelection.ts`,
// `sheetView.ts`, `sheetCellRender.ts`). DOM event wiring (`onmousedown`,
// `window.addEventListener('mousemove'/'mouseup')`) stays in
// `SheetGrid.svelte`, consuming these functions — see §7.1 of the scope
// report.

/** Minimum column width, in px — a floor on drag-time resizing (and thus
 *  on the readout's displayed value). Matt's ruling 2026-09-04: the design
 *  frame specifies no explicit min/max (§11 open question 2 of the scope
 *  report), but an unbounded-down-to-near-0 drag would make a column's
 *  ref/type glyph illegible — 40px is the named provisional floor,
 *  matching the gutter's own fixed 40px width by convention. Provisional
 *  pending any future design-phase revision. */
export const MIN_COLUMN_WIDTH = 40;

/** Minimum row height, in px — a floor on drag-time resizing (and thus on
 *  the readout's displayed value). Matt's ruling 2026-09-04, same
 *  rationale as `MIN_COLUMN_WIDTH`: a row dragged down to near-0px would
 *  clip cell content entirely with no way to recover it via the UI alone.
 *  24px is the named provisional floor. Provisional pending any future
 *  design-phase revision. */
export const MIN_ROW_HEIGHT = 24;

/** Hit-zone half-width, in px — the locked design CSS's `.sh-h__grip`/
 *  `.sh-g__grip` are 6px wide/tall, straddling the boundary 3px into each
 *  side (`right:-3px`/`bottom:-3px` on a 6px box). This constant is the
 *  zone's total width; detection checks `[boundary - HALF, boundary +
 *  HALF]` where `HALF = GRIP_ZONE_PX / 2`. */
const GRIP_ZONE_PX = 6;

/**
 * Given a cursor X position and an array of column boundary X positions
 * (the pixel position of each column's right edge, in the same coordinate
 * space as `cursorX`), returns the 0-indexed column whose boundary the
 * cursor is within the 6px hit zone of, or `null` if the cursor isn't
 * within any boundary's zone.
 *
 * When multiple boundaries are within the zone of each other (very narrow
 * columns), the closest boundary to `cursorX` wins — ties are broken by
 * the lower index (leftmost/earliest boundary).
 */
export function detectColumnGrip(cursorX: number, columnBoundaries: number[]): number | null {
  return detectGrip(cursorX, columnBoundaries);
}

/**
 * Given a cursor Y position and an array of row boundary Y positions (the
 * pixel position of each row's bottom edge), returns the 0-indexed row
 * whose boundary the cursor is within the 6px hit zone of, or `null` if
 * the cursor isn't within any boundary's zone. Same tie-break rule as
 * `detectColumnGrip`.
 */
export function detectRowGrip(cursorY: number, rowBoundaries: number[]): number | null {
  return detectGrip(cursorY, rowBoundaries);
}

function detectGrip(cursorPos: number, boundaries: number[]): number | null {
  const half = GRIP_ZONE_PX / 2;
  let best: number | null = null;
  let bestDist = Infinity;
  for (let i = 0; i < boundaries.length; i++) {
    const dist = Math.abs(cursorPos - boundaries[i]);
    if (dist <= half && dist < bestDist) {
      bestDist = dist;
      best = i;
    }
  }
  return best;
}

/**
 * Computes a new size (width or height) given the drag's starting size and
 * the cumulative pixel delta since drag start, clamped at `floor` (never
 * below it — the readout must never display below `MIN_COLUMN_WIDTH`/
 * `MIN_ROW_HEIGHT`, per Matt's ruling 2026-09-04 cited on those
 * constants). A zero delta returns `startSize` unchanged.
 */
export function computeNewSize(startSize: number, delta: number, floor: number): number {
  const raw = startSize + delta;
  return Math.max(floor, raw);
}

/**
 * Formats a pixel size for the drag-time readout chip, matching the
 * design's verbatim format: a bare integer followed by a space and the
 * literal unit `px` (e.g. `"124 px"`, `"40 px"`). Non-integer inputs are
 * rounded (the readout always displays whole pixels — `computeNewSize`
 * itself only ever produces integers when fed integer inputs, but this
 * guards against any future float-delta caller).
 */
export function formatReadout(sizePx: number): string {
  return `${Math.round(sizePx)} px`;
}
