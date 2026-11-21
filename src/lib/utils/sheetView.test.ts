// sheetView.test.ts — unit tests for the Sheets sort/filter view-state
// engine + type-mismatch classifier (#2339 PR 11, §33.3/§33.7). Every test
// drives the real `applySort`/`applyFilter`/`isTypeMismatch`/
// `selectChoicesFromOptionsJson`/`clearSort`/`clearFilter` against real
// SheetRow/SheetCell/SheetColumn-shaped fixtures — no mocking.

import { describe, it, expect } from 'vitest';
import {
  applySort,
  applyFilter,
  isTypeMismatch,
  selectChoicesFromOptionsJson,
  clearSort,
  clearFilter,
  filterLabel,
  filterSupported,
  composeSortChipText,
  composeFilterChipText,
  INITIAL_SHEET_VIEW_STATE,
  type SheetViewState,
} from './sheetView';
import type { SheetColumn, SheetRow, SheetCell } from '$lib/types';

function col(overrides: Partial<SheetColumn> = {}): SheetColumn {
  return {
    id: overrides.id ?? 'col-a',
    note_id: 'note-1',
    name: overrides.name ?? 'Column A',
    col_type: overrides.col_type ?? 'text',
    options_json: overrides.options_json ?? '{}',
    width: overrides.width ?? 148,
    sort_order: overrides.sort_order ?? 0,
    created_at: '2026-01-01T00:00:00Z',
    updated_at: '2026-01-01T00:00:00Z',
  };
}

function row(id: string): SheetRow {
  return { id, note_id: 'note-1', sort_order: 0, height: 34, created_at: '2026-01-01T00:00:00Z', updated_at: '2026-01-01T00:00:00Z' };
}

function cell(rowId: string, columnId: string, rawValue: string): SheetCell {
  return {
    row_id: rowId,
    column_id: columnId,
    note_id: 'note-1',
    raw_value: rawValue,
    computed_value: undefined,
    style_json: '{}',
    updated_at: '2026-01-01T00:00:00Z',
  };
}

describe('isTypeMismatch', () => {
  it('text never mismatches, for any value', () => {
    expect(isTypeMismatch('anything at all', 'text')).toBe(false);
    expect(isTypeMismatch('42', 'text')).toBe(false);
  });

  it('empty/null/undefined never mismatches, regardless of column type', () => {
    expect(isTypeMismatch('', 'number')).toBe(false);
    expect(isTypeMismatch(null, 'date')).toBe(false);
    expect(isTypeMismatch(undefined, 'checkbox')).toBe(false);
  });

  it('number mismatches on non-numeric text', () => {
    expect(isTypeMismatch('TBD', 'number')).toBe(true);
    expect(isTypeMismatch('42', 'number')).toBe(false);
    expect(isTypeMismatch('-3.5', 'number')).toBe(false);
  });

  it('currency mismatches on non-numeric text', () => {
    expect(isTypeMismatch('n/a', 'currency')).toBe(true);
    expect(isTypeMismatch('19.99', 'currency')).toBe(false);
  });

  it('date mismatches on non-ISO, non-mask text', () => {
    expect(isTypeMismatch('not a date', 'date')).toBe(true);
    expect(isTypeMismatch('2026-01-15', 'date')).toBe(false);
    expect(isTypeMismatch('1/15/2026', 'date')).toBe(false);
  });

  it('date mismatches on a shape-matching but invalid calendar date', () => {
    expect(isTypeMismatch('2026-13-40', 'date')).toBe(true);
    expect(isTypeMismatch('2026-02-30', 'date')).toBe(true);
  });

  it('checkbox mismatches on anything other than 0/1', () => {
    expect(isTypeMismatch('yes', 'checkbox')).toBe(true);
    expect(isTypeMismatch('true', 'checkbox')).toBe(true);
    expect(isTypeMismatch('0', 'checkbox')).toBe(false);
    expect(isTypeMismatch('1', 'checkbox')).toBe(false);
  });

  it('select mismatches on a value not in the provided choices', () => {
    expect(isTypeMismatch('Purple', 'select', ['Red', 'Green'])).toBe(true);
    expect(isTypeMismatch('Red', 'select', ['Red', 'Green'])).toBe(false);
  });

  it('select mismatches on any non-empty value when choices is omitted/empty', () => {
    expect(isTypeMismatch('anything', 'select')).toBe(true);
    expect(isTypeMismatch('anything', 'select', [])).toBe(true);
  });

  it('an unknown column type never mismatches (defensive default)', () => {
    expect(isTypeMismatch('anything', 'unknown-type')).toBe(false);
  });
});

