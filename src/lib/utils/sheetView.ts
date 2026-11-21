// sheetView.ts — sort/filter view-state engine + type-mismatch classifier
// for the Sheets grid (#2339 PR 11, §33.3/§33.7 — scope report
// `docs/audits/2026-09-04-sheets-pr11-header-menu-scope.md`). Pure,
// DOM-free, fully unit-tested (sheetView.test.ts) — same "no Svelte
// dependency" convention as sheetGridPivot.ts/sheetSelection.ts/
// sheetCellRender.ts/sheetCellCommit.ts.
//
// Sort/filter are VIEW-ONLY transforms (§33.7, restated by Matt's ruling
// 2026-09-04): they reorder/hide already-fetched rows at render time and
// NEVER touch `sort_order`, never call any backend command, and never
// mutate their inputs. A refetch always returns the sheet's true stored
// order — sort/filter state is held entirely in `SheetGrid.svelte`'s own
// component state (`SheetViewState`) and applied on top of that on every
// render, exactly like the design's own `ShView` frame describes ("row
// order and stored data are unchanged").
//
// Single-column sort only (ruling 5): `sortColumnId` is `string | null`,
// not an array — clicking "Sort ascending" on a new column REPLACES
// whatever column was previously sorted, no multi-sort pre-shaping.

import type { SheetColumn, SheetRow, SheetCell } from '$lib/types';
import { parseDateMaskToIso } from './sheetCellCommit';

export type SortDirection = 'asc' | 'desc';

/** The Sheets view-state: which column (if any) is sorted/filtered, and
 *  how. Held by `SheetGrid.svelte`, applied on top of the fetched
 *  `SheetData` at render time via `applySort`/`applyFilter` below. */
export interface SheetViewState {
  sortColumnId: string | null;
  sortDirection: SortDirection | null;
  /** The filtered column's id, or `null` if no filter is active. Scoped
   *  to `select`/`checkbox` columns only (ruling 3) at the CALL SITE
   *  (`ShHeaderMenu.svelte`'s "Filter…" row is `disabled` for every
   *  other column type) — this module itself has no type awareness and
   *  will happily filter by any column id it's given. */
  filterColumnId: string | null;
  /** The set of raw cell values to INCLUDE when `filterColumnId` is set
   *  (checkbox: `'0'`/`'1'`; select: values from the column's configured
   *  `options_json.choices`). `null` or an empty array means "no filter
   *  active" — all rows pass. */
  filterValues: string[] | null;
}

export const INITIAL_SHEET_VIEW_STATE: SheetViewState = {
  sortColumnId: null,
  sortDirection: null,
  filterColumnId: null,
  filterValues: null,
};

/** Clears any active sort. Exposed (per scope report) for PR 12's view-
 *  strip chip `×` button — PR 11's own `ShHeaderMenu` never calls this;
 *  sort is only ever SET (ascending/descending), never explicitly cleared,
 *  from the menu itself in this PR. */
export function clearSort(state: SheetViewState): SheetViewState {
  return { ...state, sortColumnId: null, sortDirection: null };
}

/** Clears any active filter. Same PR 12 carry-forward as `clearSort` —
 *  unused by this PR's own menu wiring (the filter value-picker toggles
 *  individual values; there's no chip-level "clear all" affordance until
 *  the view strip lands). */
export function clearFilter(state: SheetViewState): SheetViewState {
  return { ...state, filterColumnId: null, filterValues: null };
}

/** Parses a `select` column's configured choice list out of its
 *  `options_json` blob (`{"choices": [...]}`), tolerating missing/
 *  malformed JSON (returns `[]`) — the ONE place this parse lives.
 *  `SheetGrid.svelte` (cell rendering, the select cell editor) and
 *  `ShHeaderMenu.svelte` (type submenu tick, filter value-picker) both
 *  import this rather than each keeping their own inline copy, so the
 *  "what counts as this column's choices" rule can't drift between
 *  call sites. */
export function selectChoicesFromOptionsJson(optionsJson: string | null | undefined): string[] {
  try {
    const opts = JSON.parse(optionsJson || '{}');
    return Array.isArray(opts.choices) ? opts.choices : [];
  } catch {
    return [];
  }
}

