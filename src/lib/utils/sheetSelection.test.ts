// sheetSelection.test.ts — unit tests for the Sheets selection/keyboard-nav
// reducer (#2339 PR 9). Every test drives the real `reduceSheetSelection`
// reducer with real state/action/bounds objects — no DOM, no mocked
// reducer internals.

import { describe, it, expect } from 'vitest';
import {
  reduceSheetSelection,
  INITIAL_SHEET_SELECTION,
  isInRange,
  isAnchor,
  rangeEdges,
  type SheetSelectionState,
  type SheetBounds,
} from './sheetSelection';

// A representative 4-column x 5-row real grid, with 3 ghost columns and
// 4 ghost rows past the real data — matches the PR spec's ghost extent.
const BOUNDS: SheetBounds = { rowCount: 5, colCount: 4, ghostRowCount: 4, ghostColCount: 3 };

const EMPTY_STATE: SheetSelectionState = { anchor: null, range: null, colSel: null, rowSel: null };

describe('INITIAL_SHEET_SELECTION', () => {
  it('defaults to A1 (row 0, col 0) selected, no range', () => {
    expect(INITIAL_SHEET_SELECTION.anchor).toEqual({ row: 0, col: 0 });
    expect(INITIAL_SHEET_SELECTION.range).toBeNull();
    expect(INITIAL_SHEET_SELECTION.colSel).toBeNull();
    expect(INITIAL_SHEET_SELECTION.rowSel).toBeNull();
  });
});

describe('SELECT_CELL', () => {
  it('sets the anchor to the clicked cell and clears any range', () => {
    const withRange: SheetSelectionState = {
      anchor: { row: 0, col: 0 },
      range: { r0: 0, r1: 1, c0: 0, c1: 1 },
      colSel: null,
      rowSel: null,
    };
    const next = reduceSheetSelection(withRange, { type: 'SELECT_CELL', coord: { row: 2, col: 3 } }, BOUNDS);
    expect(next.anchor).toEqual({ row: 2, col: 3 });
    expect(next.range).toBeNull();
  });

  it('clamps a coordinate outside the ghost-extended bounds', () => {
    const next = reduceSheetSelection(EMPTY_STATE, { type: 'SELECT_CELL', coord: { row: 999, col: 999 } }, BOUNDS);
    // maxRow = 5+4-1 = 8, maxCol = 4+3-1 = 6
    expect(next.anchor).toEqual({ row: 8, col: 6 });
  });

  it('allows selecting a negative coordinate clamped to 0', () => {
    const next = reduceSheetSelection(EMPTY_STATE, { type: 'SELECT_CELL', coord: { row: -5, col: -2 } }, BOUNDS);
    expect(next.anchor).toEqual({ row: 0, col: 0 });
  });
});

describe('MOVE — arrow key movement with bounds checking', () => {
  it('moves the anchor one cell right', () => {
    const state = { ...INITIAL_SHEET_SELECTION };
    const next = reduceSheetSelection(state, { type: 'MOVE', direction: 'right' }, BOUNDS);
    expect(next.anchor).toEqual({ row: 0, col: 1 });
  });

  it('moves the anchor one cell down', () => {
    const state = { ...INITIAL_SHEET_SELECTION };
    const next = reduceSheetSelection(state, { type: 'MOVE', direction: 'down' }, BOUNDS);
    expect(next.anchor).toEqual({ row: 1, col: 0 });
  });

  it('clamps at the left edge (col 0) — left from A1 stays at A1', () => {
    const next = reduceSheetSelection(INITIAL_SHEET_SELECTION, { type: 'MOVE', direction: 'left' }, BOUNDS);
    expect(next.anchor).toEqual({ row: 0, col: 0 });
  });

  it('clamps at the top edge (row 0) — up from A1 stays at A1', () => {
    const next = reduceSheetSelection(INITIAL_SHEET_SELECTION, { type: 'MOVE', direction: 'up' }, BOUNDS);
    expect(next.anchor).toEqual({ row: 0, col: 0 });
  });

  it('clamps at the bottom edge, including ghost rows (maxRow = rowCount+ghostRowCount-1)', () => {
    const state: SheetSelectionState = { anchor: { row: 8, col: 0 }, range: null, colSel: null, rowSel: null };
    const next = reduceSheetSelection(state, { type: 'MOVE', direction: 'down' }, BOUNDS);
    expect(next.anchor).toEqual({ row: 8, col: 0 });
  });

  it('clamps at the right edge, including ghost columns (maxCol = colCount+ghostColCount-1)', () => {
    const state: SheetSelectionState = { anchor: { row: 0, col: 6 }, range: null, colSel: null, rowSel: null };
    const next = reduceSheetSelection(state, { type: 'MOVE', direction: 'right' }, BOUNDS);
    expect(next.anchor).toEqual({ row: 0, col: 6 });
  });

  it('moving without an existing anchor snaps to the clamped origin', () => {
    const next = reduceSheetSelection(EMPTY_STATE, { type: 'MOVE', direction: 'right' }, BOUNDS);
    expect(next.anchor).toEqual({ row: 0, col: 0 });
  });

  it('a plain move collapses any active range', () => {
    const state: SheetSelectionState = {
      anchor: { row: 0, col: 0 },
      range: { r0: 0, r1: 1, c0: 0, c1: 1 },
      colSel: null,
      rowSel: null,
    };
    const next = reduceSheetSelection(state, { type: 'MOVE', direction: 'down' }, BOUNDS);
    expect(next.range).toBeNull();
  });
});