describe('selectChoicesFromOptionsJson', () => {
  it('parses a valid choices array', () => {
    expect(selectChoicesFromOptionsJson('{"choices":["Red","Green"]}')).toEqual(['Red', 'Green']);
  });

  it('returns [] for null/undefined/empty options_json', () => {
    expect(selectChoicesFromOptionsJson(null)).toEqual([]);
    expect(selectChoicesFromOptionsJson(undefined)).toEqual([]);
    expect(selectChoicesFromOptionsJson('')).toEqual([]);
  });

  it('returns [] for malformed JSON', () => {
    expect(selectChoicesFromOptionsJson('{not json')).toEqual([]);
  });

  it('returns [] when choices is present but not an array', () => {
    expect(selectChoicesFromOptionsJson('{"choices":"Red"}')).toEqual([]);
  });
});

describe('applySort', () => {
  const columns = [col({ id: 'c1', col_type: 'number' })];

  it('is a no-op (identity order, new array) when sortColumnId is null', () => {
    const rows = [row('r1'), row('r2')];
    const result = applySort(rows, [], columns, null, 'asc');
    expect(result).not.toBe(rows);
    expect(result.map((r) => r.id)).toEqual(['r1', 'r2']);
  });

  it('is a no-op when sortDirection is null', () => {
    const rows = [row('r1'), row('r2')];
    const result = applySort(rows, [], columns, 'c1', null);
    expect(result.map((r) => r.id)).toEqual(['r1', 'r2']);
  });

  it('is a no-op when sortColumnId does not resolve to a real column', () => {
    const rows = [row('r1'), row('r2')];
    const result = applySort(rows, [], columns, 'nonexistent', 'asc');
    expect(result.map((r) => r.id)).toEqual(['r1', 'r2']);
  });

  it('sorts a number column ascending', () => {
    const rows = [row('r1'), row('r2'), row('r3')];
    const cells = [cell('r1', 'c1', '30'), cell('r2', 'c1', '10'), cell('r3', 'c1', '20')];
    const result = applySort(rows, cells, columns, 'c1', 'asc');
    expect(result.map((r) => r.id)).toEqual(['r2', 'r3', 'r1']);
  });

  it('sorts a number column descending', () => {
    const rows = [row('r1'), row('r2'), row('r3')];
    const cells = [cell('r1', 'c1', '30'), cell('r2', 'c1', '10'), cell('r3', 'c1', '20')];
    const result = applySort(rows, cells, columns, 'c1', 'desc');
    expect(result.map((r) => r.id)).toEqual(['r1', 'r3', 'r2']);
  });

  it('sorts a text column ascending (lexicographic)', () => {
    const textCols = [col({ id: 'c1', col_type: 'text' })];
    const rows = [row('r1'), row('r2'), row('r3')];
    const cells = [cell('r1', 'c1', 'banana'), cell('r2', 'c1', 'apple'), cell('r3', 'c1', 'cherry')];
    const result = applySort(rows, cells, textCols, 'c1', 'asc');
    expect(result.map((r) => r.id)).toEqual(['r2', 'r1', 'r3']);
  });

  it('sorts a select column the same as text (lexicographic)', () => {
    const selectCols = [col({ id: 'c1', col_type: 'select' })];
    const rows = [row('r1'), row('r2')];
    const cells = [cell('r1', 'c1', 'Zebra'), cell('r2', 'c1', 'Apple')];
    const result = applySort(rows, cells, selectCols, 'c1', 'asc');
    expect(result.map((r) => r.id)).toEqual(['r2', 'r1']);
  });

  // Item 24: text columns get natural (numeric-aware) ordering via
  // localeCompare(..., { numeric: true }) instead of plain lexicographic
  // `<`/`>`. Select is intentionally NOT touched — it keeps the old
  // lexicographic `default` branch (see test above, unchanged).
  it('item 24: sorts a text column of pure numeric strings naturally, ascending', () => {
    const textCols = [col({ id: 'c1', col_type: 'text' })];
    const rows = [row('r1'), row('r2'), row('r3')];
    const cells = [cell('r1', 'c1', '3'), cell('r2', 'c1', '34'), cell('r3', 'c1', '7')];
    const result = applySort(rows, cells, textCols, 'c1', 'asc');
    // Natural order: 3 < 7 < 34 (NOT lexicographic '3' < '34' < '7')
    expect(result.map((r) => r.id)).toEqual(['r1', 'r3', 'r2']);
  });

  it('item 24: sorts a text column of pure numeric strings naturally, descending', () => {
    const textCols = [col({ id: 'c1', col_type: 'text' })];
    const rows = [row('r1'), row('r2'), row('r3')];
    const cells = [cell('r1', 'c1', '3'), cell('r2', 'c1', '34'), cell('r3', 'c1', '7')];
    const result = applySort(rows, cells, textCols, 'c1', 'desc');
    // Natural order descending: 34 > 7 > 3
    expect(result.map((r) => r.id)).toEqual(['r2', 'r3', 'r1']);
  });

  it('item 24: sorts a text column with mixed alpha-numeric values naturally', () => {
    const textCols = [col({ id: 'c1', col_type: 'text' })];
    const rows = [row('r1'), row('r2'), row('r3')];
    const cells = [cell('r1', 'c1', 'Item 3'), cell('r2', 'c1', 'Item 34'), cell('r3', 'c1', 'Item 7')];
    const result = applySort(rows, cells, textCols, 'c1', 'asc');
    // Natural order: 'Item 3' < 'Item 7' < 'Item 34'
    expect(result.map((r) => r.id)).toEqual(['r1', 'r3', 'r2']);
  });

  it('item 24: a text column with an empty-string cell still sorts it last, ascending', () => {
    const textCols = [col({ id: 'c1', col_type: 'text' })];
    const rows = [row('r1'), row('r2'), row('r3')];
    const cells = [cell('r1', 'c1', '7'), cell('r2', 'c1', ''), cell('r3', 'c1', '3')];
    const result = applySort(rows, cells, textCols, 'c1', 'asc');
    // Empty string is invalid-for-sort (isValidForSort) and always sorts
    // last, regardless of the new numeric-aware text comparator.
    expect(result.map((r) => r.id)).toEqual(['r3', 'r1', 'r2']);
  });

  it('sorts a checkbox column, unchecked before checked ascending', () => {
    const checkCols = [col({ id: 'c1', col_type: 'checkbox' })];
    const rows = [row('r1'), row('r2')];
    const cells = [cell('r1', 'c1', '1'), cell('r2', 'c1', '0')];
    const result = applySort(rows, cells, checkCols, 'c1', 'asc');
    expect(result.map((r) => r.id)).toEqual(['r2', 'r1']);
  });

  it('sorts a date column chronologically ascending', () => {
    const dateCols = [col({ id: 'c1', col_type: 'date' })];
    const rows = [row('r1'), row('r2'), row('r3')];
    const cells = [cell('r1', 'c1', '2026-06-01'), cell('r2', 'c1', '2026-01-01'), cell('r3', 'c1', '2026-03-01')];
    const result = applySort(rows, cells, dateCols, 'c1', 'asc');
    expect(result.map((r) => r.id)).toEqual(['r2', 'r3', 'r1']);
  });

  it('is stable — rows with equal sort keys preserve original relative order', () => {
    const rows = [row('r1'), row('r2'), row('r3')];
    const cells = [cell('r1', 'c1', '10'), cell('r2', 'c1', '10'), cell('r3', 'c1', '5')];
    const result = applySort(rows, cells, columns, 'c1', 'asc');
    expect(result.map((r) => r.id)).toEqual(['r3', 'r1', 'r2']);
  });

  // Ruling 6: mismatched/unparseable values always sort LAST, in BOTH
  // directions.
  it('ruling 6: an unparseable number value sorts last ascending', () => {
    const rows = [row('r1'), row('r2'), row('r3')];
    const cells = [cell('r1', 'c1', '5'), cell('r2', 'c1', 'TBD'), cell('r3', 'c1', '1')];
    const result = applySort(rows, cells, columns, 'c1', 'asc');
    expect(result.map((r) => r.id)).toEqual(['r3', 'r1', 'r2']);
  });

  it('ruling 6: an unparseable number value STILL sorts last descending', () => {
    const rows = [row('r1'), row('r2'), row('r3')];
    const cells = [cell('r1', 'c1', '5'), cell('r2', 'c1', 'TBD'), cell('r3', 'c1', '1')];
    const result = applySort(rows, cells, columns, 'c1', 'desc');
    expect(result.map((r) => r.id)).toEqual(['r1', 'r3', 'r2']);
  });

  it('ruling 6: an unparseable date sorts last regardless of direction', () => {
    const dateCols = [col({ id: 'c1', col_type: 'date' })];
    const rows = [row('r1'), row('r2')];
    const cells = [cell('r1', 'c1', '2026-01-01'), cell('r2', 'c1', 'not a date')];
    expect(applySort(rows, cells, dateCols, 'c1', 'asc').map((r) => r.id)).toEqual(['r1', 'r2']);
    expect(applySort(rows, cells, dateCols, 'c1', 'desc').map((r) => r.id)).toEqual(['r1', 'r2']);
  });

  it('ruling 6: an unparseable currency value sorts last regardless of direction', () => {
    const currencyCols = [col({ id: 'c1', col_type: 'currency' })];
    const rows = [row('r1'), row('r2')];
    const cells = [cell('r1', 'c1', '19.99'), cell('r2', 'c1', 'n/a')];
    expect(applySort(rows, cells, currencyCols, 'c1', 'asc').map((r) => r.id)).toEqual(['r1', 'r2']);
    expect(applySort(rows, cells, currencyCols, 'c1', 'desc').map((r) => r.id)).toEqual(['r1', 'r2']);
  });

  it('ruling 6: an unparseable checkbox value sorts last regardless of direction', () => {
    const checkCols = [col({ id: 'c1', col_type: 'checkbox' })];
    const rows = [row('r1'), row('r2')];
    const cells = [cell('r1', 'c1', '1'), cell('r2', 'c1', 'maybe')];
    expect(applySort(rows, cells, checkCols, 'c1', 'asc').map((r) => r.id)).toEqual(['r1', 'r2']);
    expect(applySort(rows, cells, checkCols, 'c1', 'desc').map((r) => r.id)).toEqual(['r1', 'r2']);
  });

  it('treats a row with no cell at all for the sort column as invalid (sorts last)', () => {
    const rows = [row('r1'), row('r2')];
    const cells = [cell('r1', 'c1', '5')]; // r2 has no cell
    const result = applySort(rows, cells, columns, 'c1', 'asc');
    expect(result.map((r) => r.id)).toEqual(['r1', 'r2']);
  });

  it('never mutates the input rows array', () => {
    const rows = [row('r1'), row('r2')];
    const cells = [cell('r1', 'c1', '10'), cell('r2', 'c1', '5')];
    const snapshot = [...rows];
    applySort(rows, cells, columns, 'c1', 'asc');
    expect(rows).toEqual(snapshot);
  });
});

