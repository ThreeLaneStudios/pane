// sheetSelection.ts — selection / range / keyboard-nav reducer for the
// Sheets grid (#2339 PR 9, extended PR 10). Pure, DOM-free, fully
// unit-tested (sheetSelection.test.ts) — SheetGrid.svelte owns the DOM
// event listeners and calls `reduceSheetSelection(state, action)` to
// compute the next state; this module has zero knowledge of Svelte, the
// DOM, or rendering.
//
// PR 9 boundary (Matt's ruling 3), preserved by PR 10: this reducer is
// navigation/selection ONLY. There is no "commit value" action and no
// editing state — typing-to-commit, editor open/close, and ghost-commit
// orchestration all live in SheetGrid.svelte's own component state
// (#2339 PR 10 scope report §3). Ghost cells remain selectable/navigable
// like any other coordinate.
//
// PR 10 carry-forward fixes (scope report §5.1/§5.2, both binding):
//   - TAB wraps to the next/previous row at the row's column edge instead
//     of clamping in place.
//   - ENTER moves WITHIN an active range (cycling down, wrapping to the
//     next column at the range's bottom edge, back to the range's
//     top-left at the bottom-right cell) instead of always collapsing the
//     range and moving down one. The range is preserved across these
//     Enter presses — this is the one action where a fix changes an
//     existing action's return-shape contract (range is NOT nulled out).

/** A single grid coordinate. `row`/`col` are 0-indexed. Real cells and
 *  ghost cells share the same coordinate space — a ghost coordinate is any
 *  (row, col) at or past `rowCount`/`colCount` in the bounds the reducer
 *  was given, up to the ghost extent. */
export interface CellCoord {
  row: number;
  col: number;
}

/** An inclusive rectangular range, normalized so r0<=r1 and c0<=c1. */
export interface CellRange {
  r0: number;
  r1: number;
  c0: number;
  c1: number;
}

/** The grid's addressable bounds, supplied by the caller (SheetGrid.svelte)
 *  on every action so the reducer can clamp navigation without owning any
 *  data-loading concerns itself. `rowCount`/`colCount` are the REAL data
 *  extent; `ghostRowCount`/`ghostColCount` extend the navigable area for
 *  ghost cells (ruling 4 — ghost cells are visually present and, per the
 *  design's ShInContext frame, keyboard-navigable, just not typeable-into
 *  in PR 9). All navigation actions clamp to `[0, rowCount+ghostRowCount-1]`
 *  / `[0, colCount+ghostColCount-1]`. A bounds with `rowCount === 0` or
 *  `colCount === 0` (nothing to select) is guarded by `hasSelection` below,
 *  because a 0-column empty sheet still has ghost columns to navigate. */
export interface SheetBounds {
  rowCount: number;
  colCount: number;
  ghostRowCount: number;
  ghostColCount: number;
}

/** Reducer state: `anchor` is the single selected cell (always set once any
 *  selection exists); `range` is set only when a multi-cell range is
 *  active (drag/shift-extend/column-select/row-select) — a plain
 *  single-cell selection has `range: null`. `colSel`/`rowSel` mirror the
 *  design's header/gutter highlight state (`sh-h.is-sel` / `sh-g.is-sel`) —
 *  set to the selected column/row index when the *entire* column/row is
 *  selected via header/gutter click, `null` otherwise. */
export interface SheetSelectionState {
  anchor: CellCoord | null;
  range: CellRange | null;
  colSel: number | null;
  rowSel: number | null;
}

export type SheetSelectionAction =
  | { type: 'SELECT_CELL'; coord: CellCoord }
  | { type: 'EXTEND_TO'; coord: CellCoord }
  | { type: 'SELECT_COLUMN'; col: number }
  | { type: 'SELECT_ROW'; row: number }
  | { type: 'MOVE'; direction: 'up' | 'down' | 'left' | 'right'; extend?: boolean; jumpToEdge?: boolean }
  | { type: 'TAB'; shift?: boolean }
  | { type: 'ENTER' }
  | { type: 'ESCAPE' };

/** A1-selected default state — the initial selection state for a freshly
 *  loaded/created sheet (README ruling 1: "New sheet appearance ... A1
 *  pre-selected"). */
