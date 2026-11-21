// sheetGridPivot.test.ts — unit tests for the sparse→dense Sheets pivot (#2339 PR 9)
//
// Every test constructs real SheetColumn/SheetRow/SheetCell-shaped fixtures
// (mirroring the backend's actual struct shapes — src-tauri/src/sheets.rs)
// and asserts against the real pivotSheetData()/columnRef()/a1() output. No
// stubs, no mocked pivot logic — this exercises the exact function the grid
// component imports.

import { describe, it, expect } from 'vitest';
import {
  pivotSheetData,
  columnRef,
  a1,
  DEFAULT_COLUMN_WIDTH,
  DEFAULT_ROW_HEIGHT,
} from './sheetGridPivot';
import type { SheetColumn, SheetRow, SheetCell } from '$lib/types';

const NOTE_ID = 'note-1';

function col(overrides: Partial<SheetColumn> = {}): SheetColumn {
  return {
    id: overrides.id ?? 'col-a',
    note_id: NOTE_ID,
    name: overrides.name ?? 'Column',
    col_type: overrides.col_type ?? 'text',
    options_json: overrides.options_json ?? '{}',
    width: overrides.width ?? 148,
    sort_order: overrides.sort_order ?? 0,
    created_at: '2026-01-01T00:00:00Z',
    updated_at: '2026-01-01T00:00:00Z',
  };
}

function row(overrides: Partial<SheetRow> = {}): SheetRow {
  return {
    id: overrides.id ?? 'row-1',
    note_id: NOTE_ID,
    sort_order: overrides.sort_order ?? 0,
    height: overrides.height ?? 34,
    created_at: '2026-01-01T00:00:00Z',
    updated_at: '2026-01-01T00:00:00Z',
  };
}

function cell(overrides: Partial<SheetCell> = {}): SheetCell {
  return {
    row_id: overrides.row_id ?? 'row-1',
    column_id: overrides.column_id ?? 'col-a',
    note_id: NOTE_ID,
    raw_value: overrides.raw_value ?? '',
    computed_value: overrides.computed_value,
    style_json: overrides.style_json ?? '{}',
    updated_at: '2026-01-01T00:00:00Z',
  };
}