describe('MOVE with extend — shift+arrow range extension', () => {
  it('extends a range one cell right of the anchor', () => {
    const state: SheetSelectionState = { anchor: { row: 1, col: 1 }, range: null, colSel: null, rowSel: null };
    const next = reduceSheetSelection(state, { type: 'MOVE', direction: 'right', extend: true }, BOUNDS);
    expect(next.anchor).toEqual({ row: 1, col: 1 });
    expect(next.range).toEqual({ r0: 1, r1: 1, c0: 1, c1: 2 });
  });

  it('extending back to the anchor cell collapses the range to null (single cell)', () => {
    const state: SheetSelectionState = { anchor: { row: 1, col: 1 }, range: { r0: 1, r1: 1, c0: 1, c1: 2 }, colSel: null, rowSel: null };
    const next = reduceSheetSelection(state, { type: 'MOVE', direction: 'left', extend: true }, BOUNDS);
    expect(next.range).toBeNull();
  });

  it('shift+down then shift+right grows a 2D range from the anchor', () => {
    let state: SheetSelectionState = { anchor: { row: 1, col: 1 }, range: null, colSel: null, rowSel: null };
    state = reduceSheetSelection(state, { type: 'MOVE', direction: 'down', extend: true }, BOUNDS);
    state = reduceSheetSelection(state, { type: 'MOVE', direction: 'right', extend: true }, BOUNDS);
    expect(state.anchor).toEqual({ row: 1, col: 1 });
    expect(state.range).toEqual({ r0: 1, r1: 2, c0: 1, c1: 2 });
  });

  it('extending contracts correctly when moving back past the anchor in the other direction', () => {
    let state: SheetSelectionState = { anchor: { row: 2, col: 2 }, range: null, colSel: null, rowSel: null };
    state = reduceSheetSelection(state, { type: 'MOVE', direction: 'up', extend: true }, BOUNDS);
    expect(state.range).toEqual({ r0: 1, r1: 2, c0: 2, c1: 2 });
    // extend back down past the anchor -> range flips below the anchor
    state = reduceSheetSelection(state, { type: 'MOVE', direction: 'down', extend: true }, BOUNDS);
    state = reduceSheetSelection(state, { type: 'MOVE', direction: 'down', extend: true }, BOUNDS);
    expect(state.range).toEqual({ r0: 2, r1: 3, c0: 2, c1: 2 });
  });

  it('clears colSel/rowSel when extending after a column/row select', () => {
    let state: SheetSelectionState = { anchor: { row: 0, col: 1 }, range: { r0: 0, r1: 8, c0: 1, c1: 1 }, colSel: 1, rowSel: null };
    state = reduceSheetSelection(state, { type: 'MOVE', direction: 'right', extend: true }, BOUNDS);
    expect(state.colSel).toBeNull();
  });
});