/** Whether `colType` supports the header menu's (and, #2874 item 16 Part
 *  C, the toolbar's) Filter… affordance — `select`/`checkbox` only
 *  (Matt's ruling 2026-09-04 (3)). Relocated verbatim from
 *  `ShHeaderMenu.svelte`'s previously-private copy (#2874 item 16 Part C),
 *  same mechanical-move-only precedent as `filterLabel`'s own PR 12
 *  extraction just below — `ShHeaderMenu.svelte` and `SheetGrid.svelte`
 *  (toolbar Filter button's disabled gate, `toolbarFilter()`'s no-op
 *  guard) both import this rather than keeping their own copies, so the
 *  supported-type list can't drift between call sites. */
export function filterSupported(colType: string): boolean {
  return colType === 'select' || colType === 'checkbox';
}

/** Human-readable label for a raw filter/cell value (#2339 PR 12, §4 —
 *  relocated verbatim from `ShHeaderMenu.svelte`'s previously-private
 *  copy, per Matt's ruling 2026-09-04 (3), so the view strip's
 *  `composeFilterChipText` can share the exact same label rule rather
 *  than re-deriving it). `checkbox`'s raw `'1'`/`'0'` maps to `'True'`/
 *  `'False'`; every other column type's raw value already IS its own
 *  human label (a `select` column's stored choice string). */
export function filterLabel(col: SheetColumn, choice: string): string {
  if (col.col_type === 'checkbox') return choice === '1' ? 'True' : 'False';
  return choice;
}

/** Chip text for the view strip's sort chip (#2339 PR 12) — the column
 *  name plus the direction word, spelled out ("descending"/"ascending",
 *  never the abbreviation "desc"/"asc"), matching the locked design's
 *  literal example ("Total descending"). Pure, DOM-free — the component
 *  bolds `columnName` itself; this function returns the plain
 *  concatenation the component then splits/renders around the bold
 *  span. */
export function composeSortChipText(columnName: string, direction: SortDirection): string {
  const word = direction === 'asc' ? 'ascending' : 'descending';
  return `${columnName} ${word}`;
}

/** Chip text for the view strip's filter chip (#2339 PR 12, Matt's
 *  ruling 2026-09-04 (1)): when exactly N-1 of N configured choices are
 *  selected, compose as exclusion phrasing ("ColumnName is not X", the
 *  one excluded value) — matching the locked design's literal example
 *  ("Status is not Cut"). Otherwise, positive list phrasing
 *  ("ColumnName is Value1, Value2" for however many values are
 *  selected, comma-joined). `allChoices` is the column's FULL configured
 *  choice list (via `selectChoicesFromOptionsJson` for `select`, or
 *  `['0','1']` for `checkbox` — resolved by the caller, this function
 *  has no type-specific parsing of its own) — needed to determine N (the
 *  choice count) and to find the one excluded value in the N-1 case.
 *  Values are rendered through `filterLabel` so a checkbox's raw `'1'`/
 *  `'0'` reads as `'True'`/`'False'`, not the raw stored string. */
export function composeFilterChipText(
  columnName: string,
  filterValues: string[],
  allChoices: string[],
  column: SheetColumn
): string {
  const n = allChoices.length;
  const selectedCount = filterValues.length;
  if (n > 0 && selectedCount === n - 1) {
    const excluded = allChoices.find((c) => !filterValues.includes(c));
    if (excluded !== undefined) {
      return `${columnName} is not ${filterLabel(column, excluded)}`;
    }
  }
  const labels = filterValues.map((v) => filterLabel(column, v));
  return `${columnName} is ${labels.join(', ')}`;
}

function cellRawValue(rowId: string, columnId: string, cells: SheetCell[]): string | null {
  const found = cells.find((c) => c.row_id === rowId && c.column_id === columnId);
  return found ? found.raw_value ?? '' : null;
}

// ── mismatch classifier (ShMismatch, ruling 7) ───────────────────────────