export const INITIAL_SHEET_SELECTION: SheetSelectionState = {
  anchor: { row: 0, col: 0 },
  range: null,
  colSel: null,
  rowSel: null,
};

function clamp(n: number, min: number, max: number): number {
  return Math.max(min, Math.min(max, n));
}

function maxRow(bounds: SheetBounds): number {
  return Math.max(0, bounds.rowCount + bounds.ghostRowCount - 1);
}

function maxCol(bounds: SheetBounds): number {
  return Math.max(0, bounds.colCount + bounds.ghostColCount - 1);
}

/** Clamps a coordinate to the grid's addressable bounds (real + ghost
 *  extent, per `SheetBounds`). Exported (#2339 PR 11) so `SheetGrid.svelte`
 *  can reference the SAME clamping style for column-delete selection
 *  recovery (ruling 4) — though that call site clamps to the REAL column
 *  extent only (never landing on a ghost column after a delete), so it
 *  implements its own narrower min/max rather than calling this function
 *  directly. Kept here, exported, as the canonical bounds-clamp pattern. */
export function clampCoord(coord: CellCoord, bounds: SheetBounds): CellCoord {
  return {
    row: clamp(coord.row, 0, maxRow(bounds)),
    col: clamp(coord.col, 0, maxCol(bounds)),
  };
}

function normalizeRange(a: CellCoord, b: CellCoord): CellRange {
  return {
    r0: Math.min(a.row, b.row),
    r1: Math.max(a.row, b.row),
    c0: Math.min(a.col, b.col),
    c1: Math.max(a.col, b.col),
  };
}

/**
 * The Sheets selection/navigation reducer. Given the current state, an
 * action, and the grid's addressable bounds, returns the NEXT state — never
 * mutates `state` in place.
 *
 * Keyboard-nav table implemented here (per the PR spec):
 *   - Arrow keys: move anchor one cell, collapsing any active range.
 *   - Shift+Arrow: extend/contract the range from the (fixed) anchor.
 *   - Tab / Shift+Tab: move right / left, collapsing any active range.
 *   - Enter: move down, collapsing any active range.
 *   - ⌘/Ctrl+Arrow: jump to the edge of the data region in that direction
 *     (mirrors spreadsheet "jump to edge of data" — the last real,
 *     non-ghost coordinate in that direction, or the grid boundary if
 *     already there / no further real data exists).
 *   - Esc: collapse an active range to the anchor; if no range is active,
 *     clears the selection entirely (anchor -> null).
 */