describe('applyFilter', () => {
  const selectCols = [col({ id: 'c1', col_type: 'select' })];
  const checkCols = [col({ id: 'c1', col_type: 'checkbox' })];

  it('shows all rows when filterColumnId is null', () => {
    const rows = [row('r1'), row('r2')];
    expect(applyFilter(rows, [], selectCols, null, ['Red']).map((r) => r.id)).toEqual(['r1', 'r2']);
  });

  it('shows all rows when filterValues is null', () => {
    const rows = [row('r1'), row('r2')];
    expect(applyFilter(rows, [], selectCols, 'c1', null).map((r) => r.id)).toEqual(['r1', 'r2']);
  });

  it('shows all rows when filterValues is an empty array', () => {
    const rows = [row('r1'), row('r2')];
    expect(applyFilter(rows, [], selectCols, 'c1', []).map((r) => r.id)).toEqual(['r1', 'r2']);
  });

  it('includes only rows whose select cell value is in filterValues', () => {
    const rows = [row('r1'), row('r2'), row('r3')];
    const cells = [cell('r1', 'c1', 'Red'), cell('r2', 'c1', 'Green'), cell('r3', 'c1', 'Red')];
    const result = applyFilter(rows, cells, selectCols, 'c1', ['Red']);
    expect(result.map((r) => r.id)).toEqual(['r1', 'r3']);
  });

  it('excludes a row with no cell at all for a select filter', () => {
    const rows = [row('r1'), row('r2')];
    const cells = [cell('r1', 'c1', 'Red')]; // r2 has no cell
    const result = applyFilter(rows, cells, selectCols, 'c1', ['Red']);
    expect(result.map((r) => r.id)).toEqual(['r1']);
  });

  it('filters checkbox TRUE/FALSE correctly', () => {
    const rows = [row('r1'), row('r2'), row('r3')];
    const cells = [cell('r1', 'c1', '1'), cell('r2', 'c1', '0'), cell('r3', 'c1', '1')];
    expect(applyFilter(rows, cells, checkCols, 'c1', ['1']).map((r) => r.id)).toEqual(['r1', 'r3']);
    expect(applyFilter(rows, cells, checkCols, 'c1', ['0']).map((r) => r.id)).toEqual(['r2']);
  });

  it('normalizes a missing checkbox cell to "0" (unchecked) rather than excluding it', () => {
    const rows = [row('r1'), row('r2')];
    const cells = [cell('r1', 'c1', '1')]; // r2 has no cell at all
    const result = applyFilter(rows, cells, checkCols, 'c1', ['0']);
    expect(result.map((r) => r.id)).toEqual(['r2']);
  });

  it('a checkbox filter for both 0 and 1 shows every row', () => {
    const rows = [row('r1'), row('r2'), row('r3')];
    const cells = [cell('r1', 'c1', '1'), cell('r2', 'c1', '0')]; // r3 has no cell
    const result = applyFilter(rows, cells, checkCols, 'c1', ['0', '1']);
    expect(result.map((r) => r.id)).toEqual(['r1', 'r2', 'r3']);
  });

  it('never mutates the input rows array', () => {
    const rows = [row('r1'), row('r2')];
    const cells = [cell('r1', 'c1', 'Red'), cell('r2', 'c1', 'Green')];
    const snapshot = [...rows];
    applyFilter(rows, cells, selectCols, 'c1', ['Red']);
    expect(rows).toEqual(snapshot);
  });
});