describe('pivotSheetData', () => {
  it('empty sheet: columns but no rows/cells produces zero dense rows, columns still present', () => {
    const columns = [
      col({ id: 'c1', name: 'A', sort_order: 0 }),
      col({ id: 'c2', name: 'B', sort_order: 1000 }),
    ];
    const result = pivotSheetData(columns, [], []);
    expect(result.columns).toHaveLength(2);
    expect(result.columns[0].id).toBe('c1');
    expect(result.columns[1].id).toBe('c2');
    expect(result.rows).toHaveLength(0);
  });

  it('sheet with rows but zero columns produces dense rows with empty cells arrays', () => {
    const rows = [row({ id: 'r1', sort_order: 0 })];
    const result = pivotSheetData([], rows, []);
    expect(result.columns).toHaveLength(0);
    expect(result.rows).toHaveLength(1);
    expect(result.rows[0].cells).toHaveLength(0);
  });

  it('full sheet: every cell populated maps to the correct dense coordinate', () => {
    const columns = [
      col({ id: 'c1', sort_order: 0 }),
      col({ id: 'c2', sort_order: 1000 }),
    ];
    const rows = [
      row({ id: 'r1', sort_order: 0 }),
      row({ id: 'r2', sort_order: 1000 }),
    ];
    const cells = [
      cell({ row_id: 'r1', column_id: 'c1', raw_value: 'A1' }),
      cell({ row_id: 'r1', column_id: 'c2', raw_value: 'B1' }),
      cell({ row_id: 'r2', column_id: 'c1', raw_value: 'A2' }),
      cell({ row_id: 'r2', column_id: 'c2', raw_value: 'B2' }),
    ];
    const result = pivotSheetData(columns, rows, cells);
    expect(result.rows).toHaveLength(2);
    expect(result.rows[0].cells).toHaveLength(2);
    expect(result.rows[0].cells[0].cell?.raw_value).toBe('A1');
    expect(result.rows[0].cells[1].cell?.raw_value).toBe('B1');
    expect(result.rows[1].cells[0].cell?.raw_value).toBe('A2');
    expect(result.rows[1].cells[1].cell?.raw_value).toBe('B2');
    // Dense coordinates match array position, not sort_order value.
    expect(result.rows[0].cells[0]).toMatchObject({ row: 0, col: 0 });
    expect(result.rows[1].cells[1]).toMatchObject({ row: 1, col: 1 });
  });

  it('sparse sheet: gaps in cells produce null placeholders at the missing coordinates', () => {
    const columns = [col({ id: 'c1', sort_order: 0 }), col({ id: 'c2', sort_order: 1000 })];
    const rows = [row({ id: 'r1', sort_order: 0 }), row({ id: 'r2', sort_order: 1000 })];
    // Only (r1,c1) and (r2,c2) populated — a diagonal-sparse grid.
    const cells = [
      cell({ row_id: 'r1', column_id: 'c1', raw_value: 'top-left' }),
      cell({ row_id: 'r2', column_id: 'c2', raw_value: 'bottom-right' }),
    ];
    const result = pivotSheetData(columns, rows, cells);
    expect(result.rows[0].cells[0].cell?.raw_value).toBe('top-left');
    expect(result.rows[0].cells[1].cell).toBeNull();
    expect(result.rows[1].cells[0].cell).toBeNull();
    expect(result.rows[1].cells[1].cell?.raw_value).toBe('bottom-right');
  });

  it('cells with various column types pivot without special-casing (pivot is type-agnostic)', () => {
    const columns = [
      col({ id: 'c-text', col_type: 'text', sort_order: 0 }),
      col({ id: 'c-number', col_type: 'number', sort_order: 1000 }),
      col({ id: 'c-currency', col_type: 'currency', sort_order: 2000 }),
      col({ id: 'c-checkbox', col_type: 'checkbox', sort_order: 3000 }),
      col({ id: 'c-select', col_type: 'select', sort_order: 4000 }),
      col({ id: 'c-date', col_type: 'date', sort_order: 5000 }),
    ];
    const rows = [row({ id: 'r1', sort_order: 0 })];
    const cells = [
      cell({ row_id: 'r1', column_id: 'c-text', raw_value: 'hello' }),
      cell({ row_id: 'r1', column_id: 'c-number', raw_value: '42' }),
      cell({ row_id: 'r1', column_id: 'c-currency', raw_value: '19.99' }),
      cell({ row_id: 'r1', column_id: 'c-checkbox', raw_value: '1' }),
      cell({ row_id: 'r1', column_id: 'c-select', raw_value: 'Done' }),
      cell({ row_id: 'r1', column_id: 'c-date', raw_value: '2026-09-04' }),
    ];
    const result = pivotSheetData(columns, rows, cells);
    const values = result.rows[0].cells.map((c) => c.cell?.raw_value);
    expect(values).toEqual(['hello', '42', '19.99', '1', 'Done', '2026-09-04']);
    expect(result.columns.map((c) => c.col_type)).toEqual([
      'text', 'number', 'currency', 'checkbox', 'select', 'date',
    ]);
  });

  it('orders columns and rows by sort_order, not by id or array-input order', () => {
    // Deliberately hand these in scrambled, with ids that would sort
    // differently than sort_order if id were used as the ordering key.
    const columns = [
      col({ id: 'z-col', name: 'Third', sort_order: 2000 }),
      col({ id: 'a-col', name: 'First', sort_order: 0 }),
      col({ id: 'm-col', name: 'Second', sort_order: 1000 }),
    ];
    const rows = [
      row({ id: 'z-row', sort_order: 2000 }),
      row({ id: 'a-row', sort_order: 0 }),
      row({ id: 'm-row', sort_order: 1000 }),
    ];
    const result = pivotSheetData(columns, rows, []);
    expect(result.columns.map((c) => c.name)).toEqual(['First', 'Second', 'Third']);
    expect(result.rows.map((r) => r.row.id)).toEqual(['a-row', 'm-row', 'z-row']);
  });

  it('breaks sort_order ties deterministically by id', () => {
    const columns = [
      col({ id: 'col-b', name: 'B', sort_order: 500 }),
      col({ id: 'col-a', name: 'A', sort_order: 500 }),
    ];
    const result = pivotSheetData(columns, [], []);
    expect(result.columns.map((c) => c.id)).toEqual(['col-a', 'col-b']);
  });

  it('does not mutate the input arrays', () => {
    const columns = [col({ id: 'c2', sort_order: 1000 }), col({ id: 'c1', sort_order: 0 })];
    const rows = [row({ id: 'r2', sort_order: 1000 }), row({ id: 'r1', sort_order: 0 })];
    const cells = [cell({ row_id: 'r1', column_id: 'c1' })];
    const columnsCopy = [...columns];
    const rowsCopy = [...rows];
    const cellsCopy = [...cells];
    pivotSheetData(columns, rows, cells);
    expect(columns).toEqual(columnsCopy);
    expect(rows).toEqual(rowsCopy);
    expect(cells).toEqual(cellsCopy);
  });

  it('applies DEFAULT_COLUMN_WIDTH when a column width is 0 (backend seed default)', () => {
    const columns = [col({ id: 'c1', width: 0 })];
    const result = pivotSheetData(columns, [], []);
    expect(result.columns[0].effectiveWidth).toBe(DEFAULT_COLUMN_WIDTH);
  });

  it('preserves a non-zero stored width as the effective width', () => {
    const columns = [col({ id: 'c1', width: 220 })];
    const result = pivotSheetData(columns, [], []);
    expect(result.columns[0].effectiveWidth).toBe(220);
  });

  it('applies DEFAULT_ROW_HEIGHT when a row height is 0 (backend seed default)', () => {
    const rows = [row({ id: 'r1', height: 0 })];
    const result = pivotSheetData([], rows, []);
    expect(result.rows[0].height).toBe(DEFAULT_ROW_HEIGHT);
  });

  it('preserves a non-zero stored height as the effective height', () => {
    const rows = [row({ id: 'r1', height: 60 })];
    const result = pivotSheetData([], rows, []);
    expect(result.rows[0].height).toBe(60);
  });

  it('ignores a cell whose row_id/column_id does not match any known row/column (defensive orphan handling)', () => {
    const columns = [col({ id: 'c1', sort_order: 0 })];
    const rows = [row({ id: 'r1', sort_order: 0 })];
    const cells = [
      cell({ row_id: 'r1', column_id: 'c1', raw_value: 'valid' }),
      cell({ row_id: 'orphan-row', column_id: 'c1', raw_value: 'orphaned' }),
      cell({ row_id: 'r1', column_id: 'orphan-col', raw_value: 'orphaned' }),
    ];
    const result = pivotSheetData(columns, rows, cells);
    expect(result.rows).toHaveLength(1);
    expect(result.rows[0].cells).toHaveLength(1);
    expect(result.rows[0].cells[0].cell?.raw_value).toBe('valid');
  });

  it('keeps computed_value alongside raw_value for formula cells', () => {
    const columns = [col({ id: 'c1' })];
    const rows = [row({ id: 'r1' })];
    const cells = [
      cell({ row_id: 'r1', column_id: 'c1', raw_value: '=SUM(A1:A2)', computed_value: '42' }),
    ];
    const result = pivotSheetData(columns, rows, cells);
    expect(result.rows[0].cells[0].cell?.raw_value).toBe('=SUM(A1:A2)');
    expect(result.rows[0].cells[0].cell?.computed_value).toBe('42');
  });
});

describe('columnRef', () => {
  it('maps 0-25 to A-Z', () => {
    expect(columnRef(0)).toBe('A');
    expect(columnRef(1)).toBe('B');
    expect(columnRef(25)).toBe('Z');
  });

  it('maps 26+ to double letters (AA, AB, ...)', () => {
    expect(columnRef(26)).toBe('AA');
    expect(columnRef(27)).toBe('AB');
    expect(columnRef(51)).toBe('AZ');
    expect(columnRef(52)).toBe('BA');
  });
});

describe('a1', () => {
  it('formats (row, col) 0-indexed pairs as A1 notation', () => {
    expect(a1(0, 0)).toBe('A1');
    expect(a1(0, 1)).toBe('B1');
    expect(a1(1, 0)).toBe('A2');
    expect(a1(4, 2)).toBe('C5');
  });
});