describe('TAB / Shift+Tab movement', () => {
  it('Tab moves right, collapsing any range', () => {
    const state: SheetSelectionState = { anchor: { row: 0, col: 0 }, range: { r0: 0, r1: 1, c0: 0, c1: 1 }, colSel: null, rowSel: null };
    const next = reduceSheetSelection(state, { type: 'TAB' }, BOUNDS);
    expect(next.anchor).toEqual({ row: 0, col: 1 });
    expect(next.range).toBeNull();
  });

  it('Shift+Tab moves left', () => {
    const state: SheetSelectionState = { anchor: { row: 0, col: 2 }, range: null, colSel: null, rowSel: null };
    const next = reduceSheetSelection(state, { type: 'TAB', shift: true }, BOUNDS);
    expect(next.anchor).toEqual({ row: 0, col: 1 });
  });

  it('Tab with no prior anchor snaps to the origin', () => {
    const next = reduceSheetSelection(EMPTY_STATE, { type: 'TAB' }, BOUNDS);
    expect(next.anchor).toEqual({ row: 0, col: 0 });
  });

  // #2339 PR 10 carry-forward fix (§5.1): Tab wraps to the next/previous
  // row at the row/column edges instead of clamping in place.
  it('Tab at the last column wraps to column 0 of the next row', () => {
    const state: SheetSelectionState = { anchor: { row: 2, col: 6 }, range: null, colSel: null, rowSel: null };
    const next = reduceSheetSelection(state, { type: 'TAB' }, BOUNDS);
    expect(next.anchor).toEqual({ row: 3, col: 0 });
  });

  it('Shift+Tab at column 0 wraps to the last column of the previous row', () => {
    const state: SheetSelectionState = { anchor: { row: 2, col: 0 }, range: null, colSel: null, rowSel: null };
    const next = reduceSheetSelection(state, { type: 'TAB', shift: true }, BOUNDS);
    expect(next.anchor).toEqual({ row: 1, col: 6 });
  });

  it('Tab at the grid\'s absolute bottom-right corner clamps the row (no wrap past the last ghost row), landing at column 0', () => {
    // Target before clamp is { row: row+1, col: 0 } (per the scope report's
    // own fix description) — the row clamps at the grid's last ghost row
    // rather than wrapping around to row 0, but the column-reset-to-0 still
    // happens (that's the "next row" part of the wrap, not something to
    // undo just because the row itself couldn't advance).
    const state: SheetSelectionState = { anchor: { row: 8, col: 6 }, range: null, colSel: null, rowSel: null };
    const next = reduceSheetSelection(state, { type: 'TAB' }, BOUNDS);
    expect(next.anchor).toEqual({ row: 8, col: 0 });
  });

  it('Shift+Tab at the grid\'s absolute top-left corner clamps at row 0 but still moves to the last column', () => {
    const state: SheetSelectionState = { anchor: { row: 0, col: 0 }, range: null, colSel: null, rowSel: null };
    const next = reduceSheetSelection(state, { type: 'TAB', shift: true }, BOUNDS);
    expect(next.anchor).toEqual({ row: 0, col: 6 });
  });

  it('Tab within a row (not at the edge) still just moves right by one', () => {
    const state: SheetSelectionState = { anchor: { row: 1, col: 2 }, range: null, colSel: null, rowSel: null };
    const next = reduceSheetSelection(state, { type: 'TAB' }, BOUNDS);
    expect(next.anchor).toEqual({ row: 1, col: 3 });
  });
});

describe('ENTER movement — no active range (unchanged behavior)', () => {
  it('moves the anchor down one row', () => {
    const state: SheetSelectionState = { anchor: { row: 0, col: 0 }, range: null, colSel: null, rowSel: null };
    const next = reduceSheetSelection(state, { type: 'ENTER' }, BOUNDS);
    expect(next.anchor).toEqual({ row: 1, col: 0 });
    expect(next.range).toBeNull();
  });

  it('clamps at the bottom edge', () => {
    const state: SheetSelectionState = { anchor: { row: 8, col: 0 }, range: null, colSel: null, rowSel: null };
    const next = reduceSheetSelection(state, { type: 'ENTER' }, BOUNDS);
    expect(next.anchor).toEqual({ row: 8, col: 0 });
  });

  it('Enter with no prior anchor snaps to the origin', () => {
    const next = reduceSheetSelection(EMPTY_STATE, { type: 'ENTER' }, BOUNDS);
    expect(next.anchor).toEqual({ row: 0, col: 0 });
  });
});