describe('clearSort / clearFilter', () => {
  const active: SheetViewState = {
    sortColumnId: 'c1',
    sortDirection: 'asc',
    filterColumnId: 'c2',
    filterValues: ['Red'],
  };

  it('clearSort resets only the sort fields, leaving filter untouched', () => {
    const result = clearSort(active);
    expect(result.sortColumnId).toBeNull();
    expect(result.sortDirection).toBeNull();
    expect(result.filterColumnId).toBe('c2');
    expect(result.filterValues).toEqual(['Red']);
  });

  it('clearFilter resets only the filter fields, leaving sort untouched', () => {
    const result = clearFilter(active);
    expect(result.filterColumnId).toBeNull();
    expect(result.filterValues).toBeNull();
    expect(result.sortColumnId).toBe('c1');
    expect(result.sortDirection).toBe('asc');
  });

  it('INITIAL_SHEET_VIEW_STATE is all-null (no sort/filter active)', () => {
    expect(INITIAL_SHEET_VIEW_STATE).toEqual({
      sortColumnId: null,
      sortDirection: null,
      filterColumnId: null,
      filterValues: null,
    });
  });
});

// #2874 item 16 Part C (relocation, same precedent as `filterLabel`'s own
// PR 12 move) — `filterSupported` moved here verbatim from
// `ShHeaderMenu.svelte`'s previously-private copy.
describe('filterSupported', () => {
  it('returns true for select columns', () => {
    expect(filterSupported('select')).toBe(true);
  });

  it('returns true for checkbox columns', () => {
    expect(filterSupported('checkbox')).toBe(true);
  });

  it('returns false for text/number/currency/date columns', () => {
    for (const t of ['text', 'number', 'currency', 'date']) {
      expect(filterSupported(t)).toBe(false);
    }
  });
});