function isParsableNumber(value: string): boolean {
  const trimmed = value.trim();
  if (trimmed === '') return false;
  return Number.isFinite(Number(trimmed));
}

function isParsableDate(value: string): boolean {
  const trimmed = value.trim();
  // A committed date cell is stored as ISO-8601 (`valueForCommit` converts
  // the mask on commit) — accept that form directly, validating it's a
  // real calendar date (not just shape-matching, e.g. reject 2026-13-40).
  const isoMatch = trimmed.match(/^(\d{4})-(\d{2})-(\d{2})$/);
  if (isoMatch) {
    const [, y, m, d] = isoMatch;
    const year = Number(y);
    const month = Number(m);
    const day = Number(d);
    const date = new Date(Date.UTC(year, month - 1, day));
    return date.getUTCFullYear() === year && date.getUTCMonth() === month - 1 && date.getUTCDate() === day;
  }
  // A legacy/sync/AI-written value might still be sitting in the mask form
  // (`M/D/YY[YY]`) rather than ISO — reuse the exact same parser the date
  // editor's own commit path uses, rather than re-deriving mask-parsing
  // rules here.
  return parseDateMaskToIso(trimmed) !== null;
}

/**
 * Classifies whether `rawValue` fails to parse as `colType`'s expected
 * shape — the ShMismatch predicate (ruling 7). `text` never mismatches
 * (any string is valid text). An empty/null value is never a mismatch (an
 * empty cell is just empty, not wrong — mismatch is specifically about a
 * value that WAS entered but doesn't fit the column's type).
 *
 * `choices` is the `select` column's configured option list (from
 * `options_json.choices`, resolved by the caller — this module has no
 * JSON-parsing concerns of its own). A `select` cell with a non-empty
 * value that isn't in `choices` (including when `choices` is omitted or
 * empty, e.g. a choice-less select column per ruling 2's carry-forward)
 * is a mismatch — there's no way to call it a "valid choice" without a
 * configured list to check it against.
 *
 * Formula cells (`raw_value` starting with `=`) are NEVER classified here
 * — `cellKind()` (sheetCellRender.ts) resolves calc/err/raw for a formula
 * cell BEFORE ever consulting `isTypeMismatch`, so this function is only
 * ever called (by `cellKind`) on a non-formula value. It doesn't special-
 * case a leading `=` itself, since it's never handed one in practice, but
 * doesn't crash on one either (an `=`-prefixed string just fails whichever
 * type check it's run through, same as any other non-parsing string).
 */
export function isTypeMismatch(
  rawValue: string | null | undefined,
  colType: string,
  choices?: string[]
): boolean {
  const value = rawValue ?? '';
  if (value === '') return false;
  switch (colType) {
    case 'text':
      return false;
    case 'number':
    case 'currency':
      return !isParsableNumber(value);
    case 'date':
      return !isParsableDate(value);
    case 'checkbox':
      return value !== '0' && value !== '1';
    case 'select':
      return !(choices ?? []).includes(value);
    default:
      return false;
  }
}

// ── sort (ruling 5: single-column only; ruling 6: mismatches sort last) ──

function numericSortKey(value: string): number {
  return Number(value.trim());
}

function dateSortKey(value: string): number {
  const trimmed = value.trim();
  const iso = /^\d{4}-\d{2}-\d{2}$/.test(trimmed) ? trimmed : parseDateMaskToIso(trimmed);
  return iso ? Date.parse(iso) : NaN;
}

function checkboxSortKey(value: string): number {
  return value === '1' ? 1 : 0;
}

function isValidForSort(raw: string | null, colType: string): boolean {
  if (raw == null || raw === '') return false;
  switch (colType) {
    case 'number':
    case 'currency':
      return isParsableNumber(raw);
    case 'date':
      return isParsableDate(raw);
    case 'checkbox':
      return raw === '0' || raw === '1';
    default:
      return true; // text / select — any non-empty string sorts normally
  }
}