export function reduceSheetSelection(
  state: SheetSelectionState,
  action: SheetSelectionAction,
  bounds: SheetBounds
): SheetSelectionState {
  switch (action.type) {
    case 'SELECT_CELL': {
      const coord = clampCoord(action.coord, bounds);
      return { anchor: coord, range: null, colSel: null, rowSel: null };
    }

    case 'EXTEND_TO': {
      if (!state.anchor) {
        const coord = clampCoord(action.coord, bounds);
        return { anchor: coord, range: null, colSel: null, rowSel: null };
      }
      const coord = clampCoord(action.coord, bounds);
      const range = normalizeRange(state.anchor, coord);
      const isSingleCell = range.r0 === range.r1 && range.c0 === range.c1;
      return { anchor: state.anchor, range: isSingleCell ? null : range, colSel: null, rowSel: null };
    }

    case 'SELECT_COLUMN': {
      const col = clamp(action.col, 0, maxCol(bounds));
      const lastRow = maxRow(bounds);
      const anchor: CellCoord = { row: 0, col };
      const range: CellRange | null =
        lastRow > 0 ? { r0: 0, r1: lastRow, c0: col, c1: col } : null;
      return { anchor, range, colSel: col, rowSel: null };
    }

    case 'SELECT_ROW': {
      const row = clamp(action.row, 0, maxRow(bounds));
      const lastCol = maxCol(bounds);
      const anchor: CellCoord = { row, col: 0 };
      const range: CellRange | null =
        lastCol > 0 ? { r0: row, r1: row, c0: 0, c1: lastCol } : null;
      return { anchor, range, colSel: null, rowSel: row };
    }

    case 'MOVE': {
      if (!state.anchor) {
        return { anchor: clampCoord({ row: 0, col: 0 }, bounds), range: null, colSel: null, rowSel: null };
      }

      if (action.extend) {
        // The extension point ("focus") is the far corner of the active range
        // (the corner that is NOT the anchor), or the anchor itself if no
        // range is active yet. Moving the focus incrementally — rather than
        // always recomputing one step from the fixed anchor — is what makes
        // repeated shift+arrow presses grow the selection cumulatively,
        // matching standard spreadsheet behavior.
        const focus = extensionFocus(state);
        const delta = directionDelta(action.direction);
        const nextFocus = action.jumpToEdge
          ? jumpToDataEdge(focus, action.direction, bounds)
          : { row: focus.row + delta.row, col: focus.col + delta.col };
        const clampedFocus = clampCoord(nextFocus, bounds);
        const range = normalizeRange(state.anchor, clampedFocus);
        const isSingleCell = range.r0 === range.r1 && range.c0 === range.c1;
        return { anchor: state.anchor, range: isSingleCell ? null : range, colSel: null, rowSel: null };
      }

      const delta = directionDelta(action.direction);
      const target = clampCoord(
        action.jumpToEdge
          ? jumpToDataEdge(state.anchor, action.direction, bounds)
          : { row: state.anchor.row + delta.row, col: state.anchor.col + delta.col },
        bounds
      );
      return { anchor: target, range: null, colSel: null, rowSel: null };
    }

    case 'TAB': {
      if (!state.anchor) {
        return { anchor: clampCoord({ row: 0, col: 0 }, bounds), range: null, colSel: null, rowSel: null };
      }
      // #2339 PR 10 carry-forward fix (§5.1): Tab wraps to column 0 of the
      // next row at the right edge; Shift+Tab wraps to the last column of
      // the previous row at the left edge. Clamps at the grid's row extent
      // exactly like any other row-overflow — no wrap past the last ghost
      // row, no wrap-around back to A1 at the absolute bottom-right corner.
      const lastCol = maxCol(bounds);
      let target: CellCoord;
      if (action.shift) {
        if (state.anchor.col <= 0) {
          target = clampCoord({ row: state.anchor.row - 1, col: lastCol }, bounds);
        } else {
          target = { row: state.anchor.row, col: state.anchor.col - 1 };
        }
      } else {
        if (state.anchor.col >= lastCol) {
          target = clampCoord({ row: state.anchor.row + 1, col: 0 }, bounds);
        } else {
          target = { row: state.anchor.row, col: state.anchor.col + 1 };
        }
      }
      return { anchor: clampCoord(target, bounds), range: null, colSel: null, rowSel: null };
    }

    case 'ENTER': {
      if (!state.anchor) {
        return { anchor: clampCoord({ row: 0, col: 0 }, bounds), range: null, colSel: null, rowSel: null };
      }
      // #2339 PR 10 carry-forward fix (§5.2): with an active range, Enter
      // moves WITHIN the range — down the anchor's column, wrapping to the
      // next column (back to the range's top row) at the range's bottom
      // edge, and clamping (not wrapping out) at the range's right edge, so
      // the full cycle lands back at the range's top-left cell. The range
      // is preserved (NOT nulled) across these presses — this is the one
      // reducer branch that keeps `state.range` on an ENTER action.
      if (state.range) {
        const r = state.range;
        let nextRow = state.anchor.row + 1;
        let nextCol = state.anchor.col;
        if (nextRow > r.r1) {
          nextRow = r.r0;
          nextCol = state.anchor.col < r.c1 ? state.anchor.col + 1 : r.c0;
        }
        return { anchor: { row: nextRow, col: nextCol }, range: r, colSel: state.colSel, rowSel: state.rowSel };
      }
      const target = clampCoord({ row: state.anchor.row + 1, col: state.anchor.col }, bounds);
      return { anchor: target, range: null, colSel: null, rowSel: null };
    }

    case 'ESCAPE': {
      if (state.range || state.colSel !== null || state.rowSel !== null) {
        return { anchor: state.anchor, range: null, colSel: null, rowSel: null };
      }
      return { anchor: null, range: null, colSel: null, rowSel: null };
    }

    default:
      return state;
  }
}