// #2339 PR 10 carry-forward fix (§5.2): with an active range, Enter cycles
// down within the range's column, wrapping to the next column at the
// range's bottom edge, back to the top-left at the range's bottom-right
// cell — and the range is PRESERVED across these presses (not nulled).
describe('ENTER movement — within an active range', () => {
  it('moves down within the range column, keeping the range', () => {
    const range = { r0: 0, r1: 1, c0: 0, c1: 1 };
    const state: SheetSelectionState = { anchor: { row: 0, col: 0 }, range, colSel: null, rowSel: null };
    const next = reduceSheetSelection(state, { type: 'ENTER' }, BOUNDS);
    expect(next.anchor).toEqual({ row: 1, col: 0 });
    expect(next.range).toEqual(range);
  });

  it('wraps to the top of the next column at the range\'s bottom edge, keeping the range', () => {
    const range = { r0: 0, r1: 1, c0: 0, c1: 1 };
    const state: SheetSelectionState = { anchor: { row: 1, col: 0 }, range, colSel: null, rowSel: null };
    const next = reduceSheetSelection(state, { type: 'ENTER' }, BOUNDS);
    expect(next.anchor).toEqual({ row: 0, col: 1 });
    expect(next.range).toEqual(range);
  });

  it('a full cycle in a 2x2 range returns to the top-left cell', () => {
    const range = { r0: 0, r1: 1, c0: 0, c1: 1 };
    let state: SheetSelectionState = { anchor: { row: 0, col: 0 }, range, colSel: null, rowSel: null };
    state = reduceSheetSelection(state, { type: 'ENTER' }, BOUNDS); // -> (1,0)
    state = reduceSheetSelection(state, { type: 'ENTER' }, BOUNDS); // -> (0,1)
    state = reduceSheetSelection(state, { type: 'ENTER' }, BOUNDS); // -> (1,1)
    expect(state.anchor).toEqual({ row: 1, col: 1 });
    state = reduceSheetSelection(state, { type: 'ENTER' }, BOUNDS); // wraps back to top-left
    expect(state.anchor).toEqual({ row: 0, col: 0 });
    expect(state.range).toEqual(range);
  });

  it('at the range\'s bottom-right cell, wraps back to the range\'s top-left cell', () => {
    const range = { r0: 2, r1: 3, c0: 2, c1: 3 };
    const state: SheetSelectionState = { anchor: { row: 3, col: 3 }, range, colSel: null, rowSel: null };
    const next = reduceSheetSelection(state, { type: 'ENTER' }, BOUNDS);
    expect(next.anchor).toEqual({ row: 2, col: 2 });
    expect(next.range).toEqual(range);
  });

  it('a single-row range (r0 === r1) always wraps down to the same row, next column', () => {
    const range = { r0: 2, r1: 2, c0: 0, c1: 2 };
    const state: SheetSelectionState = { anchor: { row: 2, col: 0 }, range, colSel: null, rowSel: null };
    const next = reduceSheetSelection(state, { type: 'ENTER' }, BOUNDS);
    expect(next.anchor).toEqual({ row: 2, col: 1 });
    expect(next.range).toEqual(range);
  });

  it('preserves colSel/rowSel across an in-range Enter (e.g. a whole-column selection)', () => {
    const range = { r0: 0, r1: 8, c0: 1, c1: 1 };
    const state: SheetSelectionState = { anchor: { row: 0, col: 1 }, range, colSel: 1, rowSel: null };
    const next = reduceSheetSelection(state, { type: 'ENTER' }, BOUNDS);
    expect(next.anchor).toEqual({ row: 1, col: 1 });
    expect(next.range).toEqual(range);
    expect(next.colSel).toBe(1);
  });
});