function compareValid(a: string, b: string, colType: string): number {
  switch (colType) {
    case 'number':
    case 'currency':
      return numericSortKey(a) - numericSortKey(b);
    case 'date':
      return dateSortKey(a) - dateSortKey(b);
    case 'checkbox':
      return checkboxSortKey(a) - checkboxSortKey(b);
    case 'text':
      return a.localeCompare(b, undefined, { numeric: true });
    default:
      return a < b ? -1 : a > b ? 1 : 0; // select / fallback
  }
}

/**
 * Sorts `rows` by `sortColumnId`'s cell values, per `sortDirection`. Pure
 * — returns a NEW array, never mutates `rows`/`cells`, never writes
 * `sort_order`. `sortColumnId: null` (or an unresolvable column id, or a
 * null `sortDirection`) is a no-op that still returns a fresh copy of
 * `rows` in their given order.
 *
 * Stable: JS's `Array.prototype.sort` has been spec-guaranteed-stable
 * since ES2019, so equal-key rows preserve their original relative order
 * with no manual index tie-break needed.
 *
 * Ruling 6: a value that doesn't parse per the column's type (or is
 * simply empty) ALWAYS sorts last, in BOTH directions — achieved by
 * splitting the comparator into an "invalid always loses" branch that
 * runs before the direction-flipped comparison, so `dir` only ever
 * flips the ordering of two VALID values against each other.
 */
export function applySort(
  rows: SheetRow[],
  cells: SheetCell[],
  columns: SheetColumn[],
  sortColumnId: string | null,
  sortDirection: SortDirection | null
): SheetRow[] {
  if (!sortColumnId || !sortDirection) return [...rows];
  const column = columns.find((c) => c.id === sortColumnId);
  if (!column) return [...rows];

  const colType = column.col_type;
  const dir = sortDirection === 'asc' ? 1 : -1;

  const decorated = rows.map((row) => {
    const raw = cellRawValue(row.id, sortColumnId, cells);
    return { row, raw: raw ?? '', valid: isValidForSort(raw, colType) };
  });

  decorated.sort((x, y) => {
    if (x.valid && !y.valid) return -1;
    if (!x.valid && y.valid) return 1;
    if (!x.valid && !y.valid) return 0;
    return dir * compareValid(x.raw, y.raw, colType);
  });

  return decorated.map((d) => d.row);
}

// ── filter (ruling 3: select/checkbox value-pickers only) ───────────────

/**
 * Filters `rows` to only those whose `filterColumnId` cell value is
 * included in `filterValues`. Pure — returns a NEW array subset, never
 * mutates. `filterValues === null` or `[]` is "no filter" — every row
 * passes.
 *
 * `columns` is used only to special-case `checkbox`: a row with NO cell
 * at all for that column (`cellRawValue` returns `null`, not `'0'`) reads
 * as unchecked in the grid's own render rule (`!!raw_value && raw_value
 * !== '0'`), so an absent checkbox cell is normalized to `'0'` here before
 * the `filterValues.includes(...)` check — otherwise a "show unchecked"
 * filter would incorrectly exclude every row that's never had its
 * checkbox cell written at all (the common case for a freshly-added
 * checkbox column). No such normalization exists for `select` — an
 * absent select cell has no analogous "default value" and is excluded
 * from every non-empty filter, matching its blank/no-pill render.
 *
 * Scoped to `select`/`checkbox` at the CALL SITE (ruling 3, same split as
 * `applySort`'s type-specific comparators) — this function itself will
 * filter by any column id/values it's given.
 */
export function applyFilter(
  rows: SheetRow[],
  cells: SheetCell[],
  columns: SheetColumn[],
  filterColumnId: string | null,
  filterValues: string[] | null
): SheetRow[] {
  if (!filterColumnId || !filterValues || filterValues.length === 0) return [...rows];
  const column = columns.find((c) => c.id === filterColumnId);
  const isCheckbox = column?.col_type === 'checkbox';

  return rows.filter((row) => {
    const raw = cellRawValue(row.id, filterColumnId, cells);
    const normalized = raw == null ? (isCheckbox ? '0' : null) : isCheckbox && raw === '' ? '0' : raw;
    return normalized != null && filterValues.includes(normalized);
  });
}
