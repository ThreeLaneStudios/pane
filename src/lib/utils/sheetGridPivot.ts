// sheetGridPivot.ts — sparse-to-dense pivot for the Sheets grid (#2339 PR 9, §33.3)
//
// The backend (`get_sheet_data`/`create_sheet`) returns a SPARSE cell list:
// `cells: SheetCell[]`, keyed by `(row_id, column_id)`, with no entry at all
// for an empty coordinate. `SheetGrid.svelte` needs a DENSE 2D structure —
// `denseRows[r].cells[c]` — so it can render a CSS Grid without doing a
// lookup-and-miss on every one of (visible rows × visible columns) cells on
// every render.
//
// Pure, DOM-free, fully unit-tested (sheetGridPivot.test.ts). No mutation of
// any input array/object — every function here returns new data.

import type { SheetColumn, SheetRow, SheetCell } from '$lib/types';

/** Fallback render width applied when a column's stored `width` is 0 (the
 *  backend's seed default for freshly-created columns — 0 is never a valid
 *  render dimension). Matches the design's default column width (sheets.css
 *  / sheets-shared.jsx: `c.w || 148`). */
export const DEFAULT_COLUMN_WIDTH = 148;

/** Fallback render height applied when a row's stored `height` is 0 (the
 *  backend's seed default for freshly-created rows). Matches the design's
 *  default row height (sheets-shared.jsx: `rowH = 34`). */
export const DEFAULT_ROW_HEIGHT = 34;

/** Ghost column render width (#2339 PR 10, §5.5 carry-forward fix) —
 *  deliberately narrower than `DEFAULT_COLUMN_WIDTH` (148). Matches the
 *  design reference's own named constant (`sheets-shared.jsx`: `const
 *  ghostW = 120;`), distinct from the `c.w || 148` fallback used for real
 *  columns — narrower ghost columns are one more "this isn't real yet"
 *  signal alongside reduced opacity and thinner hairlines. PR 9 incorrectly
 *  used `DEFAULT_COLUMN_WIDTH` for ghost columns; this constant exists so
 *  `SheetGrid.svelte`'s `gridTemplateColumns()` can use the correct value. */
export const GHOST_COLUMN_WIDTH = 120;

/** One densely-positioned cell in the pivoted grid. `col`/`row` are the
 *  0-indexed grid coordinates (col 0 = A, row 0 = row 1). `cell` is the raw
 *  sparse SheetCell if one exists at this coordinate, or `null` if the
 *  coordinate is empty (no cell was ever written). */
export interface DenseCell {
  col: number;
  row: number;
  cell: SheetCell | null;
}

/** One densely-positioned row: the row's effective render height (with the
 *  0-fallback applied) plus a dense `cells` array, one entry per column, in
 *  column order (index === column's position in `columns`, NOT the column's
 *  sort_order value itself). */
export interface DenseRow {
  /** The source SheetRow this dense row was built from. */
  row: SheetRow;
  /** Effective render height — `row.height || DEFAULT_ROW_HEIGHT`. */
  height: number;
  /** One entry per column, in column order. */
  cells: DenseCell[];
}

/** Output of `pivotSheetData` — columns (with effective render width
 *  attached) and dense rows, both already in the backend's sort_order. */
export interface PivotedSheet {
  columns: Array<SheetColumn & { effectiveWidth: number }>;
  rows: DenseRow[];
}

/**
 * Pivot a sparse SheetData (columns/rows/cells, as returned by
 * `get_sheet_data`/`create_sheet`) into the dense row/column structure
 * SheetGrid.svelte renders from.
 *
 * Ordering: `columns` and `rows` are assumed already sorted by `sort_order`
 * ASC (the backend's `get_sheet_data`/`create_sheet` queries both use
 * `ORDER BY sort_order ASC`) — this function does NOT re-sort by `sort_order`
 * itself blindly; it explicitly re-sorts defensively so a caller that hands
 * in out-of-order arrays (e.g. a future caching layer, or a hand-built test
 * fixture) still pivots correctly. Ties in `sort_order` fall back to `id`
 * for a fully deterministic order.
 *
 * Complexity: O(columns × rows) to build the dense grid skeleton, plus
 * O(cells) to place each sparse cell via a single Map lookup — no O(rows ×
 * columns × cells) nested scan.
 */
export function pivotSheetData(
  columns: SheetColumn[],
  rows: SheetRow[],
  cells: SheetCell[]
): PivotedSheet {
  const sortedColumns = [...columns].sort(sortOrderThenId);
  const sortedRows = [...rows].sort(sortOrderThenId);

  // row_id -> column_id -> SheetCell, so cell placement is O(1) per cell.

  const cellByRowThenCol = new Map<string, Map<string, SheetCell>>();
  for (const cell of cells) {
    let byCol = cellByRowThenCol.get(cell.row_id);
    if (!byCol) {
      byCol = new Map();
      cellByRowThenCol.set(cell.row_id, byCol);
    }
    byCol.set(cell.column_id, cell);
  }

  const denseRows: DenseRow[] = sortedRows.map((row, r) => {
    const byCol = cellByRowThenCol.get(row.id);
    const denseCells: DenseCell[] = sortedColumns.map((col, c) => ({
      col: c,
      row: r,
      cell: byCol?.get(col.id) ?? null,
    }));
    return {
      row,
      height: row.height || DEFAULT_ROW_HEIGHT,
      cells: denseCells,
    };
  });

  const denseColumns = sortedColumns.map((c) => ({
    ...c,
    effectiveWidth: c.width || DEFAULT_COLUMN_WIDTH,
  }));

  return { columns: denseColumns, rows: denseRows };
}

function sortOrderThenId(
  a: { sort_order: number; id: string },
  b: { sort_order: number; id: string }
): number {
  if (a.sort_order !== b.sort_order) return a.sort_order - b.sort_order;
  return a.id < b.id ? -1 : a.id > b.id ? 1 : 0;
}

/** Convenience helper: 0-indexed column number → spreadsheet column letter
 *  (0 -> 'A', 1 -> 'B', ..., 25 -> 'Z', 26 -> 'AA', ...). Matches the design
 *  reference's `COLREF` helper (sheets-shared.jsx). */
export function columnRef(index: number): string {
  let n = index;
  let ref = '';
  do {
    ref = String.fromCharCode(65 + (n % 26)) + ref;
    n = Math.floor(n / 26) - 1;
  } while (n >= 0);
  return ref;
}

/** Convenience helper: (row, col) 0-indexed grid coordinates -> A1 notation,
 *  e.g. (0,0) -> 'A1', (1,2) -> 'C2'. Matches the design reference's `A1`
 *  helper (sheets-shared.jsx). */
export function a1(row: number, col: number): string {
  return columnRef(col) + (row + 1);
}