describe('⌘/Ctrl+Arrow jump to edge of data', () => {
  it('jumps down to the last real row from inside real data', () => {
    const state: SheetSelectionState = { anchor: { row: 0, col: 0 }, range: null, colSel: null, rowSel: null };
    const next = reduceSheetSelection(state, { type: 'MOVE', direction: 'down', jumpToEdge: true }, BOUNDS);
    // rowCount=5 -> lastRealRow = 4
    expect(next.anchor).toEqual({ row: 4, col: 0 });
  });

  it('jumps to the grid boundary (through ghost space) when already at the real data edge', () => {
    const state: SheetSelectionState = { anchor: { row: 4, col: 0 }, range: null, colSel: null, rowSel: null };
    const next = reduceSheetSelection(state, { type: 'MOVE', direction: 'down', jumpToEdge: true }, BOUNDS);
    // maxRow = 5+4-1 = 8
    expect(next.anchor).toEqual({ row: 8, col: 0 });
  });

  it('jumps right to the last real column from inside real data', () => {
    const state: SheetSelectionState = { anchor: { row: 0, col: 0 }, range: null, colSel: null, rowSel: null };
    const next = reduceSheetSelection(state, { type: 'MOVE', direction: 'right', jumpToEdge: true }, BOUNDS);
    // colCount=4 -> lastRealCol = 3
    expect(next.anchor).toEqual({ row: 0, col: 3 });
  });

  it('jumps up to row 0 unconditionally', () => {
    const state: SheetSelectionState = { anchor: { row: 3, col: 2 }, range: null, colSel: null, rowSel: null };
    const next = reduceSheetSelection(state, { type: 'MOVE', direction: 'up', jumpToEdge: true }, BOUNDS);
    expect(next.anchor).toEqual({ row: 0, col: 2 });
  });

  it('jumps left to col 0 unconditionally', () => {
    const state: SheetSelectionState = { anchor: { row: 3, col: 2 }, range: null, colSel: null, rowSel: null };
    const next = reduceSheetSelection(state, { type: 'MOVE', direction: 'left', jumpToEdge: true }, BOUNDS);
    expect(next.anchor).toEqual({ row: 3, col: 0 });
  });
});

describe('ESCAPE', () => {
  it('collapses an active range to just the anchor', () => {
    const state: SheetSelectionState = { anchor: { row: 0, col: 0 }, range: { r0: 0, r1: 2, c0: 0, c1: 2 }, colSel: null, rowSel: null };
    const next = reduceSheetSelection(state, { type: 'ESCAPE' }, BOUNDS);
    expect(next.anchor).toEqual({ row: 0, col: 0 });
    expect(next.range).toBeNull();
  });

  it('deselects entirely when there is no active range', () => {
    const state: SheetSelectionState = { anchor: { row: 2, col: 2 }, range: null, colSel: null, rowSel: null };
    const next = reduceSheetSelection(state, { type: 'ESCAPE' }, BOUNDS);
    expect(next.anchor).toBeNull();
  });

  it('clears an active column selection first (does not fully deselect in one Esc)', () => {
    const state: SheetSelectionState = { anchor: { row: 0, col: 1 }, range: { r0: 0, r1: 8, c0: 1, c1: 1 }, colSel: 1, rowSel: null };
    const next = reduceSheetSelection(state, { type: 'ESCAPE' }, BOUNDS);
    expect(next.anchor).toEqual({ row: 0, col: 1 });
    expect(next.range).toBeNull();
    expect(next.colSel).toBeNull();
  });
});

describe('SELECT_COLUMN — column header click selects the whole column', () => {
  it('selects the entire column, anchored at row 0', () => {
    const next = reduceSheetSelection(EMPTY_STATE, { type: 'SELECT_COLUMN', col: 2 }, BOUNDS);
    expect(next.anchor).toEqual({ row: 0, col: 2 });
    expect(next.range).toEqual({ r0: 0, r1: 8, c0: 2, c1: 2 });
    expect(next.colSel).toBe(2);
    expect(next.rowSel).toBeNull();
  });

  it('clamps the column index to the ghost-extended bounds', () => {
    const next = reduceSheetSelection(EMPTY_STATE, { type: 'SELECT_COLUMN', col: 99 }, BOUNDS);
    expect(next.colSel).toBe(6);
  });
});

describe('SELECT_ROW — row gutter click selects the whole row', () => {
  it('selects the entire row, anchored at col 0', () => {
    const next = reduceSheetSelection(EMPTY_STATE, { type: 'SELECT_ROW', row: 3 }, BOUNDS);
    expect(next.anchor).toEqual({ row: 3, col: 0 });
    expect(next.range).toEqual({ r0: 3, r1: 3, c0: 0, c1: 6 });
    expect(next.rowSel).toBe(3);
    expect(next.colSel).toBeNull();
  });

  it('clamps the row index to the ghost-extended bounds', () => {
    const next = reduceSheetSelection(EMPTY_STATE, { type: 'SELECT_ROW', row: 99 }, BOUNDS);
    expect(next.rowSel).toBe(8);
  });
});