/** The corner of the active range farthest from the anchor — i.e. the point
 *  a shift+arrow/shift+click extension moves next. Falls back to the
 *  anchor itself when no range is active (the very first shift+arrow
 *  press). Assumes `state.anchor` is non-null (only called from that
 *  branch in the reducer above). */
function extensionFocus(state: SheetSelectionState): CellCoord {
  const anchor = state.anchor as CellCoord;
  if (!state.range) return anchor;
  const farRow = state.range.r0 === anchor.row ? state.range.r1 : state.range.r0;
  const farCol = state.range.c0 === anchor.col ? state.range.c1 : state.range.c0;
  return { row: farRow, col: farCol };
}

function directionDelta(direction: 'up' | 'down' | 'left' | 'right'): CellCoord {
  switch (direction) {
    case 'up': return { row: -1, col: 0 };
    case 'down': return { row: 1, col: 0 };
    case 'left': return { row: 0, col: -1 };
    case 'right': return { row: 0, col: 1 };
  }
}

/**
 * ⌘/Ctrl+Arrow "jump to edge of data region" — jumps to the last REAL
 * (non-ghost) coordinate in the given direction, or the grid boundary
 * (including ghost extent) if the anchor is already at/past the real data
 * edge in that direction. This mirrors standard spreadsheet behavior: from
 * inside real data, jump to the edge of the contiguous data block; from the
 * edge (or beyond, e.g. sitting in ghost space), jump the rest of the way
 * to the grid boundary.
 *
 * PR 9 has no per-cell occupancy signal in the reducer (that's a rendering
 * concern owned by the pivoted grid, not this pure module) — "edge of data"
 * here is defined structurally as the last REAL row/column index
 * (`rowCount - 1` / `colCount - 1`), matching the common case where jump
 * lands on the boundary between real and ghost space. This is a deliberate,
 * documented simplification: true occupancy-aware jumping (skip empty
 * interior cells) is not required by the PR 9 spec and is out of scope.
 */
function jumpToDataEdge(
  from: CellCoord,
  direction: 'up' | 'down' | 'left' | 'right',
  bounds: SheetBounds
): CellCoord {
  const lastRealRow = Math.max(0, bounds.rowCount - 1);
  const lastRealCol = Math.max(0, bounds.colCount - 1);
  const lastRow = maxRow(bounds);
  const lastCol = maxCol(bounds);

  switch (direction) {
    case 'up':
      return { row: 0, col: from.col };
    case 'down':
      return { row: from.row < lastRealRow ? lastRealRow : lastRow, col: from.col };
    case 'left':
      return { row: from.row, col: 0 };
    case 'right':
      return { row: from.row, col: from.col < lastRealCol ? lastRealCol : lastCol };
  }
}

/** True if a coordinate falls within an active range (inclusive). */
export function isInRange(coord: CellCoord, range: CellRange | null): boolean {
  if (!range) return false;
  return (
    coord.row >= range.r0 && coord.row <= range.r1 && coord.col >= range.c0 && coord.col <= range.c1
  );
}

/** True if a coordinate is the current anchor. */
export function isAnchor(coord: CellCoord, anchor: CellCoord | null): boolean {
  return !!anchor && anchor.row === coord.row && anchor.col === coord.col;
}

/** Which range edges (if any) a coordinate sits on — used to apply the
 *  design's `is-redge-t/b/l/r` border classes. Returns an object with each
 *  edge flag `false` when `range` is null or the coordinate isn't in it. */
export function rangeEdges(
  coord: CellCoord,
  range: CellRange | null
): { top: boolean; bottom: boolean; left: boolean; right: boolean } {
  if (!isInRange(coord, range) || !range) {
    return { top: false, bottom: false, left: false, right: false };
  }
  return {
    top: coord.row === range.r0,
    bottom: coord.row === range.r1,
    left: coord.col === range.c0,
    right: coord.col === range.c1,
  };
}