// #2339 PR 12 (#2339 relocation, Matt's ruling 2026-09-04 (3)) — `filterLabel`
// moved here verbatim from `ShHeaderMenu.svelte`'s previously-private copy.
describe('filterLabel', () => {
  it('maps a checkbox raw "1" to "True" and "0" to "False"', () => {
    const checkCol = col({ col_type: 'checkbox' });
    expect(filterLabel(checkCol, '1')).toBe('True');
    expect(filterLabel(checkCol, '0')).toBe('False');
  });

  it('returns a select column\'s raw choice string unchanged', () => {
    const selectCol = col({ col_type: 'select' });
    expect(filterLabel(selectCol, 'Lead')).toBe('Lead');
  });
});

// #2339 PR 12 §3 — pure chip-text composition for `SheetViewStrip`.
describe('composeSortChipText', () => {
  it('spells out "descending" (never the abbreviation "desc")', () => {
    expect(composeSortChipText('Total', 'desc')).toBe('Total descending');
  });

  it('spells out "ascending" (never the abbreviation "asc")', () => {
    expect(composeSortChipText('Total', 'asc')).toBe('Total ascending');
  });
});

describe('composeFilterChipText', () => {
  it('single value selected out of many choices: positive phrasing', () => {
    const selectCol = col({ col_type: 'select' });
    const text = composeFilterChipText('Status', ['Lead'], ['Lead', 'Contacted', 'Won', 'Cut'], selectCol);
    expect(text).toBe('Status is Lead');
  });

  it('Matt\'s ruling (1): exactly N-1 of N choices selected reads as exclusion "is not X"', () => {
    const selectCol = col({ col_type: 'select' });
    const text = composeFilterChipText('Status', ['Lead', 'Contacted', 'Won'], ['Lead', 'Contacted', 'Won', 'Cut'], selectCol);
    expect(text).toBe('Status is not Cut');
  });

  it('multiple (but not N-1) values selected: positive comma-joined list', () => {
    const selectCol = col({ col_type: 'select' });
    const text = composeFilterChipText('Status', ['Lead', 'Contacted'], ['Lead', 'Contacted', 'Won', 'Cut'], selectCol);
    expect(text).toBe('Status is Lead, Contacted');
  });

  it('checkbox values are rendered through filterLabel (True/False, not raw 1/0)', () => {
    const checkCol = col({ col_type: 'checkbox' });
    expect(composeFilterChipText('Active', ['1'], ['0', '1'], checkCol)).toBe('Active is not False');
    expect(composeFilterChipText('Active', ['1', '0'], ['0', '1'], checkCol)).toBe('Active is True, False');
  });

  it('all choices selected (N of N) falls through to the positive list, not exclusion', () => {
    const selectCol = col({ col_type: 'select' });
    const text = composeFilterChipText('Status', ['Lead', 'Cut'], ['Lead', 'Cut'], selectCol);
    expect(text).toBe('Status is Lead, Cut');
  });
});