describe('EXTEND_TO — shift+click range from anchor to clicked cell', () => {
  it('extends the range from the current anchor to the clicked cell', () => {
    const state: SheetSelectionState = { anchor: { row: 1, col: 1 }, range: null, colSel: null, rowSel: null };
    const next = reduceSheetSelection(state, { type: 'EXTEND_TO', coord: { row: 3, col: 3 } }, BOUNDS);
    expect(next.anchor).toEqual({ row: 1, col: 1 });
    expect(next.range).toEqual({ r0: 1, r1: 3, c0: 1, c1: 3 });
  });

  it('normalizes the range regardless of click direction (click above/left of anchor)', () => {
    const state: SheetSelectionState = { anchor: { row: 3, col: 3 }, range: null, colSel: null, rowSel: null };
    const next = reduceSheetSelection(state, { type: 'EXTEND_TO', coord: { row: 0, col: 0 } }, BOUNDS);
    expect(next.range).toEqual({ r0: 0, r1: 3, c0: 0, c1: 3 });
  });

  it('shift+click on the anchor itself collapses to a single-cell (null range)', () => {
    const state: SheetSelectionState = { anchor: { row: 2, col: 2 }, range: null, colSel: null, rowSel: null };
    const next = reduceSheetSelection(state, { type: 'EXTEND_TO', coord: { row: 2, col: 2 } }, BOUNDS);
    expect(next.range).toBeNull();
  });

  it('shift+click with no prior anchor behaves like a plain click (sets a new anchor)', () => {
    const next = reduceSheetSelection(EMPTY_STATE, { type: 'EXTEND_TO', coord: { row: 2, col: 2 } }, BOUNDS);
    expect(next.anchor).toEqual({ row: 2, col: 2 });
    expect(next.range).toBeNull();
  });
});

describe('helper predicates', () => {
  it('isInRange: true for a coordinate within bounds, false outside', () => {
    const range = { r0: 1, r1: 3, c0: 1, c1: 3 };
    expect(isInRange({ row: 2, col: 2 }, range)).toBe(true);
    expect(isInRange({ row: 0, col: 2 }, range)).toBe(false);
    expect(isInRange({ row: 2, col: 2 }, null)).toBe(false);
  });

  it('isAnchor: true only for the exact anchor coordinate', () => {
    const anchor = { row: 2, col: 2 };
    expect(isAnchor({ row: 2, col: 2 }, anchor)).toBe(true);
    expect(isAnchor({ row: 2, col: 3 }, anchor)).toBe(false);
    expect(isAnchor({ row: 2, col: 2 }, null)).toBe(false);
  });

  it('rangeEdges: flags all four edges for a 1x1 range', () => {
    const range = { r0: 2, r1: 2, c0: 2, c1: 2 };
    expect(rangeEdges({ row: 2, col: 2 }, range)).toEqual({ top: true, bottom: true, left: true, right: true });
  });

  it('rangeEdges: flags only the correct edges for an interior cell of a larger range', () => {
    const range = { r0: 0, r1: 2, c0: 0, c1: 2 };
    expect(rangeEdges({ row: 1, col: 1 }, range)).toEqual({ top: false, bottom: false, left: false, right: false });
    expect(rangeEdges({ row: 0, col: 0 }, range)).toEqual({ top: true, bottom: false, left: true, right: false });
    expect(rangeEdges({ row: 2, col: 2 }, range)).toEqual({ top: false, bottom: true, left: false, right: true });
  });

  it('rangeEdges: all false when the coordinate is not in the range', () => {
    const range = { r0: 0, r1: 1, c0: 0, c1: 1 };
    expect(rangeEdges({ row: 5, col: 5 }, range)).toEqual({ top: false, bottom: false, left: false, right: false });
    expect(rangeEdges({ row: 0, col: 0 }, null)).toEqual({ top: false, bottom: false, left: false, right: false });
  });
});
