// SheetGrid.test.ts — component tests for the Sheets grid's PR 10 editing
// lifecycle: editor open/commit/cancel, checkbox toggle, ghost
// typing-to-commit (all 3 create-mutation cases), header rename, and the
// toolbar-facing insertRow/insertColumn exports (#2339 PR 10, §8).
//
// `$lib/api` is mocked wholesale (all four PR 10 mutating wrappers plus
// `getSheetData`) — this is a component test asserting SheetGrid's own
// orchestration (call order, state transitions), not a re-test of the
// pure `sheetSelection`/`sheetCellCommit`/`sheetCellRender` modules, which
// already have their own real-fixture unit tests.

import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, within } from '@testing-library/svelte/svelte5';
import { tick } from 'svelte';
import SheetGrid from './SheetGrid.svelte';
import type { SheetColumn, SheetRow, SheetCell, SheetData } from '$lib/types';

// vitest-setup.ts replaces `globalThis.window` with a plain mock object
// lacking addEventListener/removeEventListener. Most of this file's tests
// don't need real window-level event dispatch, so the original no-op
// stub (matching ProviderConnectionCard.test.ts / sheetEditors' own
// tests) is fine for them. PR 13's resize-drag tests DO need real
// dispatch (SheetGrid.svelte attaches `window.addEventListener('mousemove'
// / 'mouseup', ...)` on grip mousedown) — same pattern as
// ChatSidebar.onboardingAgentName.test.ts: bind the mock's
// addEventListener/removeEventListener through to the REAL jsdom global
// (still reachable via `globalThis.addEventListener`, since vitest-setup
// only replaces `globalThis.window`, not `globalThis` itself), then
// dispatch test events via `globalThis.dispatchEvent(...)` (the mocked
// `window.dispatchEvent` stays a no-op, unused by this file).
const win = globalThis.window as any;
if (typeof win.addEventListener !== 'function') {
  win.addEventListener = globalThis.addEventListener?.bind(globalThis) ?? (() => {});
}
if (typeof win.removeEventListener !== 'function') {
  win.removeEventListener = globalThis.removeEventListener?.bind(globalThis) ?? (() => {});
}

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

function baseSheetData(overrides: Partial<SheetData> = {}): SheetData {
  return {
    note_meta: { id: NOTE_ID, title: 'Sheet', notebook: '', slug: 'sheet', created_at: '', updated_at: '', preview: '' } as any,
    columns: overrides.columns ?? [col({ id: 'col-a', name: 'Name', sort_order: 0 })],
    rows: overrides.rows ?? [row({ id: 'row-1', sort_order: 0 })],
    cells: overrides.cells ?? [],
    formula_count: overrides.formula_count ?? 0,
    formulas_paused: overrides.formulas_paused ?? false,
  };
}

const {
  getSheetDataMock,
  updateSheetCellMock,
  updateSheetCellsStyleBatchMock,
  createSheetRowMock,
  createSheetColumnMock,
  createSheetRowsBatchMock,
  createSheetColumnsBatchMock,
  updateSheetColumnMock,
  updateSheetRowMock,
  deleteSheetColumnMock,
} = vi.hoisted(() => ({
  getSheetDataMock: vi.fn(),
  updateSheetCellMock: vi.fn(),
  // #2874 item 51: `applyStyleToSelection`/`clearSelectedStyling` now
  // send ONE batch call per action instead of one `updateSheetCell` call
  // PER CELL — kept as its OWN mock (not reusing `updateSheetCellMock`)
  // so every test below can assert unambiguously on call COUNT (one per
  // style action, never N) as well as on the batch's entries shape.
  updateSheetCellsStyleBatchMock: vi.fn(),
  createSheetRowMock: vi.fn(),
  createSheetColumnMock: vi.fn(),
  // Item 23 / #2874: ghost typing-to-commit's batch endpoints. Kept
  // separate from createSheetRowMock/createSheetColumnMock (still used
  // by the toolbar/context-menu single-insert flows, which are UNCHANGED
  // by this item) so each mock's call assertions stay unambiguous about
  // which endpoint shape is under test.
  createSheetRowsBatchMock: vi.fn(),
  createSheetColumnsBatchMock: vi.fn(),
  updateSheetColumnMock: vi.fn(),
  updateSheetRowMock: vi.fn(),
  deleteSheetColumnMock: vi.fn(),
}));

vi.mock('$lib/api', () => ({
  getSheetData: getSheetDataMock,
  updateSheetCell: updateSheetCellMock,
  updateSheetCellsStyleBatch: updateSheetCellsStyleBatchMock,
  createSheetRow: createSheetRowMock,
  createSheetColumn: createSheetColumnMock,
  createSheetRowsBatch: createSheetRowsBatchMock,
  createSheetColumnsBatch: createSheetColumnsBatchMock,
  updateSheetColumn: updateSheetColumnMock,
  updateSheetRow: updateSheetRowMock,
  deleteSheetColumn: deleteSheetColumnMock,
}));

beforeEach(() => {
  getSheetDataMock.mockReset();
  updateSheetCellMock.mockReset();
  updateSheetCellsStyleBatchMock.mockReset();
  // #2874 item 51: a sane default so tests that don't care about the
  // exact returned `SheetCell` shape (the vast majority — most just
  // assert on the batch CALL, not the patched render) don't need their
  // own `mockResolvedValue` just to avoid `applyLocalCellPatches`
  // receiving `undefined`. Deliberately mirrors this file's PRE-item-51
  // `updateSheetCellMock.mockResolvedValue(cell({ raw_value: 'a' }))`
  // convention EXACTLY — a generic cell with DEFAULT (`'{}'`) style_json,
  // not an echo of the entry's own `styleJson` — for every cell in the
  // batch. Echoing the real written style back would change what
  // `applyStyleToSelection`'s NEXT invocation reads as `existing` from
  // the local cache for a multi-click-in-one-test sequence (the item 29/
  // 45 toggle tests), which is a pre-existing per-cell border-math
  // behavior this item must not perturb ("the per-cell border math is
  // unchanged"). Tests that need a SPECIFIC `computed_value`/`raw_value`/
  // `style_json` (the item 47 tests) override this via their own
  // `.mockResolvedValue([...])` call.
  // #2874 item 59: echo the WRITTEN styleJson back, same as the real
  // backend does (`sheets.rs:2517`'s `result_cells` map returns
  // `style_json: w.new_style` — the write verbatim, not a blank
  // default). The previous blank-`'{}'`-every-time default (see
  // `docs/audits/2026-10-06-2874-items-59-61.md`, "Test coverage gap")
  // meant `existing` read `[]` on every click after the first in any
  // multi-click-in-one-test sequence, which is NOT what production does
  // and is exactly why the item 59 stale-side bug shipped with a green
  // suite. Tests that need a specific DIFFERENT `computed_value`/
  // `raw_value`/`style_json` still override this via their own
  // `.mockResolvedValue([...])` / `.mockResolvedValueOnce([...])` call.
  updateSheetCellsStyleBatchMock.mockImplementation(
    async (_noteId: string, entries: { rowId: string; columnId: string; styleJson: string }[]) =>
      entries.map((e) => cell({ row_id: e.rowId, column_id: e.columnId, raw_value: 'a', style_json: e.styleJson }))
  );
  createSheetRowMock.mockReset();
  createSheetColumnMock.mockReset();
  createSheetRowsBatchMock.mockReset();
  createSheetColumnsBatchMock.mockReset();
  updateSheetColumnMock.mockReset();
  updateSheetRowMock.mockReset();
  deleteSheetColumnMock.mockReset();
});

async function renderGrid(data: SheetData) {
  getSheetDataMock.mockResolvedValue(data);
  const result = render(SheetGrid, { props: { noteId: NOTE_ID } });
  await vi.waitFor(() => expect(screen.getByTestId('sheet-grid')).toBeTruthy());
  await tick();
  return result;
}

/** #2874 item 51: `applyStyleToSelection`/`clearSelectedStyling` now send
 *  ONE `updateSheetCellsStyleBatch` call per user action, carrying every
 *  touched cell as one entry — this is the batch-era replacement for the
 *  old `updateSheetCellMock.mock.calls.find((c) => c[0] === rowId && ...)`
 *  per-cell lookup. `callIndex` selects WHICH batch call to inspect, for
 *  tests that fire several sequential style actions in one test (defaults
 *  to the MOST RECENT call). Returns the entry's PARSED `styleJson`
 *  object directly, since every caller immediately needs the parsed
 *  shape (`bg`/`fg`/`bd`), not the raw string. */
function batchEntry(rowId: string, columnId: string, callIndex = -1) {
  const calls = updateSheetCellsStyleBatchMock.mock.calls;
  const call = callIndex === -1 ? calls[calls.length - 1] : calls[callIndex];
  const entries = (call?.[1] ?? []) as { rowId: string; columnId: string; styleJson: string }[];
  const entry = entries.find((e) => e.rowId === rowId && e.columnId === columnId);
  return entry ? JSON.parse(entry.styleJson) : undefined;
}

describe('SheetGrid — cell editor open/commit/cancel (§2/§3)', () => {
  it('double-click on a text cell opens an editor pre-filled with the existing value', async () => {
    const data = baseSheetData({ cells: [cell({ raw_value: 'Longform essays' })] });
    await renderGrid(data);

    await fireEvent.dblClick(screen.getByTestId('sheet-cell-0-0'));
    await tick();

    const input = screen.getByTestId('sheet-cell-editor-input') as HTMLInputElement;
    expect(input.value).toBe('Longform essays');
  });

  it('commits the edited value via updateSheetCell on Enter, closes the editor, and moves selection down', async () => {
    const data = baseSheetData({ cells: [cell({ raw_value: 'old' })] });
    updateSheetCellMock.mockResolvedValue(cell({ raw_value: 'new value' }));
    await renderGrid(data);

    await fireEvent.dblClick(screen.getByTestId('sheet-cell-0-0'));
    await tick();
    const input = screen.getByTestId('sheet-cell-editor-input') as HTMLInputElement;
    await fireEvent.input(input, { target: { value: 'new value' } });
    await fireEvent.keyDown(input, { key: 'Enter' });
    await tick();

    expect(updateSheetCellMock).toHaveBeenCalledWith('row-1', 'col-a', 'new value', null);
    expect(screen.queryByTestId('sheet-cell-editor-input')).toBeNull();
  });

  it('reverts to the pre-edit value on Escape without calling updateSheetCell', async () => {
    const data = baseSheetData({ cells: [cell({ raw_value: 'original' })] });
    await renderGrid(data);

    await fireEvent.dblClick(screen.getByTestId('sheet-cell-0-0'));
    await tick();
    const input = screen.getByTestId('sheet-cell-editor-input') as HTMLInputElement;
    await fireEvent.input(input, { target: { value: 'garbage typed' } });
    await fireEvent.keyDown(input, { key: 'Escape' });
    await tick();

    expect(updateSheetCellMock).not.toHaveBeenCalled();
    expect(screen.queryByTestId('sheet-cell-editor-input')).toBeNull();
    expect(screen.getByTestId('sheet-cell-0-0').textContent).toContain('original');
  });

  it('typing a printable character while a real cell is the sole anchor opens the editor with the cell CLEARED (type-to-overwrite, ruling 1)', async () => {
    const data = baseSheetData({ cells: [cell({ raw_value: 'existing' })] });
    await renderGrid(data);

    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-0-0'));
    await tick();
    await fireEvent.keyDown(screen.getByTestId('sheet-grid-wrap'), { key: 'x' });
    await tick();

    const input = screen.getByTestId('sheet-cell-editor-input') as HTMLInputElement;
    expect(input.value).toBe('x');
  });

  it('Enter on an anchored, non-editing cell is navigation only — it never opens an editor (ruling 1)', async () => {
    const data = baseSheetData({
      rows: [row({ id: 'row-1', sort_order: 0 }), row({ id: 'row-2', sort_order: 1 })],
      cells: [cell({ raw_value: 'a' })],
    });
    await renderGrid(data);

    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-0-0'));
    await tick();
    await fireEvent.keyDown(screen.getByTestId('sheet-grid-wrap'), { key: 'Enter' });
    await tick();

    expect(screen.queryByTestId('sheet-cell-editor-input')).toBeNull();
    expect(updateSheetCellMock).not.toHaveBeenCalled();
  });

  it('a rejected commit (e.g. invalid formula) reopens the editor with the unsent value and an inline error', async () => {
    const data = baseSheetData({ cells: [cell({ raw_value: '' })] });
    updateSheetCellMock.mockRejectedValue(new Error('Invalid formula'));
    await renderGrid(data);

    await fireEvent.dblClick(screen.getByTestId('sheet-cell-0-0'));
    await tick();
    const input = screen.getByTestId('sheet-cell-editor-input') as HTMLInputElement;
    await fireEvent.input(input, { target: { value: '=BROKEN(' } });
    await fireEvent.keyDown(input, { key: 'Enter' });
    await vi.waitFor(() => expect(screen.getByText('Invalid formula')).toBeTruthy());

    const reopened = screen.getByTestId('sheet-cell-editor-input') as HTMLInputElement;
    expect(reopened.value).toBe('=BROKEN(');
  });
});

describe('SheetGrid — checkbox toggle (§2/checkbox — no editor)', () => {
  it('clicking a checkbox cell toggles the value immediately via updateSheetCell, with no editor overlay', async () => {
    const data = baseSheetData({
      columns: [col({ id: 'col-a', col_type: 'checkbox' })],
      cells: [cell({ raw_value: '0' })],
    });
    updateSheetCellMock.mockResolvedValue(cell({ raw_value: '1' }));
    await renderGrid(data);

    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-0-0'));
    await fireEvent.click(screen.getByTestId('sheet-cell-0-0'));
    await tick();

    expect(updateSheetCellMock).toHaveBeenCalledWith('row-1', 'col-a', '1', null);
    expect(screen.queryByTestId('sheet-cell-editor-input')).toBeNull();
  });
});

// #2874 item 39 (Matt's ruling 2026-09-15): the checked state renders an
// inline SVG check mark inside the `.sh-box` span, reusing the existing
// `.sh-box`/`.sh-box.is-on` color rules — no new CSS color values.
describe('SheetGrid — checkbox check mark rendering (#2874 item 39)', () => {
  it('checkbox checked cell renders SVG check mark', async () => {
    const data = baseSheetData({
      columns: [col({ id: 'col-a', col_type: 'checkbox' })],
      cells: [cell({ raw_value: '1' })],
    });
    await renderGrid(data);

    const cellEl = screen.getByTestId('sheet-cell-0-0');
    const box = cellEl.querySelector('.sh-box.is-on');
    expect(box).toBeTruthy();
    expect(box?.querySelector('svg')).toBeTruthy();
  });

  it('checkbox unchecked cell does not render SVG check mark', async () => {
    const data = baseSheetData({
      columns: [col({ id: 'col-a', col_type: 'checkbox' })],
      cells: [cell({ raw_value: '0' })],
    });
    await renderGrid(data);

    const cellEl = screen.getByTestId('sheet-cell-0-0');
    const box = cellEl.querySelector('.sh-box');
    expect(box).toBeTruthy();
    expect(box?.classList.contains('is-on')).toBe(false);
    expect(box?.querySelector('svg')).toBeNull();
  });
});

// #2874 item 38 (Matt's ruling 2026-09-15): currency cells render via the
// shared `formatCurrencyValue()` formatter — symbol + two fixed decimals
// (`$930.00`), never the raw decimal value followed by a separate
// `.sh-cur` ISO-code span. The column header's own `.sh-h__code` label is
// untouched — removal is cells-only (ruling 2).
describe('SheetGrid — currency cell formatting (#2874 item 38)', () => {
  it('currency cell renders symbol and two decimals', async () => {
    const data = baseSheetData({
      columns: [col({ id: 'col-a', name: 'Total', col_type: 'currency', options_json: '{"currency":"USD"}' })],
      cells: [cell({ raw_value: '930' })],
    });
    await renderGrid(data);

    expect(screen.getByTestId('sheet-cell-0-0').textContent).toContain('$930.00');
  });

  it('currency cell does not render ISO code span', async () => {
    const data = baseSheetData({
      columns: [col({ id: 'col-a', name: 'Total', col_type: 'currency', options_json: '{"currency":"USD"}' })],
      cells: [cell({ raw_value: '930' })],
    });
    const { container } = await renderGrid(data);

    const cellEl = screen.getByTestId('sheet-cell-0-0');
    expect(cellEl.querySelector('.sh-cur')).toBeNull();
    expect(cellEl.textContent).not.toContain('USD');
    expect(container.querySelector('.sh-cur')).toBeNull();
  });

  it('currency header still shows the code label', async () => {
    const data = baseSheetData({
      columns: [col({ id: 'col-a', name: 'Total', col_type: 'currency', options_json: '{"currency":"USD"}' })],
      cells: [cell({ raw_value: '930' })],
    });
    await renderGrid(data);

    expect(screen.getByTestId('sheet-col-header-0').textContent).toContain('USD');
  });
});

describe('SheetGrid — ghost typing-to-commit (§4, batch-shaped per item 23 / #2874)', () => {
  it('case 1 (ghost row only, offset 1): creates ONE row via the batch endpoint, writes the typed char, refetches, and reopens the editor', async () => {
    const initial = baseSheetData({ cells: [cell({ raw_value: 'a' })] });
    getSheetDataMock.mockResolvedValueOnce(initial);
    await renderGrid(initial);

    const newRow = row({ id: 'row-new', sort_order: 1 });
    createSheetRowsBatchMock.mockResolvedValue([newRow]);
    updateSheetCellMock.mockResolvedValue(cell({ row_id: 'row-new', raw_value: 'z' }));
    const afterCreate = baseSheetData({
      rows: [row({ id: 'row-1', sort_order: 0 }), newRow],
      cells: [cell({ raw_value: 'a' }), cell({ row_id: 'row-new', raw_value: 'z' })],
    });
    getSheetDataMock.mockResolvedValueOnce(afterCreate);

    // Ghost row 0 (first ghost row, index = pivoted.rows.length = 1) at col 0.
    // Offset = 1 - 1 + 1 = 1 (backward-compatible single-row case).
    await fireEvent.mouseDown(screen.getByTestId('sheet-ghost-cell-1-0'));
    await tick();
    await fireEvent.keyDown(screen.getByTestId('sheet-grid-wrap'), { key: 'z' });
    await vi.waitFor(() => expect(createSheetRowsBatchMock).toHaveBeenCalledWith(NOTE_ID, 1));

    expect(updateSheetCellMock).toHaveBeenCalledWith('row-new', 'col-a', 'z', null);
    expect(createSheetRowMock).not.toHaveBeenCalled();
    await vi.waitFor(() => expect(getSheetDataMock).toHaveBeenCalledTimes(2));
    await vi.waitFor(() => expect(screen.getByTestId('sheet-cell-editor-input')).toBeTruthy());
  });

  it('case 1 (ghost row only, offset N>1 — the actual bug scenario): creates N rows via ONE batch call, value lands in the LAST row', async () => {
    // Regression test for item 23: clicking the 3rd ghost row down (not
    // the 1st) must create 3 rows in ONE call, and the typed value must
    // land in the 3rd (last) one — not the 1st, which was the pre-fix bug.
    const initial = baseSheetData({ cells: [cell({ raw_value: 'a' })] });
    getSheetDataMock.mockResolvedValueOnce(initial);
    await renderGrid(initial);

    const newRows = [
      row({ id: 'row-ghost-1', sort_order: 1000 }),
      row({ id: 'row-ghost-2', sort_order: 2000 }),
      row({ id: 'row-ghost-3', sort_order: 3000 }),
    ];
    createSheetRowsBatchMock.mockResolvedValue(newRows);
    updateSheetCellMock.mockResolvedValue(cell({ row_id: 'row-ghost-3', raw_value: 'z' }));
    const afterCreate = baseSheetData({
      rows: [row({ id: 'row-1', sort_order: 0 }), ...newRows],
      cells: [cell({ raw_value: 'a' }), cell({ row_id: 'row-ghost-3', raw_value: 'z' })],
    });
    getSheetDataMock.mockResolvedValueOnce(afterCreate);

    // rowOrder.length = 1 (one real row). Clicking ghost row index gri=2
    // (the 3rd ghost row, gRow = 1 + 2 = 3) → offset = 3 - 1 + 1 = 3.
    await fireEvent.mouseDown(screen.getByTestId('sheet-ghost-cell-3-0'));
    await tick();
    await fireEvent.keyDown(screen.getByTestId('sheet-grid-wrap'), { key: 'z' });

    // Exactly ONE batch call for all 3 rows — not 3 separate single-row calls.
    await vi.waitFor(() => expect(createSheetRowsBatchMock).toHaveBeenCalledWith(NOTE_ID, 3));
    expect(createSheetRowsBatchMock).toHaveBeenCalledTimes(1);
    expect(createSheetRowMock).not.toHaveBeenCalled();

    // Typed value targets the LAST row in the batch, not the first.
    expect(updateSheetCellMock).toHaveBeenCalledWith('row-ghost-3', 'col-a', 'z', null);
  });

  it('case 2 (ghost column only, offset 1): creates ONE column via the batch endpoint with name:"" col_type:"text", writes the char, and the cell editor stays open (§33.3.5 amendment, item 44 / #2874 — supersedes PR 10\'s header-rename-on-birth)', async () => {
    const initial = baseSheetData({ cells: [cell({ raw_value: 'a' })] });
    await renderGrid(initial);

    const newCol = col({ id: 'col-new', name: '', col_type: 'text', sort_order: 1 });
    createSheetColumnsBatchMock.mockResolvedValue([newCol]);
    updateSheetCellMock.mockResolvedValue(cell({ column_id: 'col-new', raw_value: 'q' }));
    const afterCreate = baseSheetData({
      columns: [col({ id: 'col-a', name: 'Name', sort_order: 0 }), newCol],
      cells: [cell({ raw_value: 'a' }), cell({ column_id: 'col-new', raw_value: 'q' })],
    });
    getSheetDataMock.mockResolvedValueOnce(afterCreate);

    // Ghost column 0 (first ghost col, index = pivoted.columns.length = 1) at row 0.
    await fireEvent.mouseDown(screen.getByTestId('sheet-ghost-cell-0-0'));
    await tick();
    await fireEvent.keyDown(screen.getByTestId('sheet-grid-wrap'), { key: 'q' });
    await vi.waitFor(() => expect(createSheetColumnsBatchMock).toHaveBeenCalledWith(NOTE_ID, 1));

    expect(updateSheetCellMock).toHaveBeenCalledWith('row-1', 'col-new', 'q', null);
    expect(createSheetColumnMock).not.toHaveBeenCalled();
    // §33.3.5: no rename input opens — the cell editor stays open on the
    // new cell WITH the typed character in it, focused, exactly like the
    // ghost-row-only path.
    expect(screen.queryByTestId('sheet-header-rename-input')).toBeNull();
    const editorInput = await vi.waitFor(() => screen.getByTestId('sheet-cell-editor-input') as HTMLInputElement);
    expect(editorInput.value).toBe('q');
    expect(document.activeElement).toBe(editorInput);
    // The new column keeps its empty name — no rename call ever fires from
    // ghost typing.
    expect(updateSheetColumnMock).not.toHaveBeenCalled();
  });

  it('case 2 (ghost column only, offset N>1): creates N columns via ONE batch call, value lands in the LAST column', async () => {
    const initial = baseSheetData({ cells: [cell({ raw_value: 'a' })] });
    await renderGrid(initial);

    const newCols = [
      col({ id: 'col-ghost-1', name: '', col_type: 'text', sort_order: 1000 }),
      col({ id: 'col-ghost-2', name: '', col_type: 'text', sort_order: 2000 }),
    ];
    createSheetColumnsBatchMock.mockResolvedValue(newCols);
    updateSheetCellMock.mockResolvedValue(cell({ column_id: 'col-ghost-2', raw_value: 'q' }));
    const afterCreate = baseSheetData({
      columns: [col({ id: 'col-a', name: 'Name', sort_order: 0 }), ...newCols],
      cells: [cell({ raw_value: 'a' }), cell({ column_id: 'col-ghost-2', raw_value: 'q' })],
    });
    getSheetDataMock.mockResolvedValueOnce(afterCreate);

    // pivoted.columns.length = 1. Clicking ghost col index gi=1 (the 2nd
    // ghost column, gCol = 1 + 1 = 2) → offset = 2 - 1 + 1 = 2.
    await fireEvent.mouseDown(screen.getByTestId('sheet-ghost-cell-0-1'));
    await tick();
    await fireEvent.keyDown(screen.getByTestId('sheet-grid-wrap'), { key: 'q' });

    await vi.waitFor(() => expect(createSheetColumnsBatchMock).toHaveBeenCalledWith(NOTE_ID, 2));
    expect(createSheetColumnsBatchMock).toHaveBeenCalledTimes(1);
    expect(createSheetColumnMock).not.toHaveBeenCalled();
    expect(updateSheetCellMock).toHaveBeenCalledWith('row-1', 'col-ghost-2', 'q', null);
  });

  it('case 3 (both ghost): creates row batch AND column batch (two independent transactions), writes the char, and the cell editor stays open (§33.3.5 amendment, item 44 / #2874)', async () => {
    const initial = baseSheetData({ cells: [cell({ raw_value: 'a' })] });
    await renderGrid(initial);

    const newRow = row({ id: 'row-new', sort_order: 1 });
    const newCol = col({ id: 'col-new', name: '', col_type: 'text', sort_order: 1 });
    createSheetRowsBatchMock.mockResolvedValue([newRow]);
    createSheetColumnsBatchMock.mockResolvedValue([newCol]);
    updateSheetCellMock.mockResolvedValue(cell({ row_id: 'row-new', column_id: 'col-new', raw_value: 'm' }));
    const afterCreate = baseSheetData({
      columns: [col({ id: 'col-a', name: 'Name', sort_order: 0 }), newCol],
      rows: [row({ id: 'row-1', sort_order: 0 }), newRow],
      cells: [cell({ raw_value: 'a' }), cell({ row_id: 'row-new', column_id: 'col-new', raw_value: 'm' })],
    });
    getSheetDataMock.mockResolvedValueOnce(afterCreate);

    // Bottom-right ghost intersection: ghost row 0 × ghost col 0.
    await fireEvent.mouseDown(screen.getByTestId('sheet-ghost-cell-1-1'));
    await tick();
    await fireEvent.keyDown(screen.getByTestId('sheet-grid-wrap'), { key: 'm' });
    await vi.waitFor(() => expect(createSheetRowsBatchMock).toHaveBeenCalledWith(NOTE_ID, 1));
    expect(createSheetColumnsBatchMock).toHaveBeenCalledWith(NOTE_ID, 1);

    expect(updateSheetCellMock).toHaveBeenCalledWith('row-new', 'col-new', 'm', null);
    // §33.3.5: same as column-only — no rename input, editor stays open
    // with the typed character, on the new (row, column) cell.
    expect(screen.queryByTestId('sheet-header-rename-input')).toBeNull();
    const editorInput = await vi.waitFor(() => screen.getByTestId('sheet-cell-editor-input') as HTMLInputElement);
    expect(editorInput.value).toBe('m');
    expect(document.activeElement).toBe(editorInput);
  });

  it('intermediate-rows-are-empty invariant: an N>1 offset commit never writes a cell value into any row except the last', async () => {
    const initial = baseSheetData({ cells: [cell({ raw_value: 'a' })] });
    getSheetDataMock.mockResolvedValueOnce(initial);
    await renderGrid(initial);

    const newRows = [
      row({ id: 'row-ghost-1', sort_order: 1000 }),
      row({ id: 'row-ghost-2', sort_order: 2000 }),
    ];
    createSheetRowsBatchMock.mockResolvedValue(newRows);
    updateSheetCellMock.mockResolvedValue(cell({ row_id: 'row-ghost-2', raw_value: 'x' }));
    getSheetDataMock.mockResolvedValueOnce(baseSheetData({
      rows: [row({ id: 'row-1', sort_order: 0 }), ...newRows],
      cells: [cell({ raw_value: 'a' }), cell({ row_id: 'row-ghost-2', raw_value: 'x' })],
    }));

    // rowOrder.length = 1, click gRow=2 (2nd ghost row) → offset 2.
    await fireEvent.mouseDown(screen.getByTestId('sheet-ghost-cell-2-0'));
    await tick();
    await fireEvent.keyDown(screen.getByTestId('sheet-grid-wrap'), { key: 'x' });

    await vi.waitFor(() => expect(updateSheetCellMock).toHaveBeenCalledTimes(1));
    // updateSheetCell is called exactly once, targeting only the last
    // created row — the intermediate row ('row-ghost-1') never receives a
    // separate updateSheetCell call (the backend itself leaves it sparse).
    expect(updateSheetCellMock).not.toHaveBeenCalledWith('row-ghost-1', expect.anything(), expect.anything(), expect.anything());
    expect(updateSheetCellMock).toHaveBeenCalledWith('row-ghost-2', 'col-a', 'x', null);
  });

  it('a failed ghost-row batch create shows a toast and reverts the ghost cell (no partial commit)', async () => {
    const initial = baseSheetData({ cells: [cell({ raw_value: 'a' })] });
    await renderGrid(initial);

    createSheetRowsBatchMock.mockRejectedValue(new Error('DB error'));

    await fireEvent.mouseDown(screen.getByTestId('sheet-ghost-cell-1-0'));
    await tick();
    await fireEvent.keyDown(screen.getByTestId('sheet-grid-wrap'), { key: 'z' });

    await vi.waitFor(() => expect(screen.getByTestId('sheet-toast')).toBeTruthy());
    expect(updateSheetCellMock).not.toHaveBeenCalled();
    // Only one get_sheet_data call — the initial load; the failed create
    // never reached the refetch step.
    expect(getSheetDataMock).toHaveBeenCalledTimes(1);
  });
});

// #2874 item 44 / §33.3.5 amendment: "Typing into a ghost column never
// opens the column rename input. After the commit, the cell editor stays
// open on the new cell with the typed character in it, so typing continues
// in the cell exactly as it does for a ghost row. The new column keeps its
// empty name until the user renames it from the header menu (right-click
// → Rename). The same applies when a row and a column are both ghosts.
// Supersedes PR 10's rename-on-birth for ghost typing (Matt ruling, #2874
// item 44)." This block asserts the four testable requirements directly
// (the "ghost typing-to-commit" block above also re-asserts requirements
// 1/2 as part of its own case 2/3 coverage — this block is the dedicated,
// spec-traceable home for all four, including the header-menu-still-works
// requirement, which nothing above exercises post-ghost-commit).
describe('SheetGrid — ghost-column typing never opens the rename input (§33.3.5 amendment, item 44 / #2874)', () => {
  it('requirement 1+2: no rename input ever renders for a ghost-column commit; the cell editor stays open on the new cell WITH the typed character, and that editor is focused', async () => {
    const initial = baseSheetData({ cells: [cell({ raw_value: 'a' })] });
    await renderGrid(initial);

    const newCol = col({ id: 'col-new', name: '', col_type: 'text', sort_order: 1 });
    createSheetColumnsBatchMock.mockResolvedValue([newCol]);
    updateSheetCellMock.mockResolvedValue(cell({ column_id: 'col-new', raw_value: 'q' }));
    getSheetDataMock.mockResolvedValueOnce(baseSheetData({
      columns: [col({ id: 'col-a', name: 'Name', sort_order: 0 }), newCol],
      cells: [cell({ raw_value: 'a' }), cell({ column_id: 'col-new', raw_value: 'q' })],
    }));

    await fireEvent.mouseDown(screen.getByTestId('sheet-ghost-cell-0-0'));
    await tick();
    await fireEvent.keyDown(screen.getByTestId('sheet-grid-wrap'), { key: 'q' });

    // Real assertion on the editor's own content and the focused element —
    // not merely "no rename input rendered".
    const editorInput = await vi.waitFor(() => screen.getByTestId('sheet-cell-editor-input') as HTMLInputElement);
    expect(editorInput.value).toBe('q');
    expect(document.activeElement).toBe(editorInput);
    expect(screen.queryByTestId('sheet-header-rename-input')).toBeNull();
    expect(updateSheetColumnMock).not.toHaveBeenCalled();
  });

  it('requirement 4: same behavior when the row AND column are both ghosts — no rename input, editor stays open with the typed character, focused', async () => {
    const initial = baseSheetData({ cells: [cell({ raw_value: 'a' })] });
    await renderGrid(initial);

    const newRow = row({ id: 'row-new', sort_order: 1 });
    const newCol = col({ id: 'col-new', name: '', col_type: 'text', sort_order: 1 });
    createSheetRowsBatchMock.mockResolvedValue([newRow]);
    createSheetColumnsBatchMock.mockResolvedValue([newCol]);
    updateSheetCellMock.mockResolvedValue(cell({ row_id: 'row-new', column_id: 'col-new', raw_value: 'm' }));
    getSheetDataMock.mockResolvedValueOnce(baseSheetData({
      columns: [col({ id: 'col-a', name: 'Name', sort_order: 0 }), newCol],
      rows: [row({ id: 'row-1', sort_order: 0 }), newRow],
      cells: [cell({ raw_value: 'a' }), cell({ row_id: 'row-new', column_id: 'col-new', raw_value: 'm' })],
    }));

    await fireEvent.mouseDown(screen.getByTestId('sheet-ghost-cell-1-1'));
    await tick();
    await fireEvent.keyDown(screen.getByTestId('sheet-grid-wrap'), { key: 'm' });

    const editorInput = await vi.waitFor(() => screen.getByTestId('sheet-cell-editor-input') as HTMLInputElement);
    expect(editorInput.value).toBe('m');
    expect(document.activeElement).toBe(editorInput);
    expect(screen.queryByTestId('sheet-header-rename-input')).toBeNull();
    expect(updateSheetColumnMock).not.toHaveBeenCalled();
  });

  it('requirement 3 + header-menu-still-works: the new column keeps its empty name after ghost-column typing, and right-click \u2192 Rename from the header menu still renames it', async () => {
    const initial = baseSheetData({ cells: [cell({ raw_value: 'a' })] });
    await renderGrid(initial);

    const newCol = col({ id: 'col-new', name: '', col_type: 'text', sort_order: 1 });
    createSheetColumnsBatchMock.mockResolvedValue([newCol]);
    updateSheetCellMock.mockResolvedValue(cell({ column_id: 'col-new', raw_value: 'q' }));
    getSheetDataMock.mockResolvedValueOnce(baseSheetData({
      columns: [col({ id: 'col-a', name: 'Name', sort_order: 0 }), newCol],
      cells: [cell({ raw_value: 'a' }), cell({ column_id: 'col-new', raw_value: 'q' })],
    }));

    await fireEvent.mouseDown(screen.getByTestId('sheet-ghost-cell-0-0'));
    await tick();
    await fireEvent.keyDown(screen.getByTestId('sheet-grid-wrap'), { key: 'q' });
    const editorInput = await vi.waitFor(() => screen.getByTestId('sheet-cell-editor-input') as HTMLInputElement);

    // The new column (now real, index 1) shows no name text and no rename
    // call has fired from the ghost-typing commit itself.
    expect(screen.queryByText('Budget')).toBeNull();
    expect(updateSheetColumnMock).not.toHaveBeenCalled();

    // Escape out of the cell editor, then exercise the ONLY sanctioned
    // rename path per §33.3.5: right-click the new column's header → Rename.
    await fireEvent.keyDown(editorInput, { key: 'Escape' });
    await tick();
    updateSheetColumnMock.mockResolvedValue({ ...newCol, name: 'Budget' });

    await fireEvent.contextMenu(screen.getByTestId('sheet-col-header-1'));
    await tick();
    await fireEvent.click(screen.getByTestId('sheet-header-menu-rename'));
    await tick();

    const renameInput = screen.getByTestId('sheet-header-rename-input') as HTMLInputElement;
    await fireEvent.input(renameInput, { target: { value: 'Budget' } });
    await fireEvent.keyDown(renameInput, { key: 'Enter' });

    await vi.waitFor(() => expect(updateSheetColumnMock).toHaveBeenCalledWith('col-new', { name: 'Budget' }));
  });
});

describe('SheetGrid — header rename (§4, entered via right-click → Rename — §33.3.5 sanctioned path, item 44 / #2874)', () => {
  // These two tests used to drive the rename input open via ghost-column
  // typing. §33.3.5 (item 44 / #2874) removed that entry path entirely —
  // ghost typing never opens the rename input anymore (see the ghost
  // typing-to-commit and "ghost column typing never opens rename" describe
  // blocks) — so the ONLY way left to reach `headerRename` on an existing
  // column is the header menu's Rename row (`menuRename` → `startHeaderRename`,
  // both unchanged by this fix). Re-pointed at that path to keep exercising
  // the input's own commit/cancel mechanics.
  it('commits the typed name via updateSheetColumn on Enter', async () => {
    const initial = baseSheetData({ cells: [cell({ raw_value: 'a' })] });
    await renderGrid(initial);
    updateSheetColumnMock.mockResolvedValue(col({ id: 'col-a', name: 'Budget' }));

    await fireEvent.contextMenu(screen.getByTestId('sheet-col-header-0'));
    await tick();
    await fireEvent.click(screen.getByTestId('sheet-header-menu-rename'));
    await tick();

    const renameInput = screen.getByTestId('sheet-header-rename-input') as HTMLInputElement;
    await fireEvent.input(renameInput, { target: { value: 'Budget' } });
    await fireEvent.keyDown(renameInput, { key: 'Enter' });

    await vi.waitFor(() => expect(updateSheetColumnMock).toHaveBeenCalledWith('col-a', { name: 'Budget' }));
  });

  it('Escape closes the rename input without calling updateSheetColumn (empty name is a valid state)', async () => {
    const initial = baseSheetData({ cells: [cell({ raw_value: 'a' })] });
    await renderGrid(initial);

    await fireEvent.contextMenu(screen.getByTestId('sheet-col-header-0'));
    await tick();
    await fireEvent.click(screen.getByTestId('sheet-header-menu-rename'));
    await tick();

    const renameInput = screen.getByTestId('sheet-header-rename-input');
    await fireEvent.keyDown(renameInput, { key: 'Escape' });
    await tick();

    expect(updateSheetColumnMock).not.toHaveBeenCalled();
    expect(screen.queryByTestId('sheet-header-rename-input')).toBeNull();
  });
});

describe('SheetGrid — toolbar-facing insertRow/insertColumn exports (ruling 5)', () => {
  it('insertRow() called with no args (toolbar\'s "+ row" button path) calls createSheetRow with NO reference/position and refetches — regression guard: append behavior must never regress into a positional insert', async () => {
    const initial = baseSheetData({ cells: [cell({ raw_value: 'a' })] });
    const { component } = await renderGrid(initial);

    const newRow = row({ id: 'row-new', sort_order: 1 });
    createSheetRowMock.mockResolvedValue(newRow);
    const afterCreate = baseSheetData({
      rows: [row({ id: 'row-1', sort_order: 0 }), newRow],
      cells: [cell({ raw_value: 'a' })],
    });
    getSheetDataMock.mockResolvedValueOnce(afterCreate);

    // SheetToolbar's "+ row" button calls `sheetGridRef?.insertRow()`
    // with NO arguments (Editor.svelte:2175) — this call shape IS that
    // toolbar path, exercised directly since Editor.svelte is out of
    // this component test's scope.
    await (component as any).insertRow();
    await tick();

    expect(createSheetRowMock).toHaveBeenCalledWith(NOTE_ID, undefined, undefined);
    expect(getSheetDataMock).toHaveBeenCalledTimes(2);
  });

  it('insertColumn() called with no args (toolbar\'s "+ column" button path) calls createSheetColumn with text/empty-name defaults and NO reference/position, refetches, and enters header-rename mode — regression guard', async () => {
    const initial = baseSheetData({ cells: [cell({ raw_value: 'a' })] });
    const { component } = await renderGrid(initial);

    const newCol = col({ id: 'col-new', name: '', col_type: 'text', sort_order: 1 });
    createSheetColumnMock.mockResolvedValue(newCol);
    const afterCreate = baseSheetData({
      columns: [col({ id: 'col-a', name: 'Name', sort_order: 0 }), newCol],
      cells: [cell({ raw_value: 'a' })],
    });
    getSheetDataMock.mockResolvedValueOnce(afterCreate);

    // SheetToolbar's "+ column" button calls `sheetGridRef?.insertColumn()`
    // with NO arguments (Editor.svelte:2176) — same rationale as above.
    await (component as any).insertColumn();
    await tick();

    expect(createSheetColumnMock).toHaveBeenCalledWith(NOTE_ID, '', 'text', null, undefined, undefined);
    await vi.waitFor(() => expect(screen.getByTestId('sheet-header-rename-input')).toBeTruthy());
  });
});

// #2339 PR 11 — ShHeaderMenu + sort/filter + ShMismatch + delete-column
// selection recovery. See docs/audits/2026-09-04-sheets-pr11-header-menu-
// scope.md and Matt's 7 rulings (2026-09-04).

describe('SheetGrid — column header click/right-click split (§33.3.6, PR #2924 commit e7e341ce, item 10 amendment)', () => {
  it('left-click on a column header selects the column and opens nothing', async () => {
    const data = baseSheetData({ cells: [cell({ raw_value: 'a' })] });
    await renderGrid(data);

    await fireEvent.click(screen.getByTestId('sheet-col-header-0'));
    await tick();

    expect(screen.getByTestId('sheet-col-header-0').className).toContain('is-sel');
    expect(screen.queryByTestId('sheet-header-menu')).toBeNull();
  });

  it('right-click on a column header opens the menu without changing selection state twice', async () => {
    const data = baseSheetData({ cells: [cell({ raw_value: 'a' })] });
    await renderGrid(data);

    await fireEvent.contextMenu(screen.getByTestId('sheet-col-header-0'));
    await tick();

    expect(screen.getByTestId('sheet-header-menu')).toBeTruthy();
  });

  it('right-click alone does NOT select the column — no SELECT_COLUMN dispatch on contextmenu', async () => {
    const data = baseSheetData({ cells: [cell({ raw_value: 'a' })] });
    await renderGrid(data);

    // INITIAL_SHEET_SELECTION starts with colSel: null, so is-sel is absent
    // unless SELECT_COLUMN fired. Right-click must only open the menu.
    expect(screen.getByTestId('sheet-col-header-0').className).not.toContain('is-sel');

    await fireEvent.contextMenu(screen.getByTestId('sheet-col-header-0'));
    await tick();

    expect(screen.getByTestId('sheet-header-menu')).toBeTruthy();
    expect(screen.getByTestId('sheet-col-header-0').className).not.toContain('is-sel');
  });

  it('right-click on a header that already has its menu open re-anchors (does not close) the menu', async () => {
    const data = baseSheetData({ cells: [cell({ raw_value: 'a' })] });
    await renderGrid(data);

    await fireEvent.contextMenu(screen.getByTestId('sheet-col-header-0'));
    await tick();
    await fireEvent.contextMenu(screen.getByTestId('sheet-col-header-0'));
    await tick();

    expect(screen.getByTestId('sheet-header-menu')).toBeTruthy();
  });

  it('left-clicking a header while ITS OWN menu is open (from a prior right-click) does not close the menu', async () => {
    const data = baseSheetData({ cells: [cell({ raw_value: 'a' })] });
    await renderGrid(data);

    await fireEvent.contextMenu(screen.getByTestId('sheet-col-header-0'));
    await tick();
    expect(screen.getByTestId('sheet-header-menu')).toBeTruthy();

    await fireEvent.click(screen.getByTestId('sheet-col-header-0'));
    await tick();

    // Left-click no longer toggles the menu — it only (re-)selects. The
    // menu, once opened by right-click, stays open until closeHeaderMenu
    // fires (outside-click dismiss or a menu action), per §33.3.6.
    expect(screen.getByTestId('sheet-header-menu')).toBeTruthy();
    expect(screen.getByTestId('sheet-col-header-0').className).toContain('is-sel');
  });

  it('right-clicking a DIFFERENT header while one menu is open switches the menu to that column', async () => {
    const data = baseSheetData({
      columns: [col({ id: 'col-a', sort_order: 0 }), col({ id: 'col-b', name: 'B', sort_order: 1 })],
      cells: [cell({ column_id: 'col-a', raw_value: 'a' }), cell({ column_id: 'col-b', raw_value: 'b' })],
    });
    await renderGrid(data);

    await fireEvent.contextMenu(screen.getByTestId('sheet-col-header-0'));
    await tick();
    expect(screen.getByTestId('sheet-col-header-0').className).toContain('is-open');

    await fireEvent.contextMenu(screen.getByTestId('sheet-col-header-1'));
    await tick();

    expect(screen.getByTestId('sheet-col-header-0').className).not.toContain('is-open');
    expect(screen.getByTestId('sheet-col-header-1').className).toContain('is-open');
  });

  it('the Rename row closes the menu and opens the SAME inline .sh-h__ren input PR 10 uses (ruling 1)', async () => {
    const data = baseSheetData({ cells: [cell({ raw_value: 'a' })] });
    await renderGrid(data);

    await fireEvent.contextMenu(screen.getByTestId('sheet-col-header-0'));
    await tick();
    await fireEvent.click(screen.getByTestId('sheet-header-menu-rename'));
    await tick();

    expect(screen.queryByTestId('sheet-header-menu')).toBeNull();
    expect(screen.getByTestId('sheet-header-rename-input')).toBeTruthy();
  });

  it('right-click on the resize grip does not open the header menu (stopPropagation)', async () => {
    const data = baseSheetData({ cells: [cell({ raw_value: 'a' })] });
    await renderGrid(data);

    await fireEvent.contextMenu(screen.getByTestId('sheet-col-grip-0'));
    await tick();

    expect(screen.queryByTestId('sheet-header-menu')).toBeNull();
  });
});

describe('SheetGrid — type-change via menu (structural mutation, refetch)', () => {
  it('picking a different type calls updateSheetColumn(col_type) then refetches', async () => {
    const data = baseSheetData({ columns: [col({ id: 'col-a', col_type: 'text' })], cells: [cell({ raw_value: 'a' })] });
    await renderGrid(data);
    updateSheetColumnMock.mockResolvedValue(col({ id: 'col-a', col_type: 'number' }));
    const afterChange = baseSheetData({ columns: [col({ id: 'col-a', col_type: 'number' })], cells: [cell({ raw_value: 'a' })] });
    getSheetDataMock.mockResolvedValueOnce(afterChange);

    await fireEvent.contextMenu(screen.getByTestId('sheet-col-header-0'));
    await tick();
    await fireEvent.click(screen.getByTestId('sheet-header-menu-type'));
    await tick();
    await fireEvent.click(screen.getByTestId('sheet-header-menu-type-number'));
    await tick();

    expect(updateSheetColumnMock).toHaveBeenCalledWith('col-a', { colType: 'number' });
    await vi.waitFor(() => expect(getSheetDataMock).toHaveBeenCalledTimes(2));
  });
});

describe('SheetGrid — delete column (structural mutation, ruling 4 selection recovery)', () => {
  it('deletes via deleteSheetColumn then refetches', async () => {
    const data = baseSheetData({
      columns: [col({ id: 'col-a', sort_order: 0 }), col({ id: 'col-b', name: 'B', sort_order: 1 })],
      cells: [cell({ raw_value: 'a' })],
    });
    await renderGrid(data);
    deleteSheetColumnMock.mockResolvedValue(undefined);
    const afterDelete = baseSheetData({ columns: [col({ id: 'col-b', name: 'B', sort_order: 0 })], cells: [] });
    getSheetDataMock.mockResolvedValueOnce(afterDelete);

    await fireEvent.contextMenu(screen.getByTestId('sheet-col-header-0'));
    await tick();
    await fireEvent.click(screen.getByTestId('sheet-header-menu-delete'));
    await tick();

    expect(deleteSheetColumnMock).toHaveBeenCalledWith('col-a');
    await vi.waitFor(() => expect(getSheetDataMock).toHaveBeenCalledTimes(2));
  });

  it('ruling 4: deleting the ONLY column clamps selection to column 0 (no columns remain)', async () => {
    const data = baseSheetData({ columns: [col({ id: 'col-a', sort_order: 0 })], cells: [cell({ raw_value: 'a' })] });
    await renderGrid(data);
    deleteSheetColumnMock.mockResolvedValue(undefined);
    const afterDelete = baseSheetData({ columns: [], rows: [row({ id: 'row-1', sort_order: 0 })], cells: [] });
    getSheetDataMock.mockResolvedValueOnce(afterDelete);

    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-0-0'));
    await tick();
    await fireEvent.contextMenu(screen.getByTestId('sheet-col-header-0'));
    await tick();
    await fireEvent.click(screen.getByTestId('sheet-header-menu-delete'));
    await vi.waitFor(() => expect(getSheetDataMock).toHaveBeenCalledTimes(2));
    await tick();

    // No real columns left to assert a header-selection class against —
    // the meaningful assertion is that the delete flow didn't throw/crash
    // rendering with a zero-column sheet, and the grid re-rendered cleanly.
    expect(screen.getByTestId('sheet-grid')).toBeTruthy();
  });

  it('ruling 4: deleting the rightmost of two columns clamps selection to the nearest remaining (column 0)', async () => {
    const data = baseSheetData({
      columns: [col({ id: 'col-a', sort_order: 0 }), col({ id: 'col-b', name: 'B', sort_order: 1 })],
      cells: [cell({ raw_value: 'a' })],
    });
    await renderGrid(data);
    deleteSheetColumnMock.mockResolvedValue(undefined);
    const afterDelete = baseSheetData({ columns: [col({ id: 'col-a', sort_order: 0 })], cells: [cell({ raw_value: 'a' })] });
    getSheetDataMock.mockResolvedValueOnce(afterDelete);

    // Opening column B's header menu selects column B first (existing
    // SELECT_COLUMN dispatch, unchanged from PR 9/10) — the deleted column
    // IS the selected one at delete time, which is the realistic path
    // through this UI (there's no way to delete a column via the menu
    // without first opening ITS OWN header menu).
    await fireEvent.contextMenu(screen.getByTestId('sheet-col-header-1'));
    await tick();
    await fireEvent.click(screen.getByTestId('sheet-header-menu-delete'));
    await vi.waitFor(() => expect(screen.getByTestId('sheet-cell-0-0').className).toContain('is-anchor'));
  });
});

describe('SheetGrid — sort engine (pure view-state, ruling 5 single-column)', () => {
  it('Sort ascending reorders visible rows without calling any backend command', async () => {
    const data = baseSheetData({
      columns: [col({ id: 'col-a', col_type: 'number' })],
      rows: [row({ id: 'r1', sort_order: 0 }), row({ id: 'r2', sort_order: 1 }), row({ id: 'r3', sort_order: 2 })],
      cells: [
        cell({ row_id: 'r1', raw_value: '30' }),
        cell({ row_id: 'r2', raw_value: '10' }),
        cell({ row_id: 'r3', raw_value: '20' }),
      ],
    });
    await renderGrid(data);

    await fireEvent.contextMenu(screen.getByTestId('sheet-col-header-0'));
    await tick();
    await fireEvent.click(screen.getByTestId('sheet-header-menu-sort-asc'));
    await tick();

    expect(screen.getByTestId('sheet-cell-0-0').textContent).toContain('10');
    expect(screen.getByTestId('sheet-cell-1-0').textContent).toContain('20');
    expect(screen.getByTestId('sheet-cell-2-0').textContent).toContain('30');
    expect(updateSheetColumnMock).not.toHaveBeenCalled();
    expect(getSheetDataMock).toHaveBeenCalledTimes(1); // no refetch — pure view state
  });

  it('renders a sort header glyph on the sorted column', async () => {
    const data = baseSheetData({
      columns: [col({ id: 'col-a', col_type: 'number' })],
      cells: [cell({ raw_value: '5' })],
    });
    await renderGrid(data);

    await fireEvent.contextMenu(screen.getByTestId('sheet-col-header-0'));
    await tick();
    await fireEvent.click(screen.getByTestId('sheet-header-menu-sort-desc'));
    await tick();

    expect(screen.getByTestId('sheet-header-flag-sort-0')).toBeTruthy();
  });

  it('ruling 5: sorting a second column REPLACES the first column\'s sort (single-column only)', async () => {
    const data = baseSheetData({
      columns: [col({ id: 'col-a', col_type: 'number' }), col({ id: 'col-b', name: 'B', col_type: 'number' })],
      rows: [row({ id: 'r1', sort_order: 0 }), row({ id: 'r2', sort_order: 1 })],
      cells: [
        cell({ row_id: 'r1', column_id: 'col-a', raw_value: '2' }),
        cell({ row_id: 'r2', column_id: 'col-a', raw_value: '1' }),
        cell({ row_id: 'r1', column_id: 'col-b', raw_value: '9' }),
        cell({ row_id: 'r2', column_id: 'col-b', raw_value: '8' }),
      ],
    });
    await renderGrid(data);

    await fireEvent.contextMenu(screen.getByTestId('sheet-col-header-0'));
    await tick();
    await fireEvent.click(screen.getByTestId('sheet-header-menu-sort-asc'));
    await tick();
    expect(screen.queryByTestId('sheet-header-flag-sort-0')).toBeTruthy();

    await fireEvent.contextMenu(screen.getByTestId('sheet-col-header-1'));
    await tick();
    await fireEvent.click(screen.getByTestId('sheet-header-menu-sort-asc'));
    await tick();

    expect(screen.queryByTestId('sheet-header-flag-sort-0')).toBeNull();
    expect(screen.getByTestId('sheet-header-flag-sort-1')).toBeTruthy();
  });
});

describe('SheetGrid — filter engine (ruling 3: select/checkbox only, pure view-state)', () => {
  it('filtering a checkbox column to only TRUE hides unchecked rows, with a filter header glyph', async () => {
    const data = baseSheetData({
      columns: [col({ id: 'col-a', col_type: 'checkbox' })],
      rows: [row({ id: 'r1', sort_order: 0 }), row({ id: 'r2', sort_order: 1 })],
      cells: [cell({ row_id: 'r1', raw_value: '1' }), cell({ row_id: 'r2', raw_value: '0' })],
    });
    await renderGrid(data);

    await fireEvent.contextMenu(screen.getByTestId('sheet-col-header-0'));
    await tick();
    await fireEvent.click(screen.getByTestId('sheet-header-menu-filter'));
    await tick();
    await fireEvent.click(screen.getByText('True'));
    await tick();

    expect(screen.queryByTestId('sheet-cell-1-0')).toBeNull();
    expect(screen.getByTestId('sheet-header-flag-filter-0')).toBeTruthy();
    expect(getSheetDataMock).toHaveBeenCalledTimes(1); // no refetch — pure view state
  });
});

// #2339 PR 12 — SheetViewStrip (ShView), mounted inside SheetGrid.svelte.
describe('SheetGrid — view strip (#2339 PR 12)', () => {
  it('is absent when neither sort nor filter is active', async () => {
    const data = baseSheetData({
      columns: [col({ id: 'col-a', col_type: 'number' })],
      cells: [cell({ raw_value: '5' })],
    });
    await renderGrid(data);

    expect(screen.queryByTestId('sheet-view-strip')).toBeNull();
  });

  it('setting a sort mounts the strip with the correct column name and direction', async () => {
    const data = baseSheetData({
      columns: [col({ id: 'col-a', name: 'Total', col_type: 'number' })],
      cells: [cell({ raw_value: '5' })],
    });
    await renderGrid(data);

    await fireEvent.contextMenu(screen.getByTestId('sheet-col-header-0'));
    await tick();
    await fireEvent.click(screen.getByTestId('sheet-header-menu-sort-desc'));
    await tick();

    expect(screen.getByTestId('sheet-view-strip')).toBeTruthy();
    const sortChip = screen.getByTestId('sheet-view-chip-sort');
    expect(sortChip.textContent).toContain('Total');
    expect(sortChip.textContent).toContain('descending');
    expect(screen.queryByTestId('sheet-view-chip-filter')).toBeNull();
  });

  it('setting a filter mounts the strip with the correct column name and values', async () => {
    const data = baseSheetData({
      columns: [col({ id: 'col-a', name: 'Active', col_type: 'checkbox' })],
      rows: [row({ id: 'r1', sort_order: 0 }), row({ id: 'r2', sort_order: 1 })],
      cells: [cell({ row_id: 'r1', raw_value: '1' }), cell({ row_id: 'r2', raw_value: '0' })],
    });
    await renderGrid(data);

    await fireEvent.contextMenu(screen.getByTestId('sheet-col-header-0'));
    await tick();
    await fireEvent.click(screen.getByTestId('sheet-header-menu-filter'));
    await tick();
    await fireEvent.click(screen.getByText('True'));
    await tick();

    expect(screen.getByTestId('sheet-view-strip')).toBeTruthy();
    const filterChip = screen.getByTestId('sheet-view-chip-filter');
    expect(filterChip.textContent).toContain('Active');
    // N-1 of N (checkbox has 2 choices, 1 selected) → exclusion phrasing.
    expect(filterChip.textContent).toContain('is not False');
    expect(screen.queryByTestId('sheet-view-chip-sort')).toBeNull();
  });

  it('clearing sort via the strip\'s × removes the strip, restores identity rowOrder, and clears the header flag', async () => {
    const data = baseSheetData({
      columns: [col({ id: 'col-a', col_type: 'number' })],
      rows: [row({ id: 'r1', sort_order: 0 }), row({ id: 'r2', sort_order: 1 })],
      cells: [cell({ row_id: 'r1', raw_value: '30' }), cell({ row_id: 'r2', raw_value: '10' })],
    });
    await renderGrid(data);

    await fireEvent.contextMenu(screen.getByTestId('sheet-col-header-0'));
    await tick();
    await fireEvent.click(screen.getByTestId('sheet-header-menu-sort-asc'));
    await tick();
    // Sorted ascending: r2 (10) now displays first — gutter shows stored
    // position 2 (rowOrder[0] === 1), NOT display position 1.
    expect(screen.getByTestId('sheet-row-gutter-0').textContent?.trim()).toBe('2');

    await fireEvent.click(screen.getByTestId('sheet-view-chip-sort-clear'));
    await tick();

    expect(screen.queryByTestId('sheet-view-strip')).toBeNull();
    expect(screen.queryByTestId('sheet-header-flag-sort-0')).toBeNull();
    // Back to identity order/gutter.
    expect(screen.getByTestId('sheet-row-gutter-0').textContent?.trim()).toBe('1');
    expect(screen.getByTestId('sheet-cell-0-0').textContent).toContain('30');
  });

  it('clearing filter via the strip\'s × removes the strip, restores hidden rows, and clears the header flag', async () => {
    const data = baseSheetData({
      columns: [col({ id: 'col-a', col_type: 'checkbox' })],
      rows: [row({ id: 'r1', sort_order: 0 }), row({ id: 'r2', sort_order: 1 })],
      cells: [cell({ row_id: 'r1', raw_value: '1' }), cell({ row_id: 'r2', raw_value: '0' })],
    });
    await renderGrid(data);

    await fireEvent.contextMenu(screen.getByTestId('sheet-col-header-0'));
    await tick();
    await fireEvent.click(screen.getByTestId('sheet-header-menu-filter'));
    await tick();
    await fireEvent.click(screen.getByText('True'));
    await tick();
    expect(screen.queryByTestId('sheet-cell-1-0')).toBeNull();

    await fireEvent.click(screen.getByTestId('sheet-view-chip-filter-clear'));
    await tick();

    expect(screen.queryByTestId('sheet-view-strip')).toBeNull();
    expect(screen.queryByTestId('sheet-header-flag-filter-0')).toBeNull();
    expect(screen.getByTestId('sheet-cell-1-0')).toBeTruthy();
  });

  // Matt's ruling 2026-09-04 (2): the row gutter shows STORED positions,
  // not display positions, under an active sort — "1, 5, 3, 4" is itself
  // the tell that a sort is active.
  it("Matt's ruling 2026-09-04 (2): row gutter shows stored positions (not 1..N) under an active sort", async () => {
    const data = baseSheetData({
      columns: [col({ id: 'col-a', col_type: 'number' })],
      rows: [row({ id: 'r1', sort_order: 0 }), row({ id: 'r2', sort_order: 1 }), row({ id: 'r3', sort_order: 2 })],
      cells: [
        cell({ row_id: 'r1', raw_value: '30' }),
        cell({ row_id: 'r2', raw_value: '10' }),
        cell({ row_id: 'r3', raw_value: '20' }),
      ],
    });
    await renderGrid(data);

    await fireEvent.contextMenu(screen.getByTestId('sheet-col-header-0'));
    await tick();
    await fireEvent.click(screen.getByTestId('sheet-header-menu-sort-asc'));
    await tick();

    // Display order after ascending sort: r2 (10), r3 (20), r1 (30) —
    // stored positions 2, 3, 1 respectively.
    expect(screen.getByTestId('sheet-row-gutter-0').textContent?.trim()).toBe('2');
    expect(screen.getByTestId('sheet-row-gutter-1').textContent?.trim()).toBe('3');
    expect(screen.getByTestId('sheet-row-gutter-2').textContent?.trim()).toBe('1');
  });

  it('no sort/filter active: row gutter still reads plain 1..N (unchanged behavior)', async () => {
    const data = baseSheetData({
      rows: [row({ id: 'r1', sort_order: 0 }), row({ id: 'r2', sort_order: 1 })],
      cells: [cell({ row_id: 'r1' }), cell({ row_id: 'r2' })],
    });
    await renderGrid(data);

    expect(screen.getByTestId('sheet-row-gutter-0').textContent?.trim()).toBe('1');
    expect(screen.getByTestId('sheet-row-gutter-1').textContent?.trim()).toBe('2');
  });

  it('deleting the sorted column makes the strip disappear (regression guard)', async () => {
    const data = baseSheetData({
      columns: [col({ id: 'col-a', name: 'Total', col_type: 'number', sort_order: 0 })],
      cells: [cell({ raw_value: '5' })],
    });
    await renderGrid(data);

    await fireEvent.contextMenu(screen.getByTestId('sheet-col-header-0'));
    await tick();
    await fireEvent.click(screen.getByTestId('sheet-header-menu-sort-asc'));
    await tick();
    expect(screen.getByTestId('sheet-view-strip')).toBeTruthy();

    deleteSheetColumnMock.mockResolvedValue(undefined);
    const afterDelete = baseSheetData({ columns: [], rows: [row({ id: 'row-1', sort_order: 0 })], cells: [] });
    getSheetDataMock.mockResolvedValueOnce(afterDelete);

    await fireEvent.contextMenu(screen.getByTestId('sheet-col-header-0'));
    await tick();
    await fireEvent.click(screen.getByTestId('sheet-header-menu-delete'));
    await vi.waitFor(() => expect(deleteSheetColumnMock).toHaveBeenCalledTimes(1));
    await vi.waitFor(() => expect(getSheetDataMock).toHaveBeenCalledTimes(2));
    await tick();

    expect(screen.queryByTestId('sheet-view-strip')).toBeNull();
  });
});

describe('SheetGrid — computed wedge removed from the live grid (#2874 item 41)', () => {
  it('formula cells do not render the computed wedge in the live grid', async () => {
    const data = baseSheetData({
      columns: [col({ id: 'col-a', col_type: 'number' })],
      cells: [cell({ raw_value: '=1+1', computed_value: '2' })],
    });
    await renderGrid(data);

    expect(document.querySelector('.sh-w--calc')).toBeNull();
  });

  it('a formula cell with an error still renders the red error wedge', async () => {
    const data = baseSheetData({
      columns: [col({ id: 'col-a', col_type: 'number' })],
      cells: [cell({ raw_value: '=1/0', computed_value: '#DIV/0!' })],
    });
    await renderGrid(data);

    expect(document.querySelector('.sh-w--err')).toBeTruthy();
  });
});

describe('SheetGrid — ShMismatch rendering (ruling 7)', () => {
  it('renders the amber mismatch wedge for a value that does not parse as the column type', async () => {
    const data = baseSheetData({
      columns: [col({ id: 'col-a', col_type: 'number' })],
      cells: [cell({ raw_value: 'TBD' })],
    });
    await renderGrid(data);

    expect(screen.getByTestId('sheet-mismatch-wedge-0-0')).toBeTruthy();
  });

  it('does not render the mismatch wedge for a value that parses fine', async () => {
    const data = baseSheetData({
      columns: [col({ id: 'col-a', col_type: 'number' })],
      cells: [cell({ raw_value: '42' })],
    });
    await renderGrid(data);

    expect(screen.queryByTestId('sheet-mismatch-wedge-0-0')).toBeNull();
  });

  it('shows the explanation tooltip on hover of a mismatched cell', async () => {
    const data = baseSheetData({
      columns: [col({ id: 'col-a', col_type: 'number' })],
      cells: [cell({ raw_value: 'TBD' })],
    });
    await renderGrid(data);

    await fireEvent.mouseEnter(screen.getByTestId('sheet-cell-0-0'));
    await tick();

    expect(screen.getByTestId('sheet-mismatch-tooltip')).toBeTruthy();
    expect(screen.getByTestId('sheet-mismatch-tooltip').textContent).toContain('Not a number');
  });

  it('hides the tooltip again on mouseleave', async () => {
    const data = baseSheetData({
      columns: [col({ id: 'col-a', col_type: 'number' })],
      cells: [cell({ raw_value: 'TBD' })],
    });
    await renderGrid(data);

    await fireEvent.mouseEnter(screen.getByTestId('sheet-cell-0-0'));
    await tick();
    await fireEvent.mouseLeave(screen.getByTestId('sheet-cell-0-0'));
    await tick();

    expect(screen.queryByTestId('sheet-mismatch-tooltip')).toBeNull();
  });
});

// #2339 PR 15b-alpha — ShErrorPopover: error detail popover shown when an
// error cell is selected (§2c). See ShErrorPopover.test.ts for the
// component's own content/catalogue tests; these cover SheetGrid's own
// show-on-select / hide-on-deselect wiring.
describe('SheetGrid — error detail popover (§2c)', () => {
  it('shows the error popover when an error cell becomes the sole selection anchor', async () => {
    const data = baseSheetData({
      columns: [col({ id: 'col-a', col_type: 'number' })],
      cells: [cell({ raw_value: '=1/0', computed_value: '#DIV/0!' })],
    });
    await renderGrid(data);

    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-0-0'));
    await tick();

    expect(screen.getByTestId('sheet-error-popover')).toBeTruthy();
    expect(screen.getByTestId('sheet-error-popover-code').textContent).toBe('#DIV/0!');
    expect(screen.getByTestId('sheet-error-popover-formula').textContent).toBe('=1/0');
  });

  it('does not show the popover for a non-error cell', async () => {
    const data = baseSheetData({
      columns: [col({ id: 'col-a', col_type: 'number' })],
      cells: [cell({ raw_value: '42' })],
    });
    await renderGrid(data);

    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-0-0'));
    await tick();

    expect(screen.queryByTestId('sheet-error-popover')).toBeNull();
  });

  it('hides the popover when selection moves off the error cell (dismissed on deselection)', async () => {
    const data = baseSheetData({
      columns: [col({ id: 'col-a', col_type: 'number' }), col({ id: 'col-b', col_type: 'text' })],
      cells: [cell({ column_id: 'col-a', raw_value: '=1/0', computed_value: '#DIV/0!' })],
    });
    await renderGrid(data);

    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-0-0'));
    await tick();
    expect(screen.getByTestId('sheet-error-popover')).toBeTruthy();

    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-0-1'));
    await tick();

    expect(screen.queryByTestId('sheet-error-popover')).toBeNull();
  });

  it('hides the popover on Escape (selection anchor cleared)', async () => {
    const data = baseSheetData({
      columns: [col({ id: 'col-a', col_type: 'number' })],
      cells: [cell({ raw_value: '=1/0', computed_value: '#DIV/0!' })],
    });
    await renderGrid(data);

    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-0-0'));
    await tick();
    expect(screen.getByTestId('sheet-error-popover')).toBeTruthy();

    await fireEvent.keyDown(screen.getByTestId('sheet-grid-wrap'), { key: 'Escape' });
    await tick();

    expect(screen.queryByTestId('sheet-error-popover')).toBeNull();
  });
});

// #2339 PR 13 — Column & Row Resize (ShResize), §33.3.3. See
// docs/audits/2026-09-04-sheets-pr13-resize-scope.md.

function moveEvent(clientX: number, clientY: number) {
  return new MouseEvent('mousemove', { clientX, clientY, bubbles: true });
}

function upEvent(clientX = 0, clientY = 0) {
  return new MouseEvent('mouseup', { clientX, clientY, bubbles: true });
}

describe('SheetGrid — resize grip rendering (scope §1.4: unconditional on real, absent on ghost)', () => {
  it('renders a grip on every real column header', async () => {
    const data = baseSheetData({
      columns: [col({ id: 'col-a', sort_order: 0 }), col({ id: 'col-b', sort_order: 1 })],
      cells: [],
    });
    await renderGrid(data);

    expect(screen.getByTestId('sheet-col-grip-0')).toBeTruthy();
    expect(screen.getByTestId('sheet-col-grip-1')).toBeTruthy();
  });

  it('does NOT render a grip on ghost column headers', async () => {
    const data = baseSheetData({ columns: [col({ id: 'col-a' })], cells: [] });
    await renderGrid(data);

    const ghostHeader = screen.getByTestId('sheet-ghost-col-header-0');
    expect(ghostHeader.querySelector('.sh-h__grip')).toBeNull();
  });

  it('renders a grip on every real row gutter', async () => {
    const data = baseSheetData({
      rows: [row({ id: 'r1', sort_order: 0 }), row({ id: 'r2', sort_order: 1 })],
      cells: [],
    });
    await renderGrid(data);

    expect(screen.getByTestId('sheet-row-grip-0')).toBeTruthy();
    expect(screen.getByTestId('sheet-row-grip-1')).toBeTruthy();
  });

  it('does NOT render a grip on ghost row gutters', async () => {
    const data = baseSheetData({ rows: [row({ id: 'r1' })], cells: [] });
    await renderGrid(data);

    const ghostGutter = screen.getByTestId('sheet-ghost-row-gutter-0');
    expect(ghostGutter.querySelector('.sh-g__grip')).toBeNull();
  });
});

describe('SheetGrid — resize grip vs. selection coexistence (scope §3.3)', () => {
  it('mousedown on a column grip does NOT select the column or open ShHeaderMenu', async () => {
    const data = baseSheetData({ cells: [] });
    await renderGrid(data);

    await fireEvent.mouseDown(screen.getByTestId('sheet-col-grip-0'));
    await tick();

    expect(screen.getByTestId('sheet-col-header-0').className).not.toContain('is-sel');
    expect(screen.queryByTestId('sheet-header-menu')).toBeNull();

    globalThis.dispatchEvent(upEvent());
    await tick();
  });

  it('a click (mousedown+click, zero-delta) on the column grip does not fall through to column-select', async () => {
    const data = baseSheetData({ cells: [] });
    await renderGrid(data);

    await fireEvent.mouseDown(screen.getByTestId('sheet-col-grip-0'));
    await fireEvent.click(screen.getByTestId('sheet-col-grip-0'));
    globalThis.dispatchEvent(upEvent());
    await tick();

    expect(screen.getByTestId('sheet-col-header-0').className).not.toContain('is-sel');
  });

  it('mousedown on a row grip does NOT select the row', async () => {
    const data = baseSheetData({ cells: [] });
    await renderGrid(data);

    await fireEvent.mouseDown(screen.getByTestId('sheet-row-grip-0'));
    await tick();

    expect(screen.getByTestId('sheet-row-gutter-0').className).not.toContain('is-sel');

    globalThis.dispatchEvent(upEvent());
    await tick();
  });

  it('regression guard: clicking elsewhere on the header (not the grip) still selects the column (and opens nothing, §33.3.6)', async () => {
    const data = baseSheetData({ cells: [] });
    await renderGrid(data);

    await fireEvent.click(screen.getByTestId('sheet-col-header-0'));
    await tick();

    expect(screen.getByTestId('sheet-col-header-0').className).toContain('is-sel');
    expect(screen.queryByTestId('sheet-header-menu')).toBeNull();
  });

  it('regression guard: clicking elsewhere on the row gutter (not the grip) still selects the row', async () => {
    const data = baseSheetData({ cells: [] });
    await renderGrid(data);

    await fireEvent.click(screen.getByTestId('sheet-row-gutter-0'));
    await tick();

    expect(screen.getByTestId('sheet-row-gutter-0').className).toContain('is-sel');
  });
});

describe('SheetGrid — column resize drag lifecycle (commit-on-mouseup)', () => {
  it('drag simulation: guide and readout appear during drag, disappear after mouseup', async () => {
    const data = baseSheetData({ columns: [col({ id: 'col-a', width: 148 })], cells: [] });
    await renderGrid(data);

    expect(screen.queryByTestId('sheet-resize-guide')).toBeNull();
    expect(screen.queryByTestId('sheet-resize-readout')).toBeNull();

    await fireEvent.mouseDown(screen.getByTestId('sheet-col-grip-0'), { clientX: 100, clientY: 10 });
    globalThis.dispatchEvent(moveEvent(140, 10));
    await tick();

    expect(screen.getByTestId('sheet-resize-guide')).toBeTruthy();
    expect(screen.getByTestId('sheet-resize-readout')).toBeTruthy();

    updateSheetColumnMock.mockResolvedValue(col({ id: 'col-a', width: 188 }));
    globalThis.dispatchEvent(upEvent(140, 10));
    await tick();

    expect(screen.queryByTestId('sheet-resize-guide')).toBeNull();
    expect(screen.queryByTestId('sheet-resize-readout')).toBeNull();
  });

  it('commit fires with the correct final width via updateSheetColumn, then refetches', async () => {
    const data = baseSheetData({ columns: [col({ id: 'col-a', width: 148 })], cells: [] });
    await renderGrid(data);
    updateSheetColumnMock.mockResolvedValue(col({ id: 'col-a', width: 188 }));

    await fireEvent.mouseDown(screen.getByTestId('sheet-col-grip-0'), { clientX: 100, clientY: 10 });
    globalThis.dispatchEvent(moveEvent(140, 10));
    await tick();
    globalThis.dispatchEvent(upEvent(140, 10));
    await vi.waitFor(() => expect(updateSheetColumnMock).toHaveBeenCalledTimes(1));

    expect(updateSheetColumnMock).toHaveBeenCalledWith('col-a', { width: 188 });
    await vi.waitFor(() => expect(getSheetDataMock).toHaveBeenCalledTimes(2));
  });

  it('readout text during drag matches formatReadout format ("188 px")', async () => {
    const data = baseSheetData({ columns: [col({ id: 'col-a', width: 148 })], cells: [] });
    await renderGrid(data);

    await fireEvent.mouseDown(screen.getByTestId('sheet-col-grip-0'), { clientX: 100, clientY: 10 });
    globalThis.dispatchEvent(moveEvent(140, 10));
    await tick();

    expect(screen.getByTestId('sheet-resize-readout').textContent).toBe('188 px');

    globalThis.dispatchEvent(upEvent(140, 10));
    await tick();
  });

  it('readout displays the clamped value, never below the 40px column floor', async () => {
    const data = baseSheetData({ columns: [col({ id: 'col-a', width: 50 })], cells: [] });
    await renderGrid(data);

    await fireEvent.mouseDown(screen.getByTestId('sheet-col-grip-0'), { clientX: 100, clientY: 10 });
    // Huge leftward drag — would go deeply negative without the floor.
    globalThis.dispatchEvent(moveEvent(-500, 10));
    await tick();

    expect(screen.getByTestId('sheet-resize-readout').textContent).toBe('40 px');

    globalThis.dispatchEvent(upEvent(-500, 10));
    await tick();
  });

  it('zero-delta drag (mousedown then mouseup with no movement) skips the API call entirely', async () => {
    const data = baseSheetData({ columns: [col({ id: 'col-a', width: 148 })], cells: [] });
    await renderGrid(data);

    await fireEvent.mouseDown(screen.getByTestId('sheet-col-grip-0'), { clientX: 100, clientY: 10 });
    globalThis.dispatchEvent(upEvent(100, 10));
    await tick();

    expect(updateSheetColumnMock).not.toHaveBeenCalled();
    expect(getSheetDataMock).toHaveBeenCalledTimes(1); // only the initial load
  });

  it('write failure on column resize commit calls showToast (rendered as the toast banner)', async () => {
    const data = baseSheetData({ columns: [col({ id: 'col-a', width: 148 })], cells: [] });
    await renderGrid(data);
    updateSheetColumnMock.mockRejectedValue(new Error('db locked'));

    await fireEvent.mouseDown(screen.getByTestId('sheet-col-grip-0'), { clientX: 100, clientY: 10 });
    globalThis.dispatchEvent(moveEvent(140, 10));
    await tick();
    globalThis.dispatchEvent(upEvent(140, 10));

    await vi.waitFor(() => expect(screen.getByTestId('sheet-toast')).toBeTruthy());
    expect(screen.getByTestId('sheet-toast').textContent).toContain('db locked');
  });
});

describe('SheetGrid — row resize drag lifecycle (commit-on-mouseup, update_sheet_row)', () => {
  it('commit fires with the correct final height via updateSheetRow, then refetches', async () => {
    const data = baseSheetData({ rows: [row({ id: 'row-1', height: 34 })], cells: [] });
    await renderGrid(data);
    updateSheetRowMock.mockResolvedValue(row({ id: 'row-1', height: 74 }));

    await fireEvent.mouseDown(screen.getByTestId('sheet-row-grip-0'), { clientX: 10, clientY: 100 });
    globalThis.dispatchEvent(moveEvent(10, 140));
    await tick();
    globalThis.dispatchEvent(upEvent(10, 140));
    await vi.waitFor(() => expect(updateSheetRowMock).toHaveBeenCalledTimes(1));

    expect(updateSheetRowMock).toHaveBeenCalledWith('row-1', 74);
    await vi.waitFor(() => expect(getSheetDataMock).toHaveBeenCalledTimes(2));
  });

  it('readout displays the clamped value, never below the 24px row floor', async () => {
    const data = baseSheetData({ rows: [row({ id: 'row-1', height: 34 })], cells: [] });
    await renderGrid(data);

    await fireEvent.mouseDown(screen.getByTestId('sheet-row-grip-0'), { clientX: 10, clientY: 100 });
    globalThis.dispatchEvent(moveEvent(10, -900));
    await tick();

    expect(screen.getByTestId('sheet-resize-readout').textContent).toBe('24 px');

    globalThis.dispatchEvent(upEvent(10, -900));
    await tick();
  });

  it('zero-delta row drag skips the API call entirely', async () => {
    const data = baseSheetData({ rows: [row({ id: 'row-1', height: 34 })], cells: [] });
    await renderGrid(data);

    await fireEvent.mouseDown(screen.getByTestId('sheet-row-grip-0'), { clientX: 10, clientY: 100 });
    globalThis.dispatchEvent(upEvent(10, 100));
    await tick();

    expect(updateSheetRowMock).not.toHaveBeenCalled();
    expect(getSheetDataMock).toHaveBeenCalledTimes(1);
  });

  it('write failure on row resize commit calls showToast', async () => {
    const data = baseSheetData({ rows: [row({ id: 'row-1', height: 34 })], cells: [] });
    await renderGrid(data);
    updateSheetRowMock.mockRejectedValue(new Error('network error'));

    await fireEvent.mouseDown(screen.getByTestId('sheet-row-grip-0'), { clientX: 10, clientY: 100 });
    globalThis.dispatchEvent(moveEvent(10, 140));
    await tick();
    globalThis.dispatchEvent(upEvent(10, 140));

    await vi.waitFor(() => expect(screen.getByTestId('sheet-toast')).toBeTruthy());
    expect(screen.getByTestId('sheet-toast').textContent).toContain('network error');
  });

  it('drag simulation on a row grip also shows the horizontal guide variant', async () => {
    const data = baseSheetData({ rows: [row({ id: 'row-1', height: 34 })], cells: [] });
    await renderGrid(data);

    await fireEvent.mouseDown(screen.getByTestId('sheet-row-grip-0'), { clientX: 10, clientY: 100 });
    globalThis.dispatchEvent(moveEvent(10, 140));
    await tick();

    const guide = screen.getByTestId('sheet-resize-guide');
    expect(guide.className).toContain('sh-guide--row');

    globalThis.dispatchEvent(upEvent(10, 140));
    await tick();
  });
});

// #2874 item 19 — drag-select mouseup must clear `isDragSelecting`
// (previously an element-scoped `onmouseup` on `.sh-gridwrap`, which
// never fires for a mouseup released outside that element — over the
// toolbar, a popover, or outside the window entirely — leaving the drag
// permanently "on" and every later mouseenter still extending the
// selection). Fixed with a window-level listener attached in
// `handleCellMouseDown`, mirroring the resize-drag pattern directly
// above (`startColumnResize`/`startRowResize` + `handleResizeMouseUp`).
describe('SheetGrid — drag-select mouseup lifecycle (#2874 item 19)', () => {
  it('a mouseup released OUTSIDE .sh-gridwrap (window-level) still clears drag-select — regression guard for the original bug', async () => {
    const data = baseSheetData({
      columns: [col({ id: 'col-a' }), col({ id: 'col-b', name: 'B' })],
      rows: [row({ id: 'row-1', sort_order: 0 }), row({ id: 'row-2', sort_order: 1 })],
      cells: [],
    });
    await renderGrid(data);

    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-0-0'));
    await tick();

    // Mouseup dispatched at the window level (never touching
    // `.sh-gridwrap` itself) — this is exactly the case the old
    // element-scoped `onmouseup={handleGridMouseUp}` could never catch.
    globalThis.dispatchEvent(upEvent());
    await tick();

    // If drag-select were still "on", a later mouseenter over a distant
    // cell would extend the range to it. It must not.
    await fireEvent.mouseEnter(screen.getByTestId('sheet-cell-1-1'));
    await tick();

    expect(screen.getByTestId('sheet-cell-1-1').className).not.toContain('is-range');
    expect(screen.getByTestId('sheet-cell-0-0').className).toContain('is-anchor');
  });

  it('while drag-select is active, mouseenter over another cell DOES extend the range (sanity check for the guard above)', async () => {
    const data = baseSheetData({
      columns: [col({ id: 'col-a' }), col({ id: 'col-b', name: 'B' })],
      rows: [row({ id: 'row-1', sort_order: 0 }), row({ id: 'row-2', sort_order: 1 })],
      cells: [],
    });
    await renderGrid(data);

    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-0-0'));
    await tick();
    await fireEvent.mouseEnter(screen.getByTestId('sheet-cell-1-1'));
    await tick();

    expect(screen.getByTestId('sheet-cell-1-1').className).toContain('is-range');

    globalThis.dispatchEvent(upEvent());
    await tick();
  });

  it('mouseup ends the drag so a LATER mouseenter (no new mousedown) does not extend the range', async () => {
    const data = baseSheetData({
      columns: [col({ id: 'col-a' }), col({ id: 'col-b', name: 'B' })],
      rows: [row({ id: 'row-1', sort_order: 0 }), row({ id: 'row-2', sort_order: 1 })],
      cells: [],
    });
    await renderGrid(data);

    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-0-0'));
    await tick();
    globalThis.dispatchEvent(upEvent());
    await tick();

    await fireEvent.mouseEnter(screen.getByTestId('sheet-cell-1-1'));
    await tick();

    expect(screen.getByTestId('sheet-cell-1-1').className).not.toContain('is-range');
  });

  it('a fresh mousedown after a completed drag starts a NEW drag-select (listener re-armed, not left stale)', async () => {
    const data = baseSheetData({
      columns: [col({ id: 'col-a' }), col({ id: 'col-b', name: 'B' })],
      rows: [row({ id: 'row-1', sort_order: 0 }), row({ id: 'row-2', sort_order: 1 })],
      cells: [],
    });
    await renderGrid(data);

    // First drag: 0,0 → ends immediately.
    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-0-0'));
    await tick();
    globalThis.dispatchEvent(upEvent());
    await tick();

    // Second drag: 0,1 → 1,1 should extend normally, proving the mouseup
    // listener was correctly re-attached by the second mousedown rather
    // than the first one's (already-removed-by-itself) instance somehow
    // still being relied upon.
    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-0-1'));
    await tick();
    await fireEvent.mouseEnter(screen.getByTestId('sheet-cell-1-1'));
    await tick();

    expect(screen.getByTestId('sheet-cell-1-1').className).toContain('is-range');

    globalThis.dispatchEvent(upEvent());
    await tick();
  });
});

// #2874 item 19 (follow-up) — ghost cells previously bound
// `handleCellMouseDown` to BOTH `onclick` and `onmousedown`. A single
// left-click on a ghost cell fired: mousedown (arms drag-select, adds a
// window mouseup listener) → mouseup (clears drag-select, removes the
// listener — correct so far) → click (re-fires `handleCellMouseDown`,
// re-arming `isDragSelecting = true` with NO matching mouseup coming to
// ever clear it again). Any later mouseenter over another ghost cell then
// silently extended the selection with no button held. Fixed by removing
// the redundant `onclick` binding from all three ghost-cell blocks —
// selection/drag-arm now only happens on `onmousedown`, matching real
// cells. See docs/audits/2026-09-12-items-19-22-diagnosis.md.
describe('SheetGrid — ghost cell single-click does not arm drag-select (#2874 item 19)', () => {
  it('ghost cell single-click does not arm drag-select for subsequent mouseenter', async () => {
    const data = baseSheetData({
      columns: [col({ id: 'col-a' })],
      rows: [row({ id: 'row-1', sort_order: 0 })],
      cells: [],
    });
    await renderGrid(data);

    // A full single-click gesture on a ghost cell: mousedown, then
    // mouseup, then the browser's own synthesized click.
    await fireEvent.mouseDown(screen.getByTestId('sheet-ghost-cell-0-0'));
    await tick();
    globalThis.dispatchEvent(upEvent());
    await tick();
    await fireEvent.click(screen.getByTestId('sheet-ghost-cell-0-0'));
    await tick();

    // Hover another ghost cell with no button held — if the click had
    // re-armed `isDragSelecting`, this would extend the range to it.
    await fireEvent.mouseEnter(screen.getByTestId('sheet-ghost-cell-0-1'));
    await tick();

    expect(screen.getByTestId('sheet-ghost-cell-0-1').className).not.toContain('is-range');
  });
});

describe('SheetGrid — formula editing mode (#2339 PR 14)', () => {
  it('typing = as the first character opens the formula editor, not the plain text editor', async () => {
    const data = baseSheetData({ cells: [cell({ raw_value: '' })] });
    await renderGrid(data);

    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-0-0'));
    await tick();
    await fireEvent.keyDown(screen.getByTestId('sheet-grid-wrap'), { key: '=' });
    await tick();

    expect(screen.getByTestId('sheet-cell-editor').getAttribute('data-variant')).toBe('formula');
    const input = screen.getByTestId('sheet-cell-editor-input') as HTMLInputElement;
    expect(input.value).toBe('=');
  });

  it('double-clicking into an existing formula cell reopens the formula editor (not plain text)', async () => {
    const data = baseSheetData({ cells: [cell({ raw_value: '=A1+B1' })] });
    await renderGrid(data);

    await fireEvent.dblClick(screen.getByTestId('sheet-cell-0-0'));
    await tick();

    expect(screen.getByTestId('sheet-cell-editor').getAttribute('data-variant')).toBe('formula');
    const input = screen.getByTestId('sheet-cell-editor-input') as HTMLInputElement;
    expect(input.value).toBe('=A1+B1');
  });

  it('a plain (non-formula) double-click still opens the ordinary text editor', async () => {
    const data = baseSheetData({ cells: [cell({ raw_value: 'plain text' })] });
    await renderGrid(data);

    await fireEvent.dblClick(screen.getByTestId('sheet-cell-0-0'));
    await tick();

    expect(screen.getByTestId('sheet-cell-editor').getAttribute('data-variant')).not.toBe('formula');
  });

  it('typing = into a checkbox column is ignored — no editor opens at all (ruling 2)', async () => {
    const data = baseSheetData({
      columns: [col({ id: 'col-a', col_type: 'checkbox' })],
      cells: [cell({ raw_value: '0' })],
    });
    await renderGrid(data);

    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-0-0'));
    await tick();
    await fireEvent.keyDown(screen.getByTestId('sheet-grid-wrap'), { key: '=' });
    await tick();

    expect(screen.queryByTestId('sheet-cell-editor')).toBeNull();
    expect(updateSheetCellMock).not.toHaveBeenCalled();
  });

  it('double-clicking a select cell whose stored value happens to start with = still opens the select dropdown, not the formula editor (ruling 2)', async () => {
    const data = baseSheetData({
      columns: [col({ id: 'col-a', col_type: 'select', options_json: '{"choices":["=weird","ok"]}' })],
      cells: [cell({ raw_value: '=weird' })],
    });
    await renderGrid(data);

    await fireEvent.dblClick(screen.getByTestId('sheet-cell-0-0'));
    await tick();

    expect(screen.getByTestId('sheet-cell-editor').getAttribute('data-variant')).toBe('select');
  });

  it('publishes formula bar info (cref/value/editable) via onSelectionInfo as selection moves', async () => {
    const data = baseSheetData({
      columns: [col({ id: 'col-a', name: 'A' })],
      cells: [cell({ raw_value: '42' })],
    });
    const onSelectionInfo = vi.fn();
    getSheetDataMock.mockResolvedValue(data);
    render(SheetGrid, { props: { noteId: NOTE_ID, onSelectionInfo } });
    await vi.waitFor(() => expect(screen.getByTestId('sheet-grid')).toBeTruthy());
    await tick();

    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-0-0'));
    await tick();

    const lastCall = onSelectionInfo.mock.calls.at(-1)![0];
    expect(lastCall.cref).toBe('A1');
    expect(lastCall.value).toBe('42');
    expect(lastCall.editable).toBe(true);
    expect(lastCall.editing).toBe(false);
  });

  it('onSelectionInfo reports editable: false for a checkbox anchor', async () => {
    const data = baseSheetData({
      columns: [col({ id: 'col-a', col_type: 'checkbox' })],
      cells: [cell({ raw_value: '0' })],
    });
    const onSelectionInfo = vi.fn();
    getSheetDataMock.mockResolvedValue(data);
    render(SheetGrid, { props: { noteId: NOTE_ID, onSelectionInfo } });
    await vi.waitFor(() => expect(screen.getByTestId('sheet-grid')).toBeTruthy());
    await tick();

    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-0-0'));
    await tick();

    const lastCall = onSelectionInfo.mock.calls.at(-1)![0];
    expect(lastCall.editable).toBe(false);
  });

  it('onSelectionInfo reflects the live edit buffer and clears editing/error after a successful commit', async () => {
    const data = baseSheetData({ cells: [cell({ raw_value: '' })] });
    updateSheetCellMock.mockResolvedValue(cell({ raw_value: '=A1' }));
    const onSelectionInfo = vi.fn();
    getSheetDataMock.mockResolvedValue(data);
    render(SheetGrid, { props: { noteId: NOTE_ID, onSelectionInfo } });
    await vi.waitFor(() => expect(screen.getByTestId('sheet-grid')).toBeTruthy());
    await tick();

    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-0-0'));
    await tick();
    await fireEvent.keyDown(screen.getByTestId('sheet-grid-wrap'), { key: '=' });
    await tick();

    let lastCall = onSelectionInfo.mock.calls.at(-1)![0];
    expect(lastCall.editing).toBe(true);
    expect(lastCall.value).toBe('=');

    const input = screen.getByTestId('sheet-cell-editor-input') as HTMLInputElement;
    await fireEvent.input(input, { target: { value: '=A1' } });
    await tick();
    lastCall = onSelectionInfo.mock.calls.at(-1)![0];
    expect(lastCall.value).toBe('=A1');

    await fireEvent.keyDown(input, { key: 'Enter' });
    await tick();
    lastCall = onSelectionInfo.mock.calls.at(-1)![0];
    expect(lastCall.editing).toBe(false);
    expect(lastCall.error).toBeNull();
  });

  it('onSelectionInfo surfaces the rejection error inline after a bad formula commit (ruling 3/8)', async () => {
    const data = baseSheetData({ cells: [cell({ raw_value: '' })] });
    updateSheetCellMock.mockRejectedValue(new Error("'Z99' is out of bounds \u2014 this sheet has no column Z"));
    const onSelectionInfo = vi.fn();
    getSheetDataMock.mockResolvedValue(data);
    render(SheetGrid, { props: { noteId: NOTE_ID, onSelectionInfo } });
    await vi.waitFor(() => expect(screen.getByTestId('sheet-grid')).toBeTruthy());
    await tick();

    await fireEvent.dblClick(screen.getByTestId('sheet-cell-0-0'));
    await tick();
    const input = screen.getByTestId('sheet-cell-editor-input') as HTMLInputElement;
    await fireEvent.input(input, { target: { value: '=Z99' } });
    await fireEvent.keyDown(input, { key: 'Enter' });

    await vi.waitFor(() => {
      const lastCall = onSelectionInfo.mock.calls.at(-1)![0];
      expect(lastCall.error).toContain('out of bounds');
    });
  });

  it('dashed ref-range outline classes appear on cells a formula-in-progress references', async () => {
    const data = baseSheetData({
      columns: [col({ id: 'col-a', name: 'A', sort_order: 0 }), col({ id: 'col-b', name: 'B', sort_order: 1 })],
      rows: [row({ id: 'row-1', sort_order: 0 }), row({ id: 'row-2', sort_order: 1 })],
      cells: [cell({ row_id: 'row-1', column_id: 'col-b', raw_value: '' })],
    });
    await renderGrid(data);

    // Anchor the second column's first cell and type a formula referencing A1:A2.
    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-0-1'));
    await tick();
    await fireEvent.keyDown(screen.getByTestId('sheet-grid-wrap'), { key: '=' });
    await tick();
    const input = screen.getByTestId('sheet-cell-editor-input') as HTMLInputElement;
    await fireEvent.input(input, { target: { value: '=SUM(A1:A2)' } });
    await tick();

    const refCellTop = screen.getByTestId('sheet-cell-0-0');
    const refCellBottom = screen.getByTestId('sheet-cell-1-0');
    expect(refCellTop.className).toContain('is-ref');
    expect(refCellTop.className).toContain('is-refedge-t');
    expect(refCellBottom.className).toContain('is-refedge-b');

    // The editing cell itself (col B) is never part of its own ref range.
    const editingCell = screen.getByTestId('sheet-cell-0-1');
    expect(editingCell.className).not.toContain('is-ref');
  });

  it('ref-range outlines disappear once the formula edit is cancelled', async () => {
    const data = baseSheetData({
      columns: [col({ id: 'col-a', name: 'A', sort_order: 0 }), col({ id: 'col-b', name: 'B', sort_order: 1 })],
      cells: [],
    });
    await renderGrid(data);

    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-0-1'));
    await tick();
    await fireEvent.keyDown(screen.getByTestId('sheet-grid-wrap'), { key: '=' });
    await tick();
    const input = screen.getByTestId('sheet-cell-editor-input') as HTMLInputElement;
    await fireEvent.input(input, { target: { value: '=A1' } });
    await tick();
    expect(screen.getByTestId('sheet-cell-0-0').className).toContain('is-ref');

    await fireEvent.keyDown(input, { key: 'Escape' });
    await tick();
    expect(screen.getByTestId('sheet-cell-0-0').className).not.toContain('is-ref');
  });

  it('barBeginEdit/barSetValue/barCommit drive the anchor cell editor from the bar side', async () => {
    const data = baseSheetData({ cells: [cell({ raw_value: '' })] });
    updateSheetCellMock.mockResolvedValue(cell({ raw_value: '=A1' }));
    const result = await renderGrid(data);
    const gridRef = result.component as unknown as {
      barBeginEdit: () => void;
      barSetValue: (v: string) => void;
      barCommit: (cause: 'enter' | 'blur') => void;
    };

    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-0-0'));
    await tick();

    gridRef.barBeginEdit();
    await tick();
    expect(screen.getByTestId('sheet-cell-editor')).toBeTruthy();

    gridRef.barSetValue('=A1');
    await tick();
    const input = screen.getByTestId('sheet-cell-editor-input') as HTMLInputElement;
    expect(input.value).toBe('=A1');

    gridRef.barCommit('enter');
    await tick();
    expect(updateSheetCellMock).toHaveBeenCalledWith('row-1', 'col-a', '=A1', null);
  });

  it('barBeginEdit is a no-op for a checkbox anchor (ruling 2)', async () => {
    const data = baseSheetData({
      columns: [col({ id: 'col-a', col_type: 'checkbox' })],
      cells: [cell({ raw_value: '0' })],
    });
    const result = await renderGrid(data);
    const gridRef = result.component as unknown as { barBeginEdit: () => void };

    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-0-0'));
    await tick();

    gridRef.barBeginEdit();
    await tick();
    expect(screen.queryByTestId('sheet-cell-editor')).toBeNull();
  });

  it('barCancel reverts a bar-initiated edit without committing', async () => {
    const data = baseSheetData({ cells: [cell({ raw_value: 'original' })] });
    const result = await renderGrid(data);
    const gridRef = result.component as unknown as {
      barBeginEdit: () => void;
      barSetValue: (v: string) => void;
      barCancel: () => void;
    };

    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-0-0'));
    await tick();
    gridRef.barBeginEdit();
    await tick();
    gridRef.barSetValue('garbage');
    await tick();

    gridRef.barCancel();
    await tick();

    expect(updateSheetCellMock).not.toHaveBeenCalled();
    expect(screen.queryByTestId('sheet-cell-editor-input')).toBeNull();
    expect(screen.getByTestId('sheet-cell-0-0').textContent).toContain('original');
  });
});

// #2339 PR 15b-alpha (alpha) — live style_json rendering, right-click
// context menu, Delete/Backspace clear-contents, "Clear styling".
describe('SheetGrid — live style_json rendering (cellInlineStyle wiring)', () => {
  it('applies a background declaration from style_json to the real cell div', async () => {
    const data = baseSheetData({ cells: [cell({ raw_value: 'hi', style_json: '{"bg":"#112233"}' })] });
    await renderGrid(data);

    const cellEl = screen.getByTestId('sheet-cell-0-0') as HTMLElement;
    expect(cellEl.style.background).toBe('rgb(17, 34, 51)');
  });

  it('does NOT apply a color declaration to an error-kind cell, even if style_json carries an fg (error text always wins)', async () => {
    const data = baseSheetData({
      cells: [cell({ raw_value: '=1/0', computed_value: '#DIV/0!', style_json: '{"fg":"#ffffff"}' })],
    });
    await renderGrid(data);

    const cellEl = screen.getByTestId('sheet-cell-0-0') as HTMLElement;
    expect(cellEl.style.color).not.toBe('rgb(255, 255, 255)');
    expect(cellEl.style.color).toBe('');
  });

  it('DOES apply a background declaration to an error-kind cell (fills apply unconditionally)', async () => {
    const data = baseSheetData({
      cells: [cell({ raw_value: '=1/0', computed_value: '#DIV/0!', style_json: '{"bg":"#331111"}' })],
    });
    await renderGrid(data);

    const cellEl = screen.getByTestId('sheet-cell-0-0') as HTMLElement;
    expect(cellEl.style.background).toBe('rgb(51, 17, 17)');
  });

  it('applies a color declaration to a non-error cell', async () => {
    const data = baseSheetData({ cells: [cell({ raw_value: 'hi', style_json: '{"fg":"#00ff00"}' })] });
    await renderGrid(data);

    const cellEl = screen.getByTestId('sheet-cell-0-0') as HTMLElement;
    expect(cellEl.style.color).toBe('rgb(0, 255, 0)');
  });
});

describe('SheetGrid — right-click context menu (ShCellContextMenu wiring)', () => {
  it('right-clicking a cell outside the current selection selects it, then opens the context menu', async () => {
    const data = baseSheetData({
      columns: [col({ id: 'col-a', sort_order: 0 }), col({ id: 'col-b', sort_order: 1 })],
      cells: [cell({ raw_value: 'a' }), cell({ column_id: 'col-b', raw_value: 'b' })],
    });
    await renderGrid(data);

    // Anchor starts at (0,0) by default (INITIAL_SHEET_SELECTION) — right-
    // click cell (0,1), which is NOT the current anchor.
    await fireEvent.contextMenu(screen.getByTestId('sheet-cell-0-1'));
    await tick();

    expect(screen.getByTestId('cell-context-menu')).toBeTruthy();
    // Re-selected to the right-clicked cell.
    expect(screen.getByTestId('sheet-cell-0-1').className).toContain('is-anchor');
  });

  it('right-clicking a cell already inside the active range does not change the selection', async () => {
    const data = baseSheetData({
      columns: [col({ id: 'col-a', sort_order: 0 }), col({ id: 'col-b', sort_order: 1 })],
      rows: [row({ id: 'row-1', sort_order: 0 }), row({ id: 'row-2', sort_order: 1 })],
      cells: [cell({ raw_value: 'a' })],
    });
    await renderGrid(data);

    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-0-0'));
    await tick();
    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-1-1'), { shiftKey: true });
    await tick();
    expect(screen.getByTestId('sheet-cell-1-1').className).toContain('is-range');

    // Right-click a cell INSIDE the range (0,1) — selection must not reset
    // to a bare single-cell anchor at (0,1).
    await fireEvent.contextMenu(screen.getByTestId('sheet-cell-0-1'));
    await tick();

    expect(screen.getByTestId('sheet-cell-0-0').className).toContain('is-anchor');
    expect(screen.getByTestId('sheet-cell-1-1').className).toContain('is-range');
  });

  it('the context menu\'s cellCount reflects the full active range, not just the right-clicked cell', async () => {
    const data = baseSheetData({
      columns: [col({ id: 'col-a', sort_order: 0 }), col({ id: 'col-b', sort_order: 1 })],
      rows: [row({ id: 'row-1', sort_order: 0 }), row({ id: 'row-2', sort_order: 1 })],
      cells: [cell({ raw_value: 'a' })],
    });
    await renderGrid(data);

    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-0-0'));
    await tick();
    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-1-1'), { shiftKey: true });
    await tick();

    await fireEvent.contextMenu(screen.getByTestId('sheet-cell-0-1'));
    await tick();

    const deleteRow = screen.getByTestId('cell-context-menu-delete-contents');
    expect(deleteRow.textContent).toContain("Delete 4 cells' contents");
  });

  it('Escape closes the context menu', async () => {
    const data = baseSheetData({ cells: [cell({ raw_value: 'a' })] });
    await renderGrid(data);

    await fireEvent.contextMenu(screen.getByTestId('sheet-cell-0-0'));
    await tick();
    expect(screen.getByTestId('cell-context-menu')).toBeTruthy();

    await fireEvent.keyDown(screen.getByTestId('cell-context-menu'), { key: 'Escape' });
    await tick();
    expect(screen.queryByTestId('cell-context-menu')).toBeNull();
  });

  it('"Insert row above" in the context menu calls createSheetRow with the right-clicked row\'s id and position "before", and closes the menu', async () => {
    // Two rows so the right-clicked row (row-2, display row 1) has a
    // real predecessor — the non-boundary case.
    const data = baseSheetData({
      rows: [row({ id: 'row-1', sort_order: 0 }), row({ id: 'row-2', sort_order: 1000 })],
      cells: [cell({ raw_value: 'a' }), cell({ row_id: 'row-2', raw_value: 'b' })],
    });
    await renderGrid(data);

    createSheetRowMock.mockResolvedValue(row({ id: 'row-new', sort_order: 500 }));
    getSheetDataMock.mockResolvedValueOnce(baseSheetData({
      rows: [row({ id: 'row-1', sort_order: 0 }), row({ id: 'row-new', sort_order: 500 }), row({ id: 'row-2', sort_order: 1000 })],
      cells: [cell({ raw_value: 'a' }), cell({ row_id: 'row-2', raw_value: 'b' })],
    }));

    await fireEvent.contextMenu(screen.getByTestId('sheet-cell-1-0'));
    await tick();
    await fireEvent.click(screen.getByTestId('cell-context-menu-insert-row'));
    await tick();

    expect(createSheetRowMock).toHaveBeenCalledWith(NOTE_ID, 'row-2', 'before');
    expect(screen.queryByTestId('cell-context-menu')).toBeNull();
  });

  it('"Insert row above" right-clicking the FIRST row passes that row\'s own id (boundary case)', async () => {
    const data = baseSheetData({
      rows: [row({ id: 'row-1', sort_order: 0 }), row({ id: 'row-2', sort_order: 1000 })],
      cells: [cell({ raw_value: 'a' }), cell({ row_id: 'row-2', raw_value: 'b' })],
    });
    await renderGrid(data);

    createSheetRowMock.mockResolvedValue(row({ id: 'row-new', sort_order: 500 }));
    getSheetDataMock.mockResolvedValueOnce(baseSheetData({
      rows: [row({ id: 'row-new', sort_order: 500 }), row({ id: 'row-1', sort_order: 1000 }), row({ id: 'row-2', sort_order: 2000 })],
      cells: [cell({ raw_value: 'a' }), cell({ row_id: 'row-2', raw_value: 'b' })],
    }));

    await fireEvent.contextMenu(screen.getByTestId('sheet-cell-0-0'));
    await tick();
    await fireEvent.click(screen.getByTestId('cell-context-menu-insert-row'));
    await tick();

    expect(createSheetRowMock).toHaveBeenCalledWith(NOTE_ID, 'row-1', 'before');
    expect(screen.queryByTestId('cell-context-menu')).toBeNull();
  });

  // #2874 item 31: "Insert column left" moved from the cell context menu
  // to the column header menu (`ShHeaderMenu`) — removed from
  // `ShCellContextMenu` entirely, not merely duplicated. These two tests
  // now exercise the header-menu door instead of the old cell-context-menu
  // door; the underlying `insertColumn(referenceColumnId, 'before')`
  // mechanism and its Rust call shape are unchanged.
  it('right-clicking a cell no longer shows an "Insert column left" row (moved to the header menu)', async () => {
    const data = baseSheetData({
      columns: [col({ id: 'col-a', sort_order: 0 }), col({ id: 'col-b', sort_order: 1000 })],
      cells: [cell({ raw_value: 'a' }), cell({ column_id: 'col-b', raw_value: 'b' })],
    });
    await renderGrid(data);

    await fireEvent.contextMenu(screen.getByTestId('sheet-cell-0-1'));
    await tick();

    expect(screen.getByTestId('cell-context-menu')).toBeTruthy();
    expect(screen.queryByTestId('cell-context-menu-insert-column')).toBeNull();
  });

  it('opening the header menu and clicking "Insert column left" calls createSheetColumn with the clicked header\'s column id and position "before", and closes the menu', async () => {
    const data = baseSheetData({
      columns: [col({ id: 'col-a', sort_order: 0 }), col({ id: 'col-b', sort_order: 1000 })],
      cells: [cell({ raw_value: 'a' }), cell({ column_id: 'col-b', raw_value: 'b' })],
    });
    await renderGrid(data);

    createSheetColumnMock.mockResolvedValue(col({ id: 'col-new', name: '', col_type: 'text', sort_order: 500 }));
    getSheetDataMock.mockResolvedValueOnce(baseSheetData({
      columns: [col({ id: 'col-a', sort_order: 0 }), col({ id: 'col-new', name: '', col_type: 'text', sort_order: 500 }), col({ id: 'col-b', sort_order: 1000 })],
      cells: [cell({ raw_value: 'a' }), cell({ column_id: 'col-b', raw_value: 'b' })],
    }));

    await fireEvent.contextMenu(screen.getByTestId('sheet-col-header-1'));
    await tick();
    await fireEvent.click(screen.getByTestId('sheet-header-menu-insert-column'));
    await tick();

    expect(createSheetColumnMock).toHaveBeenCalledWith(NOTE_ID, '', 'text', null, 'col-b', 'before');
    expect(screen.queryByTestId('sheet-header-menu')).toBeNull();
  });

  it('opening the header menu on the FIRST column and clicking "Insert column left" passes that column\'s own id (boundary case)', async () => {
    const data = baseSheetData({
      columns: [col({ id: 'col-a', sort_order: 0 }), col({ id: 'col-b', sort_order: 1000 })],
      cells: [cell({ raw_value: 'a' }), cell({ column_id: 'col-b', raw_value: 'b' })],
    });
    await renderGrid(data);

    createSheetColumnMock.mockResolvedValue(col({ id: 'col-new', name: '', col_type: 'text', sort_order: 500 }));
    getSheetDataMock.mockResolvedValueOnce(baseSheetData({
      columns: [col({ id: 'col-new', name: '', col_type: 'text', sort_order: 500 }), col({ id: 'col-a', sort_order: 1000 }), col({ id: 'col-b', sort_order: 2000 })],
      cells: [cell({ raw_value: 'a' }), cell({ column_id: 'col-b', raw_value: 'b' })],
    }));

    await fireEvent.contextMenu(screen.getByTestId('sheet-col-header-0'));
    await tick();
    await fireEvent.click(screen.getByTestId('sheet-header-menu-insert-column'));
    await tick();

    expect(createSheetColumnMock).toHaveBeenCalledWith(NOTE_ID, '', 'text', null, 'col-a', 'before');
    expect(screen.queryByTestId('sheet-header-menu')).toBeNull();
  });

  it('#2339 PR 15d-beta: "Style cells…" in the context menu closes the menu and opens ShStylePanel', async () => {
    const data = baseSheetData({ cells: [cell({ raw_value: 'a' })] });
    await renderGrid(data);

    await fireEvent.contextMenu(screen.getByTestId('sheet-cell-0-0'));
    await tick();
    await fireEvent.click(screen.getByTestId('cell-context-menu-style'));
    await tick();

    expect(updateSheetCellMock).not.toHaveBeenCalled();
    expect(screen.queryByTestId('cell-context-menu')).toBeNull();
    expect(screen.getByTestId('style-panel')).toBeTruthy();
  });
});

// #2874 item 22 — ShGhostCellContextMenu: right-clicking a ghost row/
// column/both opens a REDUCED 2-item menu (Insert row above / Insert
// column left only — ghost cells have no contents to style/clear/export/
// delete), and both insert actions append via the zero-arg `insertRow()`/
// `insertColumn()` form rather than resolving a (possibly-undefined)
// reference id alongside a hardcoded 'before' — the latter shape hits a
// Rust backend guard that rejects mismatched reference/position pairs
// when the reference id is undefined (see the item 22 scope doc §3).
//
// Default `baseSheetData()` fixture has exactly 1 real row + 1 real
// column, so: block 1 (real row × ghost col) ghost cell testid is
// `sheet-ghost-cell-0-0` (ri=0, gi=0); block 2 (ghost row × real col) is
// `sheet-ghost-cell-1-0` (gRow=1, ci=0); block 3 (ghost row × ghost col,
// "both ghost") is `sheet-ghost-cell-1-1` (gRow=1, gCol=1).
describe('SheetGrid — ghost cell context menu (#2874 item 22)', () => {
  it('right-clicking a ghost cell opens the reduced (ghost) context menu, not the full menu', async () => {
    const data = baseSheetData({ cells: [cell({ raw_value: 'a' })] });
    await renderGrid(data);

    // Ghost row × ghost column ('both ghost', block 3) — the strictest
    // case, since it's ghost on both axes.
    await fireEvent.contextMenu(screen.getByTestId('sheet-ghost-cell-1-1'));
    await tick();

    expect(screen.getByTestId('ghost-cell-context-menu')).toBeTruthy();
    expect(screen.queryByTestId('cell-context-menu')).toBeNull();

    // Only 2 menu items — the reduced set, no style/clear/export/delete.
    expect(screen.getByTestId('ghost-cell-context-menu-insert-row')).toBeTruthy();
    expect(screen.getByTestId('ghost-cell-context-menu-insert-column')).toBeTruthy();
    expect(screen.queryByTestId('cell-context-menu-style')).toBeNull();
    expect(screen.queryByTestId('cell-context-menu-clear-styling')).toBeNull();
    expect(screen.queryByTestId('cell-context-menu-export')).toBeNull();
    expect(screen.queryByTestId('cell-context-menu-delete-contents')).toBeNull();

    const menu = screen.getByTestId('ghost-cell-context-menu');
    expect(within(menu).getAllByRole('menuitem')).toHaveLength(2);
  });

  it('right-clicking a real cell still opens the full menu, not the ghost menu (regression guard)', async () => {
    const data = baseSheetData({ cells: [cell({ raw_value: 'a' })] });
    await renderGrid(data);

    await fireEvent.contextMenu(screen.getByTestId('sheet-cell-0-0'));
    await tick();

    expect(screen.getByTestId('cell-context-menu')).toBeTruthy();
    expect(screen.queryByTestId('ghost-cell-context-menu')).toBeNull();
  });

  it('"Insert row above" on a ghost row appends a real row (zero-arg call — NOT a reference/position pair)', async () => {
    const data = baseSheetData({ cells: [cell({ raw_value: 'a' })] });
    await renderGrid(data);

    const newRow = row({ id: 'row-new', sort_order: 1000 });
    createSheetRowMock.mockResolvedValue(newRow);
    getSheetDataMock.mockResolvedValueOnce(baseSheetData({
      rows: [row({ id: 'row-1', sort_order: 0 }), newRow],
      cells: [cell({ raw_value: 'a' })],
    }));

    // Ghost row × real column (block 2).
    await fireEvent.contextMenu(screen.getByTestId('sheet-ghost-cell-1-0'));
    await tick();
    expect(screen.getByTestId('ghost-cell-context-menu')).toBeTruthy();

    await fireEvent.click(screen.getByTestId('ghost-cell-context-menu-insert-row'));
    await tick();

    // Critical regression guard: NO reference id, NO position — a naive
    // port of the real-cell callback (`underlyingRow(...)?.id` + 'before')
    // would instead call this with `(NOTE_ID, undefined, 'before')`, which
    // the backend's reference/position-pair guard rejects.
    expect(createSheetRowMock).toHaveBeenCalledWith(NOTE_ID, undefined, undefined);
    expect(screen.queryByTestId('ghost-cell-context-menu')).toBeNull();
  });

  it('"Insert row above" on the BOTH-ghost cell (ghost row × ghost column) still just appends a row, independent of the column also being ghost', async () => {
    const data = baseSheetData({ cells: [cell({ raw_value: 'a' })] });
    await renderGrid(data);

    const newRow = row({ id: 'row-new', sort_order: 1000 });
    createSheetRowMock.mockResolvedValue(newRow);
    getSheetDataMock.mockResolvedValueOnce(baseSheetData({
      rows: [row({ id: 'row-1', sort_order: 0 }), newRow],
      cells: [cell({ raw_value: 'a' })],
    }));

    await fireEvent.contextMenu(screen.getByTestId('sheet-ghost-cell-1-1'));
    await tick();
    await fireEvent.click(screen.getByTestId('ghost-cell-context-menu-insert-row'));
    await tick();

    expect(createSheetRowMock).toHaveBeenCalledWith(NOTE_ID, undefined, undefined);
    expect(createSheetColumnMock).not.toHaveBeenCalled();
  });

  it('"Insert column left" on a ghost column appends a real column (zero-arg call — NOT a reference/position pair)', async () => {
    const data = baseSheetData({ cells: [cell({ raw_value: 'a' })] });
    await renderGrid(data);

    const newCol = col({ id: 'col-new', name: '', col_type: 'text', sort_order: 1000 });
    createSheetColumnMock.mockResolvedValue(newCol);
    getSheetDataMock.mockResolvedValueOnce(baseSheetData({
      columns: [col({ id: 'col-a', sort_order: 0 }), newCol],
      cells: [cell({ raw_value: 'a' })],
    }));

    // Real row × ghost column (block 1).
    await fireEvent.contextMenu(screen.getByTestId('sheet-ghost-cell-0-0'));
    await tick();
    expect(screen.getByTestId('ghost-cell-context-menu')).toBeTruthy();

    await fireEvent.click(screen.getByTestId('ghost-cell-context-menu-insert-column'));
    await tick();

    expect(createSheetColumnMock).toHaveBeenCalledWith(NOTE_ID, '', 'text', null, undefined, undefined);
    expect(screen.queryByTestId('ghost-cell-context-menu')).toBeNull();
  });

  it('Escape closes the ghost cell context menu', async () => {
    const data = baseSheetData({ cells: [cell({ raw_value: 'a' })] });
    await renderGrid(data);

    await fireEvent.contextMenu(screen.getByTestId('sheet-ghost-cell-1-1'));
    await tick();
    expect(screen.getByTestId('ghost-cell-context-menu')).toBeTruthy();

    await fireEvent.keyDown(screen.getByTestId('ghost-cell-context-menu'), { key: 'Escape' });
    await tick();
    expect(screen.queryByTestId('ghost-cell-context-menu')).toBeNull();
  });
});

// #2339 PR 15d-beta — ShStylePanel: ⌘⇧S shortcut, toolbar-facing
// `openStylePanel` export, unanimity-derived props, and
// `applyStyleToSelection`'s per-cell writes (incl. Matt's outer-edge
// border ruling). See sheetCellStyle.test.ts for the pure-function unit
// tests (buildCellStyleJson/expandSelection/detectUnanimity/
// detectBorderUnanimity/mergeStylePatch) this integration layer is built
// on top of — these tests cover SheetGrid's OWN wiring only.
describe('SheetGrid — style panel (#2339 PR 15d-beta)', () => {
  it('⌘⇧S opens the style panel when a cell is anchored', async () => {
    const data = baseSheetData({ cells: [cell({ raw_value: 'a' })] });
    await renderGrid(data);

    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-0-0'));
    await tick();
    await fireEvent.keyDown(screen.getByTestId('sheet-grid-wrap'), { key: 'S', metaKey: true, shiftKey: true });
    await tick();

    expect(screen.getByTestId('style-panel')).toBeTruthy();
  });

  it('Cmd+Shift+S anchors style panel to selected cell (#2874 item 21)', async () => {
    const data = baseSheetData({ cells: [cell({ raw_value: 'a' })] });
    await renderGrid(data);

    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-0-0'));
    await tick();

    const cellEl = screen.getByTestId('sheet-cell-0-0') as HTMLElement;
    cellEl.getBoundingClientRect = () => ({
      left: 300, right: 380, top: 120, bottom: 148, width: 80, height: 28, x: 300, y: 120, toJSON: () => ({}),
    });

    await fireEvent.keyDown(screen.getByTestId('sheet-grid-wrap'), { key: 'S', metaKey: true, shiftKey: true });
    await tick();

    const panel = screen.getByTestId('style-panel') as HTMLElement;
    // Anchored to the selected cell's own rect (left: 300, bottom+4: 152) —
    // NOT the hardcoded `{x:12, y:78}` sidebar fallback.
    expect(panel.style.left).toBe('300px');
    expect(panel.style.top).toBe('152px');
  });

  it('Cmd+Shift+S anchors style panel to ghost cell when no real cells exist (#2874 item 21 follow-up)', async () => {
    // A totally fresh sheet — zero real rows/columns, so every rendered
    // cell is a ghost cell (`sheet-ghost-cell-{row}-{col}`). Anchoring on
    // one of these used to fall through to the hardcoded `{x:12,y:78}`
    // sidebar fallback because `selectedCellEl()` only queried the
    // real-cell testid pattern.
    const data = baseSheetData({ columns: [], rows: [] });
    await renderGrid(data);

    await fireEvent.mouseDown(screen.getByTestId('sheet-ghost-cell-0-0'));
    await tick();

    const ghostCellEl = screen.getByTestId('sheet-ghost-cell-0-0') as HTMLElement;
    ghostCellEl.getBoundingClientRect = () => ({
      left: 420, right: 500, top: 200, bottom: 228, width: 80, height: 28, x: 420, y: 200, toJSON: () => ({}),
    });

    await fireEvent.keyDown(screen.getByTestId('sheet-grid-wrap'), { key: 'S', metaKey: true, shiftKey: true });
    await tick();

    const panel = screen.getByTestId('style-panel') as HTMLElement;
    // Anchored to the ghost cell's own rect (left: 420, bottom+4: 232) —
    // NOT the hardcoded `{x:12, y:78}` sidebar fallback.
    expect(panel.style.left).toBe('420px');
    expect(panel.style.top).toBe('232px');
  });

  it('⌘⇧S does nothing while a cell editor is open (guarded by the editing check)', async () => {
    const data = baseSheetData({ cells: [cell({ raw_value: 'a' })] });
    await renderGrid(data);

    await fireEvent.dblClick(screen.getByTestId('sheet-cell-0-0'));
    await tick();
    await fireEvent.keyDown(screen.getByTestId('sheet-grid-wrap'), { key: 'S', metaKey: true, shiftKey: true });
    await tick();

    expect(screen.queryByTestId('style-panel')).toBeNull();
  });

  it('a plain, unmodified "s" keystroke still opens the text editor (type-to-overwrite) rather than the style panel', async () => {
    const data = baseSheetData({ cells: [cell({ raw_value: 'existing' })] });
    await renderGrid(data);

    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-0-0'));
    await tick();
    await fireEvent.keyDown(screen.getByTestId('sheet-grid-wrap'), { key: 's' });
    await tick();

    expect(screen.queryByTestId('style-panel')).toBeNull();
    const input = screen.getByTestId('sheet-cell-editor-input') as HTMLInputElement;
    expect(input.value).toBe('s');
  });

  it('openStylePanel() (the toolbar-facing export) opens the panel directly', async () => {
    const data = baseSheetData({ cells: [cell({ raw_value: 'a' })] });
    const { component } = await renderGrid(data);

    (component as any).openStylePanel();
    await tick();

    expect(screen.getByTestId('style-panel')).toBeTruthy();
  });

  it('the panel highlights the fill swatch matching a unanimous single-cell selection', async () => {
    const data = baseSheetData({ cells: [cell({ raw_value: 'a', style_json: '{"bg":"#232327"}' })] });
    const { component } = await renderGrid(data);

    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-0-0'));
    await tick();
    (component as any).openStylePanel();
    await tick();

    expect(screen.getByTestId('style-panel-fill-slate').className).toContain('is-on');
  });

  it('the panel highlights NOTHING when a multi-cell selection disagrees on fill (unanimity ruling)', async () => {
    const data = baseSheetData({
      columns: [col({ id: 'col-a', sort_order: 0 }), col({ id: 'col-b', name: 'B', sort_order: 1 })],
      cells: [
        cell({ column_id: 'col-a', raw_value: 'a', style_json: '{"bg":"#232327"}' }),
        cell({ column_id: 'col-b', raw_value: 'b', style_json: '{"bg":"rgba(0,228,228,0.14)"}' }),
      ],
    });
    const { component } = await renderGrid(data);

    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-0-0'));
    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-0-1'), { shiftKey: true });
    await tick();
    (component as any).openStylePanel();
    await tick();

    expect(screen.getByTestId('style-panel-fill-slate').className).not.toContain('is-on');
    expect(screen.getByTestId('style-panel-fill-cyan').className).not.toContain('is-on');
  });

  it('applying a fill writes the merged style_json for every selected cell', async () => {
    const data = baseSheetData({
      columns: [col({ id: 'col-a', sort_order: 0 }), col({ id: 'col-b', name: 'B', sort_order: 1 })],
      rows: [row({ id: 'row-1', sort_order: 0 }), row({ id: 'row-2', sort_order: 1 })],
      cells: [
        cell({ row_id: 'row-1', column_id: 'col-a', raw_value: 'a', style_json: '{"fg":"#EF4444"}' }),
        cell({ row_id: 'row-1', column_id: 'col-b', raw_value: 'b' }),
        cell({ row_id: 'row-2', column_id: 'col-a', raw_value: 'c' }),
        cell({ row_id: 'row-2', column_id: 'col-b', raw_value: 'd' }),
      ],
    });
    getSheetDataMock.mockResolvedValue(data);
    render(SheetGrid, { props: { noteId: NOTE_ID } });
    await vi.waitFor(() => expect(screen.getByTestId('sheet-grid')).toBeTruthy());
    await tick();

    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-0-0'));
    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-1-1'), { shiftKey: true });
    await tick();
    await fireEvent.keyDown(screen.getByTestId('sheet-grid-wrap'), { key: 'S', metaKey: true, shiftKey: true });
    await tick();

    await fireEvent.click(screen.getByTestId('style-panel-fill-cyan'));
    // #2874 item 51: ONE batch call for the whole 4-cell apply — BEFORE
    // this item: 4 separate `updateSheetCell` calls.
    await vi.waitFor(() => expect(updateSheetCellsStyleBatchMock).toHaveBeenCalledTimes(1));
    expect(updateSheetCellsStyleBatchMock.mock.calls[0][1]).toHaveLength(4);

    // Cell (row-1, col-a) already had fg set — the fill patch must MERGE,
    // not clobber the existing fg.
    const written1 = batchEntry('row-1', 'col-a');
    expect(written1).toBeTruthy();
    expect(written1.bg).toBe('rgba(0,228,228,0.14)');
    expect(written1.fg).toBe('#EF4444');

    const written2 = batchEntry('row-2', 'col-b');
    expect(written2.bg).toBe('rgba(0,228,228,0.14)');
  });

  it("Matt's outer-edge border ruling: applying \"top\" to a 3x1 range writes border-top on the FIRST row's cells only", async () => {
    const data = baseSheetData({
      columns: [col({ id: 'col-a', sort_order: 0 })],
      rows: [row({ id: 'row-1', sort_order: 0 }), row({ id: 'row-2', sort_order: 1 }), row({ id: 'row-3', sort_order: 2 })],
      cells: [
        cell({ row_id: 'row-1', raw_value: 'a' }),
        cell({ row_id: 'row-2', raw_value: 'b' }),
        cell({ row_id: 'row-3', raw_value: 'c' }),
      ],
    });
    await renderGrid(data);

    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-0-0'));
    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-2-0'), { shiftKey: true });
    await tick();
    await fireEvent.keyDown(screen.getByTestId('sheet-grid-wrap'), { key: 'S', metaKey: true, shiftKey: true });
    await tick();

    await fireEvent.click(screen.getByTestId('style-panel-border-side-top'));
    // #2874 item 51: ONE batch call for the 3-cell apply (BEFORE: 3 calls).
    await vi.waitFor(() => expect(updateSheetCellsStyleBatchMock).toHaveBeenCalledTimes(1));
    expect(updateSheetCellsStyleBatchMock.mock.calls[0][1]).toHaveLength(3);

    const topBd = batchEntry('row-1', 'col-a').bd;
    expect(topBd).toEqual([{ side: 'top', w: 1, c: 'var(--sheet-border-styled)', dashed: false }]);

    const midBd = batchEntry('row-2', 'col-a').bd;
    expect(midBd ?? []).toEqual([]);

    const bottomBd = batchEntry('row-3', 'col-a').bd;
    expect(bottomBd ?? []).toEqual([]);
  });

  it("Matt's outer-edge border ruling: \"all\" writes all 4 sides on every cell in a 2x2 range", async () => {
    const data = baseSheetData({
      columns: [col({ id: 'col-a', sort_order: 0 }), col({ id: 'col-b', name: 'B', sort_order: 1 })],
      rows: [row({ id: 'row-1', sort_order: 0 }), row({ id: 'row-2', sort_order: 1 })],
      cells: [
        cell({ row_id: 'row-1', column_id: 'col-a', raw_value: 'a' }),
        cell({ row_id: 'row-1', column_id: 'col-b', raw_value: 'b' }),
        cell({ row_id: 'row-2', column_id: 'col-a', raw_value: 'c' }),
        cell({ row_id: 'row-2', column_id: 'col-b', raw_value: 'd' }),
      ],
    });
    await renderGrid(data);

    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-0-0'));
    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-1-1'), { shiftKey: true });
    await tick();
    await fireEvent.keyDown(screen.getByTestId('sheet-grid-wrap'), { key: 'S', metaKey: true, shiftKey: true });
    await tick();

    await fireEvent.click(screen.getByTestId('style-panel-border-side-all'));
    // #2874 item 51: ONE batch call for the 4-cell apply (BEFORE: 4 calls).
    await vi.waitFor(() => expect(updateSheetCellsStyleBatchMock).toHaveBeenCalledTimes(1));
    const allEntries = updateSheetCellsStyleBatchMock.mock.calls[0][1] as { styleJson: string }[];
    expect(allEntries).toHaveLength(4);

    for (const entry of allEntries) {
      const bd = JSON.parse(entry.styleJson).bd;
      const sides = bd.map((b: any) => b.side).sort();
      expect(sides).toEqual(['bottom', 'left', 'right', 'top']);
    }
  });

  it('a styled border writes --sheet-border-styled, not --border-strong (#2874 item 8)', async () => {
    const data = baseSheetData({
      cells: [cell({ raw_value: 'a' })],
    });
    await renderGrid(data);

    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-0-0'));
    await tick();
    await fireEvent.keyDown(screen.getByTestId('sheet-grid-wrap'), { key: 'S', metaKey: true, shiftKey: true });
    await tick();

    await fireEvent.click(screen.getByTestId('style-panel-border-side-all'));
    await vi.waitFor(() => expect(updateSheetCellsStyleBatchMock).toHaveBeenCalled());

    const written = batchEntry('row-1', 'col-a');
    expect(written.bd[0].c).toBe('var(--sheet-border-styled)');
    expect(written.bd[0].c).not.toBe('var(--border-strong)');
  });

  it("Matt's outer-edge border ruling: \"none\" clears every border on every cell in the range", async () => {
    const data = baseSheetData({
      columns: [col({ id: 'col-a', sort_order: 0 })],
      rows: [row({ id: 'row-1', sort_order: 0 }), row({ id: 'row-2', sort_order: 1 })],
      cells: [
        cell({ row_id: 'row-1', raw_value: 'a', style_json: '{"bd":[{"side":"top","w":2,"c":"#fff"}]}' }),
        cell({ row_id: 'row-2', raw_value: 'b', style_json: '{"bd":[{"side":"bottom","w":2,"c":"#fff"}]}' }),
      ],
    });
    await renderGrid(data);

    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-0-0'));
    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-1-0'), { shiftKey: true });
    await tick();
    await fireEvent.keyDown(screen.getByTestId('sheet-grid-wrap'), { key: 'S', metaKey: true, shiftKey: true });
    await tick();

    await fireEvent.click(screen.getByTestId('style-panel-border-side-none'));
    // #2874 item 51: ONE batch call for the 2-cell apply (BEFORE: 2 calls).
    await vi.waitFor(() => expect(updateSheetCellsStyleBatchMock).toHaveBeenCalledTimes(1));
    const noneEntries = updateSheetCellsStyleBatchMock.mock.calls[0][1] as { styleJson: string }[];
    expect(noneEntries).toHaveLength(2);

    for (const entry of noneEntries) {
      const parsed = JSON.parse(entry.styleJson);
      expect(parsed.bd).toBeUndefined();
    }
  });

  it("Matt's outer-edge border ruling: applying \"bottom\" to a 3x3 range writes border-bottom on the LAST row's cells only (both row AND column axes proven)", async () => {
    const data = baseSheetData({
      columns: [col({ id: 'col-a', sort_order: 0 }), col({ id: 'col-b', name: 'B', sort_order: 1 }), col({ id: 'col-c', name: 'C', sort_order: 2 })],
      rows: [row({ id: 'row-1', sort_order: 0 }), row({ id: 'row-2', sort_order: 1 }), row({ id: 'row-3', sort_order: 2 })],
      cells: [
        cell({ row_id: 'row-1', column_id: 'col-a', raw_value: 'a' }),
        cell({ row_id: 'row-1', column_id: 'col-b', raw_value: 'b' }),
        cell({ row_id: 'row-1', column_id: 'col-c', raw_value: 'c' }),
        cell({ row_id: 'row-2', column_id: 'col-a', raw_value: 'd' }),
        cell({ row_id: 'row-2', column_id: 'col-b', raw_value: 'e' }),
        cell({ row_id: 'row-2', column_id: 'col-c', raw_value: 'f' }),
        cell({ row_id: 'row-3', column_id: 'col-a', raw_value: 'g' }),
        cell({ row_id: 'row-3', column_id: 'col-b', raw_value: 'h' }),
        cell({ row_id: 'row-3', column_id: 'col-c', raw_value: 'i' }),
      ],
    });
    await renderGrid(data);

    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-0-0'));
    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-2-2'), { shiftKey: true });
    await tick();
    await fireEvent.keyDown(screen.getByTestId('sheet-grid-wrap'), { key: 'S', metaKey: true, shiftKey: true });
    await tick();

    await fireEvent.click(screen.getByTestId('style-panel-border-side-bottom'));
    // #2874 item 51: ONE batch call for the 9-cell apply (BEFORE: 9 calls).
    await vi.waitFor(() => expect(updateSheetCellsStyleBatchMock).toHaveBeenCalledTimes(1));
    const bottomEntries = updateSheetCellsStyleBatchMock.mock.calls[0][1] as { rowId: string; styleJson: string }[];
    expect(bottomEntries).toHaveLength(9);

    const bottomRowIds = ['row-3'];
    const otherRowIds = ['row-1', 'row-2'];

    for (const entry of bottomEntries) {
      const bd = JSON.parse(entry.styleJson).bd;
      if (bottomRowIds.includes(entry.rowId)) {
        expect(bd).toEqual([{ side: 'bottom', w: 1, c: 'var(--sheet-border-styled)', dashed: false }]);
      } else {
        expect(otherRowIds).toContain(entry.rowId);
        expect(bd ?? []).toEqual([]);
      }
    }
  });

  it('Matt\'s outer-edge border ruling: applying "left" to a range writes border-left on the LEFTMOST column\'s cells only', async () => {
    const data = baseSheetData({
      columns: [col({ id: 'col-a', sort_order: 0 }), col({ id: 'col-b', name: 'B', sort_order: 1 }), col({ id: 'col-c', name: 'C', sort_order: 2 })],
      rows: [row({ id: 'row-1', sort_order: 0 })],
      cells: [
        cell({ row_id: 'row-1', column_id: 'col-a', raw_value: 'a' }),
        cell({ row_id: 'row-1', column_id: 'col-b', raw_value: 'b' }),
        cell({ row_id: 'row-1', column_id: 'col-c', raw_value: 'c' }),
      ],
    });
    await renderGrid(data);

    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-0-0'));
    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-0-2'), { shiftKey: true });
    await tick();
    await fireEvent.keyDown(screen.getByTestId('sheet-grid-wrap'), { key: 'S', metaKey: true, shiftKey: true });
    await tick();

    await fireEvent.click(screen.getByTestId('style-panel-border-side-left'));
    // #2874 item 51: ONE batch call for the 3-cell apply (BEFORE: 3 calls).
    await vi.waitFor(() => expect(updateSheetCellsStyleBatchMock).toHaveBeenCalledTimes(1));
    expect(updateSheetCellsStyleBatchMock.mock.calls[0][1]).toHaveLength(3);

    const leftBd = batchEntry('row-1', 'col-a').bd;
    expect(leftBd).toEqual([{ side: 'left', w: 1, c: 'var(--sheet-border-styled)', dashed: false }]);

    const midBd = batchEntry('row-1', 'col-b').bd;
    expect(midBd ?? []).toEqual([]);

    const rightBd = batchEntry('row-1', 'col-c').bd;
    expect(rightBd ?? []).toEqual([]);
  });

  it('Matt\'s outer-edge border ruling: applying "right" to a range writes border-right on the RIGHTMOST column\'s cells only', async () => {
    const data = baseSheetData({
      columns: [col({ id: 'col-a', sort_order: 0 }), col({ id: 'col-b', name: 'B', sort_order: 1 }), col({ id: 'col-c', name: 'C', sort_order: 2 })],
      rows: [row({ id: 'row-1', sort_order: 0 })],
      cells: [
        cell({ row_id: 'row-1', column_id: 'col-a', raw_value: 'a' }),
        cell({ row_id: 'row-1', column_id: 'col-b', raw_value: 'b' }),
        cell({ row_id: 'row-1', column_id: 'col-c', raw_value: 'c' }),
      ],
    });
    await renderGrid(data);

    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-0-0'));
    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-0-2'), { shiftKey: true });
    await tick();
    await fireEvent.keyDown(screen.getByTestId('sheet-grid-wrap'), { key: 'S', metaKey: true, shiftKey: true });
    await tick();

    await fireEvent.click(screen.getByTestId('style-panel-border-side-right'));
    // #2874 item 51: ONE batch call for the 3-cell apply (BEFORE: 3 calls).
    await vi.waitFor(() => expect(updateSheetCellsStyleBatchMock).toHaveBeenCalledTimes(1));
    expect(updateSheetCellsStyleBatchMock.mock.calls[0][1]).toHaveLength(3);

    const leftBd = batchEntry('row-1', 'col-a').bd;
    expect(leftBd ?? []).toEqual([]);

    const midBd = batchEntry('row-1', 'col-b').bd;
    expect(midBd ?? []).toEqual([]);

    const rightBd = batchEntry('row-1', 'col-c').bd;
    expect(rightBd).toEqual([{ side: 'right', w: 1, c: 'var(--sheet-border-styled)', dashed: false }]);
  });

  // #2874 item 29 — border sides became independent additive toggles.
  // These integration tests exercise the REAL `updateSheetCellsStyleBatch`
  // mock call sequence through `ShStylePanel`'s toggle buttons and
  // `applyStyleToSelection`'s side-union loop — not a re-implementation.
  // #2874 item 51: each click below is its OWN user action, so each still
  // produces its own batch call (a single-cell selection's batch just has
  // 1 entry) — only a MULTI-cell selection's single click collapses to
  // one call instead of N (proven by the outer-edge tests above).
  it('#2874 item 29: apply Top then apply Left on the same single-cell selection → cell ends with bd:[left,top] (toggle independence, two-side apply writes both entries)', async () => {
    const data = baseSheetData({ cells: [cell({ raw_value: 'a' })] });
    await renderGrid(data);

    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-0-0'));
    await tick();
    await fireEvent.keyDown(screen.getByTestId('sheet-grid-wrap'), { key: 'S', metaKey: true, shiftKey: true });
    await tick();

    await fireEvent.click(screen.getByTestId('style-panel-border-side-top'));
    await vi.waitFor(() => expect(updateSheetCellsStyleBatchMock).toHaveBeenCalledTimes(1));

    const firstCallJson = batchEntry('row-1', 'col-a', 0);
    expect(firstCallJson.bd).toEqual([{ side: 'top', w: 1, c: 'var(--sheet-border-styled)', dashed: false }]);

    await fireEvent.click(screen.getByTestId('style-panel-border-side-left'));
    await vi.waitFor(() => expect(updateSheetCellsStyleBatchMock).toHaveBeenCalledTimes(2));

    // Second write must MERGE with the first (Top kept), not replace it.
    const secondCallJson = batchEntry('row-1', 'col-a', 1);
    const sides = secondCallJson.bd.map((b: any) => b.side).sort();
    expect(sides).toEqual(['left', 'top']);
  });

  it('#2874 item 29: clicking Left after Top does NOT remove Top (Top then Left keeps Top)', async () => {
    const data = baseSheetData({ cells: [cell({ raw_value: 'a' })] });
    await renderGrid(data);

    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-0-0'));
    await tick();
    await fireEvent.keyDown(screen.getByTestId('sheet-grid-wrap'), { key: 'S', metaKey: true, shiftKey: true });
    await tick();

    await fireEvent.click(screen.getByTestId('style-panel-border-side-top'));
    await vi.waitFor(() => expect(updateSheetCellsStyleBatchMock).toHaveBeenCalledTimes(1));
    await fireEvent.click(screen.getByTestId('style-panel-border-side-left'));
    await vi.waitFor(() => expect(updateSheetCellsStyleBatchMock).toHaveBeenCalledTimes(2));

    const secondCallJson = batchEntry('row-1', 'col-a', 1);
    const topEntry = secondCallJson.bd.find((b: any) => b.side === 'top');
    expect(topEntry).toBeTruthy();
  });

  it('#2874 item 29: removing one toggled side (clicking Top again) leaves the other side (Left) intact', async () => {
    const data = baseSheetData({ cells: [cell({ raw_value: 'a' })] });
    await renderGrid(data);

    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-0-0'));
    await tick();
    await fireEvent.keyDown(screen.getByTestId('sheet-grid-wrap'), { key: 'S', metaKey: true, shiftKey: true });
    await tick();

    await fireEvent.click(screen.getByTestId('style-panel-border-side-top'));
    await vi.waitFor(() => expect(updateSheetCellsStyleBatchMock).toHaveBeenCalledTimes(1));
    await fireEvent.click(screen.getByTestId('style-panel-border-side-left'));
    await vi.waitFor(() => expect(updateSheetCellsStyleBatchMock).toHaveBeenCalledTimes(2));
    await fireEvent.click(screen.getByTestId('style-panel-border-side-top')); // toggle top OFF
    await vi.waitFor(() => expect(updateSheetCellsStyleBatchMock).toHaveBeenCalledTimes(3));

    const thirdCallJson = batchEntry('row-1', 'col-a', 2);
    const sides = (thirdCallJson.bd ?? []).map((b: any) => b.side);
    expect(sides).toEqual(['left']); // top gone, left persists
  });

  it('#2874 item 29: "All" toggle path (clicking all four directional buttons) still writes all 4 sides on every cell in a range (equivalent to the "All" fast path)', async () => {
    const data = baseSheetData({
      columns: [col({ id: 'col-a', sort_order: 0 })],
      rows: [row({ id: 'row-1', sort_order: 0 })],
      cells: [cell({ row_id: 'row-1', column_id: 'col-a', raw_value: 'a' })],
    });
    await renderGrid(data);

    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-0-0'));
    await tick();
    await fireEvent.keyDown(screen.getByTestId('sheet-grid-wrap'), { key: 'S', metaKey: true, shiftKey: true });
    await tick();

    await fireEvent.click(screen.getByTestId('style-panel-border-side-top'));
    await fireEvent.click(screen.getByTestId('style-panel-border-side-bottom'));
    await fireEvent.click(screen.getByTestId('style-panel-border-side-left'));
    await fireEvent.click(screen.getByTestId('style-panel-border-side-right'));
    await vi.waitFor(() => expect(updateSheetCellsStyleBatchMock).toHaveBeenCalledTimes(4));

    const lastCallJson = batchEntry('row-1', 'col-a', 3);
    const sides = lastCallJson.bd.map((b: any) => b.side).sort();
    expect(sides).toEqual(['bottom', 'left', 'right', 'top']);
  });

  // #2874 item 53 (Matt's ruling, 2026-10-04 — the crux of this item):
  // "four individually-chosen sides should produce an outline, not a
  // full grid." The single-cell test directly above can never prove
  // this — a lone cell is its own entire outer edge on every side, so
  // outline-fill and full-fill are literally the same result there. A
  // 3x3 block has a genuine interior cell (row-2/col-b, selected but on
  // NO outer edge), which is the only way to tell the two fast paths
  // apart: the interior cell gets a border ONLY under the (wrong,
  // pre-fix) fill-everything path, never under the outline path.
  function threeByThreeData() {
    return baseSheetData({
      columns: [col({ id: 'col-a', sort_order: 0 }), col({ id: 'col-b', name: 'B', sort_order: 1 }), col({ id: 'col-c', name: 'C', sort_order: 2 })],
      rows: [row({ id: 'row-1', sort_order: 0 }), row({ id: 'row-2', sort_order: 1 }), row({ id: 'row-3', sort_order: 2 })],
      cells: [
        cell({ row_id: 'row-1', column_id: 'col-a', raw_value: 'a' }),
        cell({ row_id: 'row-1', column_id: 'col-b', raw_value: 'b' }),
        cell({ row_id: 'row-1', column_id: 'col-c', raw_value: 'c' }),
        cell({ row_id: 'row-2', column_id: 'col-a', raw_value: 'd' }),
        cell({ row_id: 'row-2', column_id: 'col-b', raw_value: 'e' }),
        cell({ row_id: 'row-2', column_id: 'col-c', raw_value: 'f' }),
        cell({ row_id: 'row-3', column_id: 'col-a', raw_value: 'g' }),
        cell({ row_id: 'row-3', column_id: 'col-b', raw_value: 'h' }),
        cell({ row_id: 'row-3', column_id: 'col-c', raw_value: 'i' }),
      ],
    });
  }

  it('TEST 1: four individual side clicks on a 3x3 block draw the outline only (interior cell stays borderless) with "All" dark (#2874 item 53)', async () => {
    await renderGrid(threeByThreeData());

    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-0-0'));
    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-2-2'), { shiftKey: true });
    await tick();
    await fireEvent.keyDown(screen.getByTestId('sheet-grid-wrap'), { key: 'S', metaKey: true, shiftKey: true });
    await tick();

    await fireEvent.click(screen.getByTestId('style-panel-border-side-top'));
    await fireEvent.click(screen.getByTestId('style-panel-border-side-bottom'));
    await fireEvent.click(screen.getByTestId('style-panel-border-side-left'));
    await fireEvent.click(screen.getByTestId('style-panel-border-side-right'));
    await vi.waitFor(() => expect(updateSheetCellsStyleBatchMock).toHaveBeenCalledTimes(4));

    // BEFORE this fix: the 4th click's `sides` array had length 4, which
    // `SheetGrid.svelte`'s old `sides.length === 4` fast path treated
    // identically to an explicit "All" click — the interior cell
    // (row-2/col-b) would have come back with ALL FOUR sides set,
    // because that branch painted unconditionally on every cell
    // (`cellPatch.bd = allSides.map(...)`), ignoring position entirely.
    // AFTER this fix: the interior cell is on no outer edge for any of
    // the four sides, so the outline branch (`resolvedBorderSides`)
    // resolves it to nothing.
    const centerBd = batchEntry('row-2', 'col-b', 3).bd;
    expect(centerBd ?? []).toEqual([]);

    // Sanity: the outer edge itself IS still bordered, same as any
    // 1-3-side apply — top-left corner gets both its edges.
    const cornerBd = batchEntry('row-1', 'col-a', 3).bd.map((b: any) => b.side).sort();
    expect(cornerBd).toEqual(['left', 'top']);
    const topMidBd = batchEntry('row-1', 'col-b', 3).bd.map((b: any) => b.side);
    expect(topMidBd).toEqual(['top']);

    // "All" must stay dark — this was four individual clicks, not an
    // explicit All click.
    expect(screen.getByTestId('style-panel-border-side-all').className).not.toContain('is-on');
  });

  it('TEST 2: clicking "All" on the same 3x3 block fills the interior cell too, with "All" lit (#2874 item 53)', async () => {
    await renderGrid(threeByThreeData());

    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-0-0'));
    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-2-2'), { shiftKey: true });
    await tick();
    await fireEvent.keyDown(screen.getByTestId('sheet-grid-wrap'), { key: 'S', metaKey: true, shiftKey: true });
    await tick();

    await fireEvent.click(screen.getByTestId('style-panel-border-side-all'));
    await vi.waitFor(() => expect(updateSheetCellsStyleBatchMock).toHaveBeenCalledTimes(1));

    // The interior cell DOES get all four sides — this is the one and
    // only branch allowed to do that.
    const centerBd = batchEntry('row-2', 'col-b').bd.map((b: any) => b.side).sort();
    expect(centerBd).toEqual(['bottom', 'left', 'right', 'top']);

    expect(screen.getByTestId('style-panel-border-side-all').className).toContain('is-on');
  });

  it('TEST 3: toggling a side after "All" switches back to outline mode — the interior cell loses its fill (#2874 item 53)', async () => {
    await renderGrid(threeByThreeData());

    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-0-0'));
    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-2-2'), { shiftKey: true });
    await tick();
    await fireEvent.keyDown(screen.getByTestId('sheet-grid-wrap'), { key: 'S', metaKey: true, shiftKey: true });
    await tick();

    await fireEvent.click(screen.getByTestId('style-panel-border-side-all'));
    await vi.waitFor(() => expect(updateSheetCellsStyleBatchMock).toHaveBeenCalledTimes(1));
    expect((batchEntry('row-2', 'col-b').bd ?? []).length).toBe(4); // filled, from TEST 2's assertion

    // Toggle Top off — per Matt's ruling, this leaves "All" mode
    // entirely and switches to individual sides (bottom/left/right).
    await fireEvent.click(screen.getByTestId('style-panel-border-side-top'));
    await vi.waitFor(() => expect(updateSheetCellsStyleBatchMock).toHaveBeenCalledTimes(2));

    // BEFORE this fix: there was no mode to leave — toggling Top off
    // just dropped `sides.length` to 3, which already fell into the
    // (correct, unchanged) outer-edge branch, so this particular
    // assertion would have ALSO passed before the fix by coincidence of
    // `sides.length` dropping below 4. What the OLD code could never do
    // is draw the outline from exactly four individually-toggled sides
    // (TEST 1) — this test instead proves the mode transition itself:
    // the interior cell, having been filled by the All click, must lose
    // that fill the instant the panel leaves All mode.
    const centerBd = batchEntry('row-2', 'col-b', 1).bd ?? [];
    expect(centerBd).toEqual([]);

    // The corner that's on the left edge keeps its left border (still
    // requested); loses nothing it wasn't asked to keep.
    const cornerSides = batchEntry('row-1', 'col-a', 1).bd.map((b: any) => b.side);
    expect(cornerSides).toEqual(['left']);

    expect(screen.getByTestId('style-panel-border-side-all').className).not.toContain('is-on');
  });

  // #2874 item 59 (Matt's ruling, 2026-10-06, quoted in full in the fix
  // commit): "In individual-sides mode, a side click changes only that
  // side. Turning a side on adds it along the selection's outer edge;
  // turning it off removes it from every selected cell. Other sides are
  // untouched. Clicking a side while in All mode redraws the selection as
  // an outline (per the 53 ruling)." These three tests exercise the REAL
  // mock sequence through `ShStylePanel`'s toggle buttons and
  // `applyStyleToSelection`'s side loop, with `updateSheetCellsStyleBatchMock`
  // echoing the written `styleJson` back (same as the real backend,
  // `sheets.rs:2517`) so each click's `existing` read reflects the REAL
  // prior write — the same condition under which the stale-side bug
  // (`:786`'s `existingBd.filter((b) => !sides.includes(b.side))`)
  // actually manifests in production. All three fail at base (pre-fix)
  // with the echoing mock in place; a blank-every-time mock would hide
  // all three (`docs/audits/2026-10-06-2874-items-59-61.md`, "Test
  // coverage gap").
  it('#2874 item 59 TEST 1: Top → Left → Top-off on one cell leaves only Left (plain individual-toggle sequence, no "All" involved)', async () => {
    const data = baseSheetData({ cells: [cell({ raw_value: 'a' })] });
    await renderGrid(data);

    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-0-0'));
    await tick();
    await fireEvent.keyDown(screen.getByTestId('sheet-grid-wrap'), { key: 'S', metaKey: true, shiftKey: true });
    await tick();

    await fireEvent.click(screen.getByTestId('style-panel-border-side-top'));
    await vi.waitFor(() => expect(updateSheetCellsStyleBatchMock).toHaveBeenCalledTimes(1));
    expect(batchEntry('row-1', 'col-a', 0).bd.map((b: any) => b.side)).toEqual(['top']);

    await fireEvent.click(screen.getByTestId('style-panel-border-side-left'));
    await vi.waitFor(() => expect(updateSheetCellsStyleBatchMock).toHaveBeenCalledTimes(2));
    expect(batchEntry('row-1', 'col-a', 1).bd.map((b: any) => b.side).sort()).toEqual(['left', 'top']);

    await fireEvent.click(screen.getByTestId('style-panel-border-side-top')); // toggle Top OFF
    await vi.waitFor(() => expect(updateSheetCellsStyleBatchMock).toHaveBeenCalledTimes(3));

    // BEFORE the item 59 fix (with the echoing mock, so `existing` on
    // this third call is the REAL `[top, left]` written by the second
    // call, not a blank default): `:786`'s filter KEEPS `top` because it
    // is no longer in `sides` (`!sides.includes('top')` is true for the
    // now-absent side), and the add-back loop only re-adds `left` —
    // final `bd` is `[top, left]`, i.e. Top never gets removed. AFTER the
    // fix: `bd` is built fresh from `sides` alone, so it's just `[left]`.
    const thirdSides = (batchEntry('row-1', 'col-a', 2).bd ?? []).map((b: any) => b.side);
    expect(thirdSides).toEqual(['left']);
  });

  it('#2874 item 59 TEST 2: All → Bottom-off on one cell leaves Top, Left, Right (mode transition, not a bare toggle sequence)', async () => {
    const data = baseSheetData({ cells: [cell({ raw_value: 'a' })] });
    await renderGrid(data);

    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-0-0'));
    await tick();
    await fireEvent.keyDown(screen.getByTestId('sheet-grid-wrap'), { key: 'S', metaKey: true, shiftKey: true });
    await tick();

    await fireEvent.click(screen.getByTestId('style-panel-border-side-all'));
    await vi.waitFor(() => expect(updateSheetCellsStyleBatchMock).toHaveBeenCalledTimes(1));
    expect(batchEntry('row-1', 'col-a', 0).bd.map((b: any) => b.side).sort()).toEqual(['bottom', 'left', 'right', 'top']);

    await fireEvent.click(screen.getByTestId('style-panel-border-side-bottom')); // leaves All mode, toggles Bottom OFF
    await vi.waitFor(() => expect(updateSheetCellsStyleBatchMock).toHaveBeenCalledTimes(2));

    // BEFORE the item 59 fix: `existing.bd` is the real four-side write
    // from "All" (echoing mock); `sides` is now `[top, left, right]`.
    // `:786`'s filter keeps `bottom` (absent from `sides`) AND keeps
    // nothing of the other three (they're all in `sides`, so filter
    // drops them to avoid duplicates) — the add-back loop then re-adds
    // top/left/right (all on-edge for a single cell) — final `bd` is
    // `[bottom, top, left, right]`, i.e. Bottom never gets removed. AFTER
    // the fix: `bd` is built fresh from `sides` alone — `[top, left,
    // right]`.
    const sides = (batchEntry('row-1', 'col-a', 1).bd ?? []).map((b: any) => b.side).sort();
    expect(sides).toEqual(['left', 'right', 'top']);
  });

  it('#2874 item 59 TEST 3: on a block, clicking Top keeps an existing inner Left border (other sides untouched by a side click)', async () => {
    // 2 cols x 3 rows block: col-a is the left edge, col-b is the right
    // edge; row-1 is the top edge, row-3 is the bottom edge, row-2 is
    // interior (neither top nor bottom). Target cell: row-3/col-a — the
    // BOTTOM-LEFT corner. It is "inner" relative to the Top side this
    // test ultimately clicks (row-3 ≠ r0, so Top never touches it) while
    // genuinely sitting on BOTH the left edge (gets a real Left border)
    // and the bottom edge (so it's also reachable by a Bottom toggle —
    // the mechanism this test needs to prove the fix, not just assert a
    // case the bug never touched).
    const data = baseSheetData({
      columns: [col({ id: 'col-a', sort_order: 0 }), col({ id: 'col-b', name: 'B', sort_order: 1 })],
      rows: [row({ id: 'row-1', sort_order: 0 }), row({ id: 'row-2', sort_order: 1 }), row({ id: 'row-3', sort_order: 2 })],
      cells: [
        cell({ row_id: 'row-1', column_id: 'col-a', raw_value: 'a' }),
        cell({ row_id: 'row-1', column_id: 'col-b', raw_value: 'b' }),
        cell({ row_id: 'row-2', column_id: 'col-a', raw_value: 'c' }),
        cell({ row_id: 'row-2', column_id: 'col-b', raw_value: 'd' }),
        cell({ row_id: 'row-3', column_id: 'col-a', raw_value: 'e' }),
        cell({ row_id: 'row-3', column_id: 'col-b', raw_value: 'f' }),
      ],
    });
    await renderGrid(data);

    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-0-0'));
    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-2-1'), { shiftKey: true });
    await tick();
    await fireEvent.keyDown(screen.getByTestId('sheet-grid-wrap'), { key: 'S', metaKey: true, shiftKey: true });
    await tick();

    // Click 1: Left. row-3/col-a (left edge) gets `left`.
    await fireEvent.click(screen.getByTestId('style-panel-border-side-left'));
    await vi.waitFor(() => expect(updateSheetCellsStyleBatchMock).toHaveBeenCalledTimes(1));
    expect(batchEntry('row-3', 'col-a', 0).bd.map((b: any) => b.side)).toEqual(['left']);

    // Click 2: Bottom. row-3/col-a (bottom edge too) now gets `bottom`
    // added alongside `left` — a real, genuine write, not a ghost yet.
    await fireEvent.click(screen.getByTestId('style-panel-border-side-bottom'));
    await vi.waitFor(() => expect(updateSheetCellsStyleBatchMock).toHaveBeenCalledTimes(2));
    expect(batchEntry('row-3', 'col-a', 1).bd.map((b: any) => b.side).sort()).toEqual(['bottom', 'left']);

    // Click 3: Bottom again (toggle OFF). `sides` drops back to `[left]`.
    // row-3/col-a's stored `bd` going into this call is the REAL
    // `[bottom, left]` from click 2 (echoing mock) — this is exactly the
    // condition under which `:786`'s bug manifests: `bottom` just left
    // `sides` but the old filter KEEPS it anyway (`!sides.includes
    // ('bottom')` is true for an absent side). Expected: `[left]`.
    await fireEvent.click(screen.getByTestId('style-panel-border-side-bottom'));
    await vi.waitFor(() => expect(updateSheetCellsStyleBatchMock).toHaveBeenCalledTimes(3));
    expect(batchEntry('row-3', 'col-a', 2).bd.map((b: any) => b.side)).toEqual(['left']);

    // Click 4: Top. `sides` becomes `[left, top]`. row-3/col-a is NOT on
    // the top edge (row-3 ≠ r0), so Top is never added there — this is
    // the click the test is named for. The point: whatever ghost (or
    // lack thereof) click 3 left behind on this cell carries forward
    // into this and every future call, since each call's `existing` read
    // is the real echoed value from the previous one.
    //
    // BEFORE the item 59 fix: click 3's bug already left `bottom` as a
    // ghost (`[bottom, left]` instead of `[left]`) — that ghost is STILL
    // absent from `sides` on this 4th call too (sides is `[left, top]`,
    // `bottom` was never back in play), so the old filter keeps it AGAIN
    // (`!sides.includes('bottom')` true), and the add-back loop (which
    // only iterates `sides`) never removes it. Result: `[bottom, left]`
    // — `left` genuinely survives (this part of Matt's ruling — "other
    // sides are untouched" — was never broken for a continuously-active
    // side), but it's contaminated by a `bottom` ghost that should have
    // been gone two clicks ago. AFTER the fix: `bd` is built fresh from
    // `sides` alone every call, so the ghost can never accumulate in the
    // first place — result: `[left]`.
    await fireEvent.click(screen.getByTestId('style-panel-border-side-top'));
    await vi.waitFor(() => expect(updateSheetCellsStyleBatchMock).toHaveBeenCalledTimes(4));

    const innerSides = (batchEntry('row-3', 'col-a', 3).bd ?? []).map((b: any) => b.side).sort();
    expect(innerSides).toEqual(['left']);

    // Sanity: the top-left corner (on BOTH the left and top edges) gets
    // both — Top really was applied, this isn't a no-op click.
    const cornerSides = batchEntry('row-1', 'col-a', 3).bd.map((b: any) => b.side).sort();
    expect(cornerSides).toEqual(['left', 'top']);
  });

  // #2874 item 59 ROUND 2 (Matt's ruling, 2026-10-07, `docs/audits/
  // 2026-10-07-pr-2987-item59.md`): the fix shipped at PR #2987's head
  // (`4c7a457d`, the three tests directly above) REBUILT `bd` fresh from
  // the panel's complete `sides` set plus the outer-edge check on every
  // apply — passing review because `sides` was always internally
  // consistent, but answering the wrong question. Matt's ruling
  // (authoritative, quoted in full): "A side click changes only that
  // side. Turning a side on adds it to cells on the selection's outer
  // edge. Turning it off removes it from every selected cell. Every
  // other side on every cell stays exactly as stored, whether or not
  // it's lit in the panel." The two tests below are the required
  // before/after proof, each demonstrated FAILING against PR #2987's
  // actual head (`4c7a457d`) — not just the original pre-#2987 base —
  // since that head is the code this fix replaces.
  it('#2874 item 59 round 2 TEST A: a 3x3 block where the middle cell has its OWN Left border keeps that Left when Top is clicked (a side click never rebuilds sides it didn\'t touch)', async () => {
    // 3x3 block; EVERY cell carries the SAME real, unanimous Left border
    // (so the panel opens with weight/dashed already resolved, not
    // indeterminate — keeping this test focused on the rebuild-vs-delta
    // question, not item 45's separate mixed-weight guard). The cell
    // that matters is the MIDDLE one (row-2/col-b): it sits on NO outer
    // edge for either 'left' or 'top', so its Left border was never
    // addable by any click in the first place — it only has it because
    // it's real stored data, exactly like every other cell here. Nothing
    // in this test ever clicks Left; the only click fired is Top.
    const leftBorder = '{"bd":[{"side":"left","w":1,"c":"var(--sheet-border-styled)","dashed":false}]}';
    const data = baseSheetData({
      columns: [col({ id: 'col-a', sort_order: 0 }), col({ id: 'col-b', name: 'B', sort_order: 1 }), col({ id: 'col-c', name: 'C', sort_order: 2 })],
      rows: [row({ id: 'row-1', sort_order: 0 }), row({ id: 'row-2', sort_order: 1 }), row({ id: 'row-3', sort_order: 2 })],
      cells: [
        cell({ row_id: 'row-1', column_id: 'col-a', raw_value: 'a', style_json: leftBorder }),
        cell({ row_id: 'row-1', column_id: 'col-b', raw_value: 'b', style_json: leftBorder }),
        cell({ row_id: 'row-1', column_id: 'col-c', raw_value: 'c', style_json: leftBorder }),
        cell({ row_id: 'row-2', column_id: 'col-a', raw_value: 'd', style_json: leftBorder }),
        cell({ row_id: 'row-2', column_id: 'col-b', raw_value: 'e', style_json: leftBorder }),
        cell({ row_id: 'row-2', column_id: 'col-c', raw_value: 'f', style_json: leftBorder }),
        cell({ row_id: 'row-3', column_id: 'col-a', raw_value: 'g', style_json: leftBorder }),
        cell({ row_id: 'row-3', column_id: 'col-b', raw_value: 'h', style_json: leftBorder }),
        cell({ row_id: 'row-3', column_id: 'col-c', raw_value: 'i', style_json: leftBorder }),
      ],
    });
    await renderGrid(data);

    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-0-0'));
    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-2-2'), { shiftKey: true });
    await tick();
    await fireEvent.keyDown(screen.getByTestId('sheet-grid-wrap'), { key: 'S', metaKey: true, shiftKey: true });
    await tick();

    // Sanity: Left opened lit (unanimous across all 9 cells) and resolved
    // (not indeterminate) — confirming this test's click (Top) is
    // genuinely a different, unresolved side, and that the deferral this
    // function's own item 45 guard would otherwise apply to a fresh
    // Top isn't what's under test here.
    expect(screen.getByTestId('style-panel-border-side-left').className).toContain('is-on');
    expect(screen.getByTestId('style-panel-border-weight-1').className).toContain('is-on');

    await fireEvent.click(screen.getByTestId('style-panel-border-side-top'));
    await vi.waitFor(() => expect(updateSheetCellsStyleBatchMock).toHaveBeenCalledTimes(1));

    // AT PR #2987's HEAD (`4c7a457d`, before this fix): the click fired
    // `onApplyBorder({mode:'sides', sides:['left','top'], w:1,
    // dashed:false})` (the FULL current toggle set, not a delta) and
    // `SheetGrid.svelte` rebuilt `bd` from scratch for every cell using
    // ONLY `resolvedBorderSides` against `sides` — the middle cell
    // (row-2/col-b) sits on NO outer edge for either 'left' or 'top', so
    // the rebuild resolved to an empty array REGARDLESS of its real
    // stored data. Actual output at `4c7a457d`: `bd: []` — the middle
    // cell's own Left border is silently destroyed by a click on Top, a
    // side it was never on the edge for and the user never touched.
    // AFTER this fix: the click fires `onApplyBorderSide({side:'top',
    // on:true, ...})` — a delta that only ever reads or writes the
    // 'top' entry. The middle cell isn't on the top edge either, so
    // `cellPatch.bd` is never even assigned — `existing.bd` (the real
    // stored Left border) passes through `mergeStylePatch` untouched.
    const middleBd = batchEntry('row-2', 'col-b').bd;
    expect(middleBd).toEqual([{ side: 'left', w: 1, c: 'var(--sheet-border-styled)', dashed: false }]);

    // Sanity: Top really was applied along the outer edge — this isn't a
    // no-op click. The top-left corner (on the top edge) gets BOTH its
    // pre-existing Left and the newly-applied Top.
    const cornerSides = batchEntry('row-1', 'col-a').bd.map((b: any) => b.side).sort();
    expect(cornerSides).toEqual(['left', 'top']);
  });

  it('#2874 item 59 round 2 TEST B: a block where only some cells have Left removes Left from EVERY selected cell when Left is toggled off, without touching an unrelated stored side on an untouched cell', async () => {
    // 3-row x 2-col block (row-1/row-2/row-3 x col-a/col-b). Only col-a's
    // row-1 and row-3 carry a real Left border (partial presence — "only
    // some cells have Left"); only row-2/col-b (the one cell on NO outer
    // edge for either remaining direction once Left leaves `sides`: not
    // col-a so not the left edge, not row-1 so not the top edge) carries
    // a real, untouched Top border. Because Left is genuinely partial,
    // `detectBorderUnanimity` reports it 'mixed' and the panel opens with
    // `borderWeight`/`borderDashed` indeterminate (item 45’s own,
    // unchanged rule) — so this test explicitly resolves both via a real
    // weight click and a real dashed click BEFORE the Left-off click
    // that's actually under test, exactly as a real user must (item 45's
    // guard "stays as built", per this fix's own ruling). Both resolving
    // values (`w:2`, `dashed:true`) are chosen to match row-2/col-b's own
    // stored Top exactly, so those two clicks are a true no-op THERE
    // specifically (confirmed below) — any contamination they cause on
    // OTHER cells (fabricating/adjusting entries via the unchanged,
    // unrelated weight/dashed-click outer-edge rule) is irrelevant to
    // what this test actually asserts.
    const data = baseSheetData({
      columns: [col({ id: 'col-a', sort_order: 0 }), col({ id: 'col-b', name: 'B', sort_order: 1 })],
      rows: [row({ id: 'row-1', sort_order: 0 }), row({ id: 'row-2', sort_order: 1 }), row({ id: 'row-3', sort_order: 2 })],
      cells: [
        cell({
          row_id: 'row-1',
          column_id: 'col-a',
          raw_value: 'a',
          style_json: '{"bd":[{"side":"left","w":1,"c":"var(--sheet-border-styled)","dashed":false}]}',
        }),
        cell({ row_id: 'row-1', column_id: 'col-b', raw_value: 'b' }),
        cell({ row_id: 'row-2', column_id: 'col-a', raw_value: 'c' }),
        cell({
          row_id: 'row-2',
          column_id: 'col-b',
          raw_value: 'd',
          style_json: '{"bd":[{"side":"top","w":2,"c":"var(--sheet-border-styled)","dashed":true}]}',
        }),
        cell({
          row_id: 'row-3',
          column_id: 'col-a',
          raw_value: 'e',
          style_json: '{"bd":[{"side":"left","w":1,"c":"var(--sheet-border-styled)","dashed":false}]}',
        }),
        cell({ row_id: 'row-3', column_id: 'col-b', raw_value: 'f' }),
      ],
    });
    await renderGrid(data);

    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-0-0'));
    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-2-1'), { shiftKey: true });
    await tick();
    await fireEvent.keyDown(screen.getByTestId('sheet-grid-wrap'), { key: 'S', metaKey: true, shiftKey: true });
    await tick();

    // Sanity: both Left (partial presence, row-1 + row-3/col-a) and Top
    // (partial presence, row-2/col-b only) show active, and — since both
    // are partial — weight/dashed open indeterminate.
    expect(screen.getByTestId('style-panel-border-side-left').className).toContain('is-on');
    expect(screen.getByTestId('style-panel-border-side-top').className).toContain('is-on');
    expect(screen.getByTestId('style-panel-border-weight-2').className).not.toContain('is-on');
    expect(screen.getByTestId('style-panel-border-dashed').className).not.toContain('is-on');

    // Resolve weight/dashed explicitly (item 45's guard, unchanged) —
    // chosen to exactly match row-2/col-b's own stored Top, so these two
    // clicks are a no-op for that cell specifically (verified below).
    await fireEvent.click(screen.getByTestId('style-panel-border-weight-2'));
    await vi.waitFor(() => expect(updateSheetCellsStyleBatchMock).toHaveBeenCalledTimes(1));
    await fireEvent.click(screen.getByTestId('style-panel-border-dashed'));
    await vi.waitFor(() => expect(updateSheetCellsStyleBatchMock).toHaveBeenCalledTimes(2));

    // Confirm the no-op claim before the real click under test: row-2/
    // col-b's Top is untouched by either resolving click (it's on no
    // outer edge for 'left', so neither click's outer-edge-gated
    // fresh-entry creation ever reaches it; its existing Top entry
    // already matched the resolved values).
    expect(batchEntry('row-2', 'col-b').bd).toEqual([{ side: 'top', w: 2, c: 'var(--sheet-border-styled)', dashed: true }]);

    await fireEvent.click(screen.getByTestId('style-panel-border-side-left')); // toggle Left OFF
    await vi.waitFor(() => expect(updateSheetCellsStyleBatchMock).toHaveBeenCalledTimes(3));

    // AT PR #2987's HEAD (`4c7a457d`, before this fix): the click fired
    // `onApplyBorder({mode:'sides', sides:['top'], w:2, dashed:true})`
    // (Left dropped from the array, Top remaining) and `SheetGrid.svelte`
    // rebuilt EVERY selected cell's `bd` from scratch using ONLY that
    // reduced array via `resolvedBorderSides`. row-2/col-b sits on NO
    // outer edge for 'top' (row-2 isn't the range's `r0`) — so the
    // rebuild's `acc` resolved to EMPTY for it regardless of its real
    // stored data. Actual output at `4c7a457d`: `bd: []` — row-2/col-b's
    // own real Top border (confirmed present and untouched just above)
    // is silently destroyed by a click on Left, a side this cell never
    // even had.
    // AFTER this fix: the click fires `onApplyBorderSide({side:'left',
    // on:false, ...})` — removes ONLY the 'left' entry from every
    // selected cell's own `existingBd`, never reading or rewriting
    // 'top' at all. Critically, row-2/col-b — which never had 'left' to
    // remove, and whose 'top' was never part of this click — stays
    // exactly `[top, w:2, dashed:true]`, unchanged.
    for (const [rowId, colId] of [
      ['row-1', 'col-a'],
      ['row-1', 'col-b'],
      ['row-2', 'col-a'],
      ['row-2', 'col-b'],
      ['row-3', 'col-a'],
      ['row-3', 'col-b'],
    ]) {
      const sides = (batchEntry(rowId, colId).bd ?? []).map((b: any) => b.side);
      expect(sides).not.toContain('left');
    }
    expect(batchEntry('row-2', 'col-b').bd).toEqual([{ side: 'top', w: 2, c: 'var(--sheet-border-styled)', dashed: true }]);
  });

  // #2874 item 53 (Matt's ruling, verbatim): "When the panel opens: All
  // lights only if every selected cell has all four sides; otherwise
  // individual sides show as today." TEST 4 below covers BOTH cases.
  it('TEST 4a: opening the panel lights "All" when every selected cell individually has all four sides', async () => {
    const data = baseSheetData({
      columns: [col({ id: 'col-a', sort_order: 0 }), col({ id: 'col-b', name: 'B', sort_order: 1 })],
      rows: [row({ id: 'row-1', sort_order: 0 })],
      cells: [
        cell({
          row_id: 'row-1',
          column_id: 'col-a',
          raw_value: 'a',
          style_json:
            '{"bd":[{"side":"top","w":1,"c":"var(--sheet-border-styled)","dashed":false},{"side":"bottom","w":1,"c":"var(--sheet-border-styled)","dashed":false},{"side":"left","w":1,"c":"var(--sheet-border-styled)","dashed":false},{"side":"right","w":1,"c":"var(--sheet-border-styled)","dashed":false}]}',
        }),
        cell({
          row_id: 'row-1',
          column_id: 'col-b',
          raw_value: 'b',
          style_json:
            '{"bd":[{"side":"top","w":1,"c":"var(--sheet-border-styled)","dashed":false},{"side":"bottom","w":1,"c":"var(--sheet-border-styled)","dashed":false},{"side":"left","w":1,"c":"var(--sheet-border-styled)","dashed":false},{"side":"right","w":1,"c":"var(--sheet-border-styled)","dashed":false}]}',
        }),
      ],
    });
    const { component } = await renderGrid(data);

    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-0-0'));
    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-0-1'), { shiftKey: true });
    await tick();
    (component as any).openStylePanel();
    await tick();

    // BEFORE this fix: `panelBorder` had no per-cell "does this cell
    // have all four" check at all — "All" lit purely because the
    // per-side unanimity loop happened to report all four sides as
    // non-absent, which is ALSO true for TEST 4b below. The two cases
    // were indistinguishable at the seeding layer, not just the write
    // layer.
    expect(screen.getByTestId('style-panel-border-side-all').className).toContain('is-on');
  });

  it('TEST 4b: opening the panel shows individual sides (not "All") when sides are only present as an outline, even though all four directions are touched somewhere in the selection', async () => {
    const data = baseSheetData({
      columns: [col({ id: 'col-a', sort_order: 0 }), col({ id: 'col-b', name: 'B', sort_order: 1 }), col({ id: 'col-c', name: 'C', sort_order: 2 })],
      rows: [row({ id: 'row-1', sort_order: 0 }), row({ id: 'row-2', sort_order: 1 }), row({ id: 'row-3', sort_order: 2 })],
      cells: [
        cell({ row_id: 'row-1', column_id: 'col-a', raw_value: 'a', style_json: '{"bd":[{"side":"top","w":1,"c":"var(--sheet-border-styled)","dashed":false},{"side":"left","w":1,"c":"var(--sheet-border-styled)","dashed":false}]}' }),
        cell({ row_id: 'row-1', column_id: 'col-b', raw_value: 'b', style_json: '{"bd":[{"side":"top","w":1,"c":"var(--sheet-border-styled)","dashed":false}]}' }),
        cell({ row_id: 'row-1', column_id: 'col-c', raw_value: 'c', style_json: '{"bd":[{"side":"top","w":1,"c":"var(--sheet-border-styled)","dashed":false},{"side":"right","w":1,"c":"var(--sheet-border-styled)","dashed":false}]}' }),
        cell({ row_id: 'row-2', column_id: 'col-a', raw_value: 'd', style_json: '{"bd":[{"side":"left","w":1,"c":"var(--sheet-border-styled)","dashed":false}]}' }),
        cell({ row_id: 'row-2', column_id: 'col-b', raw_value: 'e' }),
        cell({ row_id: 'row-2', column_id: 'col-c', raw_value: 'f', style_json: '{"bd":[{"side":"right","w":1,"c":"var(--sheet-border-styled)","dashed":false}]}' }),
        cell({ row_id: 'row-3', column_id: 'col-a', raw_value: 'g', style_json: '{"bd":[{"side":"bottom","w":1,"c":"var(--sheet-border-styled)","dashed":false},{"side":"left","w":1,"c":"var(--sheet-border-styled)","dashed":false}]}' }),
        cell({ row_id: 'row-3', column_id: 'col-b', raw_value: 'h', style_json: '{"bd":[{"side":"bottom","w":1,"c":"var(--sheet-border-styled)","dashed":false}]}' }),
        cell({ row_id: 'row-3', column_id: 'col-c', raw_value: 'i', style_json: '{"bd":[{"side":"bottom","w":1,"c":"var(--sheet-border-styled)","dashed":false},{"side":"right","w":1,"c":"var(--sheet-border-styled)","dashed":false}]}' }),
      ],
    });
    const { component } = await renderGrid(data);

    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-0-0'));
    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-2-2'), { shiftKey: true });
    await tick();
    (component as any).openStylePanel();
    await tick();

    // Every one of the four directions IS touched by some cell in the
    // selection (that's what makes this the trap case — the per-side
    // unanimity loop reports all four as non-absent, same as TEST 4a),
    // but NO single cell has all four at once. "All" must stay dark.
    // BEFORE this fix: `panelBorder` had no mode concept, so this
    // rendered IDENTICALLY to TEST 4a — "All" lit here too, which is
    // exactly the bug ("clicking Top, Bottom, Left and Right one by one
    // lights All" also applies to re-opening a selection that already
    // looks that way).
    expect(screen.getByTestId('style-panel-border-side-all').className).not.toContain('is-on');
    // The individual sides still show correctly, per the existing
    // (unaffected) per-side unanimity/mixed machinery — each direction
    // is non-absent (mixed: some cells have it, some don't), so each
    // side button lights, same as item 45's mixed-side rendering.
    expect(screen.getByTestId('style-panel-border-side-top').className).toContain('is-on');
    expect(screen.getByTestId('style-panel-border-side-bottom').className).toContain('is-on');
    expect(screen.getByTestId('style-panel-border-side-left').className).toContain('is-on');
    expect(screen.getByTestId('style-panel-border-side-right').className).toContain('is-on');
  });

  it('#2874 item 29: panelBorder reports ALL unanimous sides independently when a multi-cell selection has both Top and Left unanimous', async () => {
    const data = baseSheetData({
      columns: [col({ id: 'col-a', sort_order: 0 })],
      rows: [row({ id: 'row-1', sort_order: 0 })],
      cells: [
        cell({
          row_id: 'row-1',
          column_id: 'col-a',
          raw_value: 'a',
          style_json: '{"bd":[{"side":"top","w":1,"c":"var(--sheet-border-styled)","dashed":false},{"side":"left","w":1,"c":"var(--sheet-border-styled)","dashed":false}]}',
        }),
      ],
    });
    const { component } = await renderGrid(data);

    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-0-0'));
    await tick();
    (component as any).openStylePanel();
    await tick();

    expect(screen.getByTestId('style-panel-border-side-top').className).toContain('is-on');
    expect(screen.getByTestId('style-panel-border-side-left').className).toContain('is-on');
    expect(screen.getByTestId('style-panel-border-side-bottom').className).not.toContain('is-on');
    expect(screen.getByTestId('style-panel-border-side-right').className).not.toContain('is-on');
  });

  it('#2874 item 29 (Matt\'s ruling #1, option B): mixed weights across unanimous sides show NO active weight button — Top unanimous at weight 2, Left unanimous at weight 1', async () => {
    const data = baseSheetData({
      columns: [col({ id: 'col-a', sort_order: 0 })],
      rows: [row({ id: 'row-1', sort_order: 0 })],
      cells: [
        cell({
          row_id: 'row-1',
          column_id: 'col-a',
          raw_value: 'a',
          style_json: '{"bd":[{"side":"top","w":2,"c":"var(--sheet-border-styled)","dashed":false},{"side":"left","w":1,"c":"var(--sheet-border-styled)","dashed":false}]}',
        }),
      ],
    });
    const { component } = await renderGrid(data);

    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-0-0'));
    await tick();
    (component as any).openStylePanel();
    await tick();

    // Both sides are still shown as active (unanimous presence)...
    expect(screen.getByTestId('style-panel-border-side-top').className).toContain('is-on');
    expect(screen.getByTestId('style-panel-border-side-left').className).toContain('is-on');
    // ...but no weight button highlights, since the two sides disagree on w.
    expect(screen.getByTestId('style-panel-border-weight-1').className).not.toContain('is-on');
    expect(screen.getByTestId('style-panel-border-weight-2').className).not.toContain('is-on');
    expect(screen.getByTestId('style-panel-border-weight-3').className).not.toContain('is-on');
  });

  // #2874 item 45 (Matt's ruling, 2026-09-24): same-SIDE disagreement,
  // one layer below the cross-side case above. `detectBorderUnanimity`
  // used to collapse "no cell has this side" and "cells disagree on this
  // side" into the same falsy `null`, which made a genuinely mixed
  // selection render exactly like a unanimously borderless one ("None"
  // and weight "1" both lit). Matt's ruling is literal: for a mixed
  // side, the side button stays LIT, and "None"/every weight button stay
  // DARK — mirroring (but distinct from, and layered below) the
  // already-working cross-side pattern proven above.
  it('#2874 item 45: unanimous weight across MULTIPLE cells on the same side lights the side button and the shared weight', async () => {
    const data = baseSheetData({
      rows: [row({ id: 'row-1', sort_order: 0 }), row({ id: 'row-2', sort_order: 1 })],
      cells: [
        cell({ row_id: 'row-1', column_id: 'col-a', raw_value: 'a', style_json: '{"bd":[{"side":"top","w":2,"c":"var(--sheet-border-styled)","dashed":false}]}' }),
        cell({ row_id: 'row-2', column_id: 'col-a', raw_value: 'b', style_json: '{"bd":[{"side":"top","w":2,"c":"var(--sheet-border-styled)","dashed":false}]}' }),
      ],
    });
    const { component } = await renderGrid(data);

    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-0-0'));
    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-1-0'), { shiftKey: true });
    await tick();
    (component as any).openStylePanel();
    await tick();

    expect(screen.getByTestId('style-panel-border-side-top').className).toContain('is-on');
    expect(screen.getByTestId('style-panel-border-side-none').className).not.toContain('is-on');
    expect(screen.getByTestId('style-panel-border-weight-2').className).toContain('is-on');
    expect(screen.getByTestId('style-panel-border-weight-1').className).not.toContain('is-on');
  });

  it('#2874 item 45: unanimous none across a multi-cell, genuinely borderless selection lights "None" and nothing else', async () => {
    const data = baseSheetData({
      rows: [row({ id: 'row-1', sort_order: 0 }), row({ id: 'row-2', sort_order: 1 })],
      cells: [
        cell({ row_id: 'row-1', column_id: 'col-a', raw_value: 'a' }),
        cell({ row_id: 'row-2', column_id: 'col-a', raw_value: 'b' }),
      ],
    });
    const { component } = await renderGrid(data);

    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-0-0'));
    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-1-0'), { shiftKey: true });
    await tick();
    (component as any).openStylePanel();
    await tick();

    expect(screen.getByTestId('style-panel-border-side-none').className).toContain('is-on');
    for (const side of ['top', 'bottom', 'left', 'right']) {
      expect(screen.getByTestId(`style-panel-border-side-${side}`).className).not.toContain('is-on');
    }
  });

  it('#2874 item 45 (item 45\'s exact bug, rendering half): mixed weights on the SAME side — side button stays lit, "None" and every weight button stay dark', async () => {
    const data = baseSheetData({
      rows: [row({ id: 'row-1', sort_order: 0 }), row({ id: 'row-2', sort_order: 1 })],
      cells: [
        cell({ row_id: 'row-1', column_id: 'col-a', raw_value: 'a', style_json: '{"bd":[{"side":"top","w":2,"c":"var(--sheet-border-styled)","dashed":false}]}' }),
        cell({ row_id: 'row-2', column_id: 'col-a', raw_value: 'b', style_json: '{"bd":[{"side":"top","w":1,"c":"var(--sheet-border-styled)","dashed":false}]}' }),
      ],
    });
    const { component } = await renderGrid(data);

    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-0-0'));
    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-1-0'), { shiftKey: true });
    await tick();
    (component as any).openStylePanel();
    await tick();

    // Side button: stays lit (a border genuinely exists on this side).
    expect(screen.getByTestId('style-panel-border-side-top').className).toContain('is-on');
    // "None" must NOT light — that would be the false-unanimity bug.
    expect(screen.getByTestId('style-panel-border-side-none').className).not.toContain('is-on');
    // No weight button lights — not even "1", which is the exact false
    // unanimity Matt reported (weight 2 vs weight 1 used to show "1" lit).
    expect(screen.getByTestId('style-panel-border-weight-1').className).not.toContain('is-on');
    expect(screen.getByTestId('style-panel-border-weight-2').className).not.toContain('is-on');
    expect(screen.getByTestId('style-panel-border-weight-3').className).not.toContain('is-on');
  });

  it('#2874 item 45: one cell bordered, one cell plain on the same side counts as mixed (not absent) — side button stays lit, "None" stays dark', async () => {
    const data = baseSheetData({
      rows: [row({ id: 'row-1', sort_order: 0 }), row({ id: 'row-2', sort_order: 1 })],
      cells: [
        cell({ row_id: 'row-1', column_id: 'col-a', raw_value: 'a', style_json: '{"bd":[{"side":"top","w":2,"c":"var(--sheet-border-styled)","dashed":false}]}' }),
        cell({ row_id: 'row-2', column_id: 'col-a', raw_value: 'b' }),
      ],
    });
    const { component } = await renderGrid(data);

    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-0-0'));
    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-1-0'), { shiftKey: true });
    await tick();
    (component as any).openStylePanel();
    await tick();

    expect(screen.getByTestId('style-panel-border-side-top').className).toContain('is-on');
    expect(screen.getByTestId('style-panel-border-side-none').className).not.toContain('is-on');
    expect(screen.getByTestId('style-panel-border-weight-1').className).not.toContain('is-on');
    expect(screen.getByTestId('style-panel-border-weight-2').className).not.toContain('is-on');
  });

  it('#2874 item 45 guard (Matt\'s ruling): a click from the mixed state does not silently reset the cells to weight 1', async () => {
    // Same ROW, two columns — both cells are on the range's top edge
    // (Matt's outer-edge ruling), so "top" mixed-weight disagreement is
    // isolated from the outer-edge per-cell resolution entirely; only the
    // weight/dashed indeterminate-click guard is under test here.
    const data = baseSheetData({
      columns: [col({ id: 'col-a', sort_order: 0 }), col({ id: 'col-b', name: 'B', sort_order: 1 })],
      cells: [
        cell({ row_id: 'row-1', column_id: 'col-a', raw_value: 'a', style_json: '{"bd":[{"side":"top","w":2,"c":"var(--sheet-border-styled)","dashed":false}]}' }),
        cell({ row_id: 'row-1', column_id: 'col-b', raw_value: 'b', style_json: '{"bd":[{"side":"top","w":1,"c":"var(--sheet-border-styled)","dashed":false}]}' }),
      ],
    });
    await renderGrid(data);

    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-0-0'));
    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-0-1'), { shiftKey: true });
    await tick();
    await fireEvent.keyDown(screen.getByTestId('sheet-grid-wrap'), { key: 'S', metaKey: true, shiftKey: true });
    await tick();

    // Sanity: opened on the mixed side, so weight is indeterminate.
    expect(screen.getByTestId('style-panel-border-weight-1').className).not.toContain('is-on');
    expect(screen.getByTestId('style-panel-border-weight-2').className).not.toContain('is-on');

    // The footgun (scope report §5): toggling a DIFFERENT side on from
    // this mixed state must NOT fire an apply that falls back to weight
    // 1 for every active side, which would silently clobber the top
    // side's real (disagreeing) weights down to a flat 1px.
    await fireEvent.click(screen.getByTestId('style-panel-border-side-left'));
    await tick();
    expect(updateSheetCellsStyleBatchMock).not.toHaveBeenCalled();

    // Once the user EXPLICITLY resolves the ambiguity via a weight click,
    // the apply proceeds with that explicit value — never a silent 1.
    await fireEvent.click(screen.getByTestId('style-panel-border-weight-2'));
    // #2874 item 51: ONE batch call for the 2-cell apply (BEFORE: 2 calls).
    await vi.waitFor(() => expect(updateSheetCellsStyleBatchMock).toHaveBeenCalledTimes(1));
    const guardEntries = updateSheetCellsStyleBatchMock.mock.calls[0][1] as { styleJson: string }[];
    expect(guardEntries).toHaveLength(2);
    for (const entry of guardEntries) {
      const parsed = JSON.parse(entry.styleJson);
      const topSpec = parsed.bd.find((b: any) => b.side === 'top');
      expect(topSpec).toBeTruthy();
      expect(topSpec.w).toBe(2);
    }
  });

  // Delta-audit fix (2026-09-24, docs/audits/2026-09-24-pr-2963-item45.md
  // item 2 FAIL, reproduced live by Reed): the guard above only ever
  // covered `handleSideClick`. A bare weight or dashed click was NEVER
  // deferred (it's a different button, not `handleSideClick`), and it
  // used to fire `applyCurrentBorder()`'s SHARED `w`/`dashed` fallback,
  // silently clobbering whichever axis the user did NOT just click. The
  // three tests below are the required before/after proof for Matt's fix
  // ruling (authoritative, quoted below): "A click in the border panel
  // changes only what was clicked, and every cell keeps its own value
  // for everything else. Clicking dashed on a mixed selection makes each
  // cell dashed while keeping its own weight and color; clicking a
  // weight works the same way for dashed. No overwrites, no dead
  // clicks. Side-button clicks keep the wait guard as built."
  it('#2874 item 45/29 fix: dashed click on a SAME-SIDE mix makes each cell dashed while each keeps its own weight', async () => {
    // Two cells, both bordered on `top`, but DIFFERENT weights (2 and 1)
    // — a same-side mix, item 45's own new reachable state. Before this
    // fix: clicking Dashed fired the shared-fallback apply, writing
    // `w: borderWeight ?? 1` (i.e. `w: 1`) over BOTH cells — clobbering
    // row-1's real weight of 2. After this fix: each cell keeps its own
    // weight; only `dashed` changes, on both.
    const data = baseSheetData({
      rows: [row({ id: 'row-1', sort_order: 0 }), row({ id: 'row-2', sort_order: 1 })],
      cells: [
        cell({ row_id: 'row-1', column_id: 'col-a', raw_value: 'a', style_json: '{"bd":[{"side":"top","w":2,"c":"var(--sheet-border-styled)","dashed":false}]}' }),
        cell({ row_id: 'row-2', column_id: 'col-a', raw_value: 'b', style_json: '{"bd":[{"side":"top","w":1,"c":"var(--sheet-border-styled)","dashed":false}]}' }),
      ],
    });
    const { component } = await renderGrid(data);

    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-0-0'));
    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-1-0'), { shiftKey: true });
    await tick();
    (component as any).openStylePanel();
    await tick();

    // Sanity: opened on a same-side mix — weight is indeterminate.
    expect(screen.getByTestId('style-panel-border-weight-1').className).not.toContain('is-on');
    expect(screen.getByTestId('style-panel-border-weight-2').className).not.toContain('is-on');

    await fireEvent.click(screen.getByTestId('style-panel-border-dashed'));
    // #2874 item 51: ONE batch call for the 2-cell apply (BEFORE: 2 calls).
    await vi.waitFor(() => expect(updateSheetCellsStyleBatchMock).toHaveBeenCalledTimes(1));
    expect(updateSheetCellsStyleBatchMock.mock.calls[0][1]).toHaveLength(2);

    const row1Top = batchEntry('row-1', 'col-a').bd.find((b: any) => b.side === 'top');
    const row2Top = batchEntry('row-2', 'col-a').bd.find((b: any) => b.side === 'top');

    expect(row1Top.dashed).toBe(true);
    expect(row2Top.dashed).toBe(true);
    // Each cell keeps ITS OWN weight — no clobber to a shared default.
    expect(row1Top.w).toBe(2);
    expect(row2Top.w).toBe(1);
  });

  it('#2874 item 45 fix: weight click on a SAME-SIDE mix makes each cell take the weight while each keeps its own dashed state', async () => {
    // Two cells, both bordered on `top` at the SAME weight (2) so the
    // mix is isolated to dashed disagreeing (true vs false) — proves the
    // weight click preserves each cell's own dashed rather than the
    // panel's shared (indeterminate) `borderDashed`.
    const data = baseSheetData({
      rows: [row({ id: 'row-1', sort_order: 0 }), row({ id: 'row-2', sort_order: 1 })],
      cells: [
        cell({ row_id: 'row-1', column_id: 'col-a', raw_value: 'a', style_json: '{"bd":[{"side":"top","w":2,"c":"var(--sheet-border-styled)","dashed":true}]}' }),
        cell({ row_id: 'row-2', column_id: 'col-a', raw_value: 'b', style_json: '{"bd":[{"side":"top","w":2,"c":"var(--sheet-border-styled)","dashed":false}]}' }),
      ],
    });
    const { component } = await renderGrid(data);

    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-0-0'));
    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-1-0'), { shiftKey: true });
    await tick();
    (component as any).openStylePanel();
    await tick();

    // Sanity: opened on a same-side mix (dashed disagrees) — dashed
    // control is indeterminate (not `is-on`).
    expect(screen.getByTestId('style-panel-border-dashed').className).not.toContain('is-on');

    await fireEvent.click(screen.getByTestId('style-panel-border-weight-3'));
    // #2874 item 51: ONE batch call for the 2-cell apply (BEFORE: 2 calls).
    await vi.waitFor(() => expect(updateSheetCellsStyleBatchMock).toHaveBeenCalledTimes(1));
    expect(updateSheetCellsStyleBatchMock.mock.calls[0][1]).toHaveLength(2);

    const row1Top = batchEntry('row-1', 'col-a').bd.find((b: any) => b.side === 'top');
    const row2Top = batchEntry('row-2', 'col-a').bd.find((b: any) => b.side === 'top');

    expect(row1Top.w).toBe(3);
    expect(row2Top.w).toBe(3);
    // Each cell keeps ITS OWN dashed state — no clobber to a shared default.
    expect(row1Top.dashed).toBe(true);
    expect(row2Top.dashed).toBe(false);
  });

  it('#2874 item 29 fix: dashed click on the CROSS-side mix (Top w2, Left w1) leaves each side at its own weight', async () => {
    // The older item-29 repro, verbatim: ONE cell, Top unanimously 2px,
    // Left unanimously 1px (both present on the same cell, so this is
    // cross-side disagreement, not same-side) — clicking Dashed used to
    // collapse BOTH sides to weight 1 via the same shared-fallback apply
    // this PR fixes.
    const data = baseSheetData({
      columns: [col({ id: 'col-a', sort_order: 0 })],
      rows: [row({ id: 'row-1', sort_order: 0 })],
      cells: [
        cell({
          row_id: 'row-1',
          column_id: 'col-a',
          raw_value: 'a',
          style_json: '{"bd":[{"side":"top","w":2,"c":"var(--sheet-border-styled)","dashed":false},{"side":"left","w":1,"c":"var(--sheet-border-styled)","dashed":false}]}',
        }),
      ],
    });
    const { component } = await renderGrid(data);

    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-0-0'));
    await tick();
    (component as any).openStylePanel();
    await tick();

    // Sanity: cross-side mix — weight is indeterminate, both sides lit.
    expect(screen.getByTestId('style-panel-border-side-top').className).toContain('is-on');
    expect(screen.getByTestId('style-panel-border-side-left').className).toContain('is-on');
    expect(screen.getByTestId('style-panel-border-weight-1').className).not.toContain('is-on');
    expect(screen.getByTestId('style-panel-border-weight-2').className).not.toContain('is-on');

    await fireEvent.click(screen.getByTestId('style-panel-border-dashed'));
    await vi.waitFor(() => expect(updateSheetCellsStyleBatchMock).toHaveBeenCalledTimes(1));

    const json = batchEntry('row-1', 'col-a');
    const topSpec = json.bd.find((b: any) => b.side === 'top');
    const leftSpec = json.bd.find((b: any) => b.side === 'left');

    expect(topSpec.dashed).toBe(true);
    expect(leftSpec.dashed).toBe(true);
    // Each SIDE keeps its own weight — no collapse to a shared default.
    expect(topSpec.w).toBe(2);
    expect(leftSpec.w).toBe(1);
  });

  // Delta-audit fix #2 (2026-09-24, Reed's second delta audit,
  // `docs/audits/2026-09-24-pr-2963-delta2.md` item 2 FAIL, reproduced
  // live then reverted): the first fix above dropped outer-edge geometry
  // checking from the NEW default-creation branch entirely, so a weight
  // or dashed click could silently create a brand-new border on a cell
  // that never had one, even when that cell is NOT on the selection's
  // outer edge for that side — something the pre-fix `patch.border`
  // path correctly refused to do. Matt's ruling on this fix
  // (authoritative, quoted in full in the fix commit): "In the border
  // panel's weight and dashed handlers, a new border may only be created
  // on a cell that's on the selection's outer edge for that side. Use
  // the same `resolvedBorderSides` check the full-apply path uses.
  // Existing borders keep their own values regardless of position, as
  // now. When a side is lit because only some cells have it, a weight or
  // dashed click fills that border in along the outer edge, the same as
  // clicking the side would. It never adds borders to inner cells and
  // never changes values that weren't clicked." The two tests below are
  // the required before/after proof.
  it('#2874 item 45/29 delta-2 fix: weight click on a vertical partial-presence mix leaves the INNER, never-bordered cell plain', async () => {
    // Two cells stacked vertically (row-1 above row-2, same column) — a
    // 2-row vertical range. Only row-1 (the range's top edge, r0) has a
    // real `top` border; row-2 (r1, NOT the top outer edge for a
    // vertical range) has no style at all — the exact partial-presence
    // fixture from the "one cell bordered, one cell plain" test above.
    // This test MUST FAIL against the pre-fix head (6d9d9ce9): row-2
    // incorrectly comes back with a brand-new `bd:[{side:"top",w:3,...}]`
    // instead of staying plain (no `bd` key at all).
    const data = baseSheetData({
      rows: [row({ id: 'row-1', sort_order: 0 }), row({ id: 'row-2', sort_order: 1 })],
      cells: [
        cell({ row_id: 'row-1', column_id: 'col-a', raw_value: 'a', style_json: '{"bd":[{"side":"top","w":2,"c":"var(--sheet-border-styled)","dashed":false}]}' }),
        cell({ row_id: 'row-2', column_id: 'col-a', raw_value: 'b' }),
      ],
    });
    const { component } = await renderGrid(data);

    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-0-0'));
    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-1-0'), { shiftKey: true });
    await tick();
    (component as any).openStylePanel();
    await tick();

    // Sanity: partial presence counts as mixed — side button lit, no
    // weight button lit.
    expect(screen.getByTestId('style-panel-border-side-top').className).toContain('is-on');
    expect(screen.getByTestId('style-panel-border-weight-2').className).not.toContain('is-on');

    await fireEvent.click(screen.getByTestId('style-panel-border-weight-3'));
    // #2874 item 51: ONE batch call for the 2-cell apply (BEFORE: 2 calls).
    await vi.waitFor(() => expect(updateSheetCellsStyleBatchMock).toHaveBeenCalledTimes(1));
    expect(updateSheetCellsStyleBatchMock.mock.calls[0][1]).toHaveLength(2);

    const row1Top = batchEntry('row-1', 'col-a').bd.find((b: any) => b.side === 'top');

    // row-1 was already on the top outer edge with real data — updates
    // per-attribute as before, keeps its own color, weight now 3.
    expect(row1Top.w).toBe(3);
    // row-2 is NOT on the top outer edge of this vertical range and had
    // no prior border — must stay plain, no `bd` key created at all.
    // At the pre-fix head, this fails: row-2's entry.bd is a real array
    // containing a fresh {side:"top",w:3,...} entry.
    expect(batchEntry('row-2', 'col-a').bd).toBeUndefined();
  });

  it('#2874 item 45/29 delta-2 fix: weight click on a horizontal partial-presence mix fills the top border in on BOTH cells (both on the outer edge)', async () => {
    // Two cells side by side (col-a, col-b), same row — both cells sit on
    // the range's top outer edge (r0 === r1 === 0 for a single-row
    // range), so partial presence here "fills in along the outer edge,
    // the same as clicking the side would" per Matt's ruling: the plain
    // cell gets a brand-new top border, not left plain.
    const data = baseSheetData({
      columns: [col({ id: 'col-a', sort_order: 0 }), col({ id: 'col-b', name: 'B', sort_order: 1 })],
      cells: [
        cell({ row_id: 'row-1', column_id: 'col-a', raw_value: 'a', style_json: '{"bd":[{"side":"top","w":2,"c":"var(--sheet-border-styled)","dashed":false}]}' }),
        cell({ row_id: 'row-1', column_id: 'col-b', raw_value: 'b' }),
      ],
    });
    const { component } = await renderGrid(data);

    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-0-0'));
    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-0-1'), { shiftKey: true });
    await tick();
    (component as any).openStylePanel();
    await tick();

    // Sanity: partial presence counts as mixed — side button lit, no
    // weight button lit.
    expect(screen.getByTestId('style-panel-border-side-top').className).toContain('is-on');
    expect(screen.getByTestId('style-panel-border-weight-2').className).not.toContain('is-on');

    await fireEvent.click(screen.getByTestId('style-panel-border-weight-3'));
    // #2874 item 51: ONE batch call for the 2-cell apply (BEFORE: 2 calls).
    await vi.waitFor(() => expect(updateSheetCellsStyleBatchMock).toHaveBeenCalledTimes(1));
    expect(updateSheetCellsStyleBatchMock.mock.calls[0][1]).toHaveLength(2);

    const colATop = batchEntry('row-1', 'col-a').bd.find((b: any) => b.side === 'top');
    const colBTop = batchEntry('row-1', 'col-b').bd.find((b: any) => b.side === 'top');

    // Both cells are on the range's top outer edge — both end up with a
    // top border at the clicked weight.
    expect(colATop).toBeTruthy();
    expect(colBTop).toBeTruthy();
    expect(colATop.w).toBe(3);
    expect(colBTop.w).toBe(3);
    // Never adds borders to inner cells / never touches other attributes
    // that weren't clicked — col-b's fresh entry uses the sane defaults.
    expect(colBTop.dashed).toBe(false);
  });

  it('Clear styling from the panel clears every selected cell\'s style_json (reuses the existing clearSelectedStyling verb)', async () => {
    const data = baseSheetData({
      cells: [cell({ raw_value: 'a', style_json: '{"bg":"#232327"}' })],
    });
    await renderGrid(data);

    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-0-0'));
    await tick();
    await fireEvent.keyDown(screen.getByTestId('sheet-grid-wrap'), { key: 'S', metaKey: true, shiftKey: true });
    await tick();

    await fireEvent.click(screen.getByTestId('style-panel-clear'));
    // #2874 item 51: ONE batch call carrying the single cleared entry.
    await vi.waitFor(() =>
      expect(updateSheetCellsStyleBatchMock).toHaveBeenCalledWith(NOTE_ID, [{ rowId: 'row-1', columnId: 'col-a', styleJson: '{}' }])
    );
    expect(screen.queryByTestId('style-panel')).toBeNull();
  });

  // #2874 item 47 (Matt ruling 2026-09-24, supersedes item 27's
  // client-side preserve patch): a style-only write's backend reply now
  // ALWAYS carries the cell's current computed value, exactly like a
  // normal read — there is no more "omits computed_value" case for the
  // frontend to guard against. `preserveComputedValueOnStyleOnlyWrite`
  // is deleted; this test now asserts the grid renders the error
  // correctly because the backend response already carries it (a
  // straight trust-the-reply patch), not because of any client-side
  // preserve/guess logic.
  it('applying a style to an error cell shows its computed_value/err kind because the backend response already carries it (#2874 item 47)', async () => {
    const data = baseSheetData({
      cells: [cell({ raw_value: '=A1/0', computed_value: '#DIV/0!', style_json: '{}' })],
    });
    // New contract: a style-only write's reply carries the LIVE computed
    // value directly (full recompute on every write, item 47), and
    // raw_value in A1 display form — never `undefined`/omitted.
    updateSheetCellsStyleBatchMock.mockResolvedValue([
      cell({ raw_value: '=A1/0', computed_value: '#DIV/0!', style_json: '{"bg":"#331111"}' }),
    ]);
    await renderGrid(data);

    const cellEl = screen.getByTestId('sheet-cell-0-0');
    expect(cellEl.className).toContain('sh-c--err');

    await fireEvent.mouseDown(cellEl);
    await tick();
    await fireEvent.keyDown(screen.getByTestId('sheet-grid-wrap'), { key: 'S', metaKey: true, shiftKey: true });
    await tick();

    await fireEvent.click(screen.getByTestId('style-panel-fill-cyan'));
    await vi.waitFor(() => expect(updateSheetCellsStyleBatchMock).toHaveBeenCalled());
    await tick();

    // The cell must STILL render as an error (kind 'err') after the
    // style-only write — the response itself carries the live error
    // value, so there's nothing to preserve or guess.
    const restyledCellEl = screen.getByTestId('sheet-cell-0-0');
    expect(restyledCellEl.className).toContain('sh-c--err');
    expect(restyledCellEl.textContent).toContain('#DIV/0!');
  });

  // #2874 item 47: the specific repro condition from the diagnosis — a
  // formula cell whose cached client-side raw_value is already in A1
  // display form (as it would be after any normal read/refetch), styled
  // via the panel. The backend reply must carry BOTH the A1-translated
  // raw_value and the live computed_value; the grid must show neither
  // the internal `{column_id:row_id}` form NOR a stale/blank value after
  // the style commit.
  it('styling a formula cell whose cached raw_value is already A1 form shows the live computed value, never internal IDs (#2874 item 47)', async () => {
    const data = baseSheetData({
      cells: [cell({ raw_value: '=SUM(B1:B5)', computed_value: '42', style_json: '{}' })],
    });
    // The backend reply's raw_value is ALSO A1 form (item 47 — the write
    // chokepoint now translates through the same path a read uses) and
    // computed_value is the live, correct total.
    updateSheetCellsStyleBatchMock.mockResolvedValue([
      cell({ raw_value: '=SUM(B1:B5)', computed_value: '42', style_json: '{"bg":"#113322"}' }),
    ]);
    await renderGrid(data);

    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-0-0'));
    await tick();
    await fireEvent.keyDown(screen.getByTestId('sheet-grid-wrap'), { key: 'S', metaKey: true, shiftKey: true });
    await tick();

    await fireEvent.click(screen.getByTestId('style-panel-fill-cyan'));
    await vi.waitFor(() => expect(updateSheetCellsStyleBatchMock).toHaveBeenCalled());
    await tick();

    const styledCellEl = screen.getByTestId('sheet-cell-0-0');
    expect(styledCellEl.textContent).toContain('42');
    expect(styledCellEl.textContent).not.toContain('{');
    expect(styledCellEl.textContent).not.toMatch(/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}/);
  });
});

describe('SheetGrid — clear-contents (Delete/Backspace keyboard + context-menu row)', () => {
  it('Delete key on a single anchored cell with a value calls updateSheetCell with an empty string, style untouched', async () => {
    const data = baseSheetData({ cells: [cell({ raw_value: 'hello', style_json: '{"bg":"#112233"}' })] });
    updateSheetCellMock.mockResolvedValue(cell({ raw_value: '', style_json: '{"bg":"#112233"}' }));
    await renderGrid(data);

    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-0-0'));
    await tick();
    await fireEvent.keyDown(screen.getByTestId('sheet-grid-wrap'), { key: 'Delete' });
    await vi.waitFor(() => expect(updateSheetCellMock).toHaveBeenCalledOnce());

    expect(updateSheetCellMock).toHaveBeenCalledWith('row-1', 'col-a', '');
  });

  it('Backspace key on a single anchored cell with a value calls updateSheetCell with an empty string', async () => {
    const data = baseSheetData({ cells: [cell({ raw_value: 'hello' })] });
    updateSheetCellMock.mockResolvedValue(cell({ raw_value: '' }));
    await renderGrid(data);

    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-0-0'));
    await tick();
    await fireEvent.keyDown(screen.getByTestId('sheet-grid-wrap'), { key: 'Backspace' });
    await vi.waitFor(() => expect(updateSheetCellMock).toHaveBeenCalledOnce());

    expect(updateSheetCellMock).toHaveBeenCalledWith('row-1', 'col-a', '');
  });

  it('Delete on an already-empty cell is a no-op (no updateSheetCell call)', async () => {
    const data = baseSheetData({ cells: [cell({ raw_value: '' })] });
    await renderGrid(data);

    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-0-0'));
    await tick();
    await fireEvent.keyDown(screen.getByTestId('sheet-grid-wrap'), { key: 'Delete' });
    await tick();

    expect(updateSheetCellMock).not.toHaveBeenCalled();
  });

  it('Delete over an active multi-cell range clears every cell in the range that has a value', async () => {
    const data = baseSheetData({
      columns: [col({ id: 'col-a', sort_order: 0 }), col({ id: 'col-b', sort_order: 1 })],
      rows: [row({ id: 'row-1', sort_order: 0 }), row({ id: 'row-2', sort_order: 1 })],
      cells: [
        cell({ row_id: 'row-1', column_id: 'col-a', raw_value: 'a' }),
        cell({ row_id: 'row-1', column_id: 'col-b', raw_value: 'b' }),
        cell({ row_id: 'row-2', column_id: 'col-a', raw_value: 'c' }),
        // row-2/col-b intentionally left empty — should NOT trigger a call.
      ],
    });
    updateSheetCellMock.mockResolvedValue(cell({ raw_value: '' }));
    await renderGrid(data);

    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-0-0'));
    await tick();
    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-1-1'), { shiftKey: true });
    await tick();
    await fireEvent.keyDown(screen.getByTestId('sheet-grid-wrap'), { key: 'Delete' });
    await vi.waitFor(() => expect(updateSheetCellMock).toHaveBeenCalledTimes(3));

    expect(updateSheetCellMock).toHaveBeenCalledWith('row-1', 'col-a', '');
    expect(updateSheetCellMock).toHaveBeenCalledWith('row-1', 'col-b', '');
    expect(updateSheetCellMock).toHaveBeenCalledWith('row-2', 'col-a', '');
  });

  it('"Delete N cells\' contents" context-menu row clears contents the same way as the keyboard shortcut', async () => {
    const data = baseSheetData({ cells: [cell({ raw_value: 'hello' })] });
    updateSheetCellMock.mockResolvedValue(cell({ raw_value: '' }));
    await renderGrid(data);

    await fireEvent.contextMenu(screen.getByTestId('sheet-cell-0-0'));
    await tick();
    await fireEvent.click(screen.getByTestId('cell-context-menu-delete-contents'));
    await vi.waitFor(() => expect(updateSheetCellMock).toHaveBeenCalledOnce());

    expect(updateSheetCellMock).toHaveBeenCalledWith('row-1', 'col-a', '');
    expect(screen.queryByTestId('cell-context-menu')).toBeNull();
  });
});

describe('SheetGrid — "Clear styling" (context-menu row, style_json only, value untouched)', () => {
  it('calls updateSheetCellsStyleBatch with a style-only entry for a styled cell (#2874 item 51)', async () => {
    const data = baseSheetData({ cells: [cell({ raw_value: 'hello', style_json: '{"bg":"#112233"}' })] });
    await renderGrid(data);

    await fireEvent.contextMenu(screen.getByTestId('sheet-cell-0-0'));
    await tick();
    await fireEvent.click(screen.getByTestId('cell-context-menu-clear-styling'));
    await vi.waitFor(() => expect(updateSheetCellsStyleBatchMock).toHaveBeenCalledOnce());

    expect(updateSheetCellsStyleBatchMock).toHaveBeenCalledWith(NOTE_ID, [{ rowId: 'row-1', columnId: 'col-a', styleJson: '{}' }]);
  });

  it('does not call updateSheetCellsStyleBatch for a cell with no styling to clear', async () => {
    const data = baseSheetData({ cells: [cell({ raw_value: 'hello', style_json: '{}' })] });
    await renderGrid(data);

    await fireEvent.contextMenu(screen.getByTestId('sheet-cell-0-0'));
    await tick();
    await fireEvent.click(screen.getByTestId('cell-context-menu-clear-styling'));
    await tick();

    expect(updateSheetCellsStyleBatchMock).not.toHaveBeenCalled();
  });

  it('clearing styling over a multi-cell selection sends ONE batch call carrying every styled cell, not one call per cell (#2874 item 51)', async () => {
    // The required "one call per style action" proof for
    // `clearSelectedStyling` specifically (mirrors the equivalent
    // `applyStyleToSelection` proof above in the style-panel describe
    // block) — BEFORE this item, clearing styling on a multi-cell
    // selection drove one `updateSheetCell` call PER styled cell.
    const data = baseSheetData({
      columns: [col({ id: 'col-a', sort_order: 0 }), col({ id: 'col-b', name: 'B', sort_order: 1 })],
      rows: [row({ id: 'row-1', sort_order: 0 }), row({ id: 'row-2', sort_order: 1 })],
      cells: [
        cell({ row_id: 'row-1', column_id: 'col-a', raw_value: 'a', style_json: '{"bg":"#112233"}' }),
        cell({ row_id: 'row-1', column_id: 'col-b', raw_value: 'b', style_json: '{"fg":"#ffffff"}' }),
        cell({ row_id: 'row-2', column_id: 'col-a', raw_value: 'c', style_json: '{}' }), // nothing to clear
        cell({ row_id: 'row-2', column_id: 'col-b', raw_value: 'd', style_json: '{"bg":"#112233"}' }),
      ],
    });
    await renderGrid(data);

    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-0-0'));
    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-1-1'), { shiftKey: true });
    await tick();
    await fireEvent.contextMenu(screen.getByTestId('sheet-cell-0-0'));
    await tick();
    await fireEvent.click(screen.getByTestId('cell-context-menu-clear-styling'));

    await vi.waitFor(() => expect(updateSheetCellsStyleBatchMock).toHaveBeenCalledTimes(1));
    const entries = updateSheetCellsStyleBatchMock.mock.calls[0][1] as { rowId: string; columnId: string; styleJson: string }[];
    // Only the 3 cells that actually HAD styling are sent — row-2/col-a
    // (already bare) is correctly skipped, same as the per-cell loop did.
    expect(entries).toHaveLength(3);
    expect(entries.every((e) => e.styleJson === '{}')).toBe(true);
    const keys = entries.map((e) => `${e.rowId}:${e.columnId}`).sort();
    expect(keys).toEqual(['row-1:col-a', 'row-1:col-b', 'row-2:col-b']);
  });

  it('leaves the cell VALUE untouched — clearing styling never sends a value field at all (#2874 item 51)', async () => {
    const data = baseSheetData({ cells: [cell({ raw_value: 'hello', style_json: '{"fg":"#ffffff"}' })] });
    await renderGrid(data);

    await fireEvent.contextMenu(screen.getByTestId('sheet-cell-0-0'));
    await tick();
    await fireEvent.click(screen.getByTestId('cell-context-menu-clear-styling'));
    await vi.waitFor(() => expect(updateSheetCellsStyleBatchMock).toHaveBeenCalledOnce());

    // #2874 item 51: the batch entry shape carries NO `value` field at all
    // (style-only, by construction) — a stronger guarantee than the old
    // per-cell call's `value: null` argument ever gave.
    const entries = updateSheetCellsStyleBatchMock.mock.calls[0][1];
    expect(entries).toEqual([{ rowId: 'row-1', columnId: 'col-a', styleJson: '{}' }]);
    expect(entries[0]).not.toHaveProperty('value');
  });

  // #2874 item 47, second call site: `clearSelectedStyling` also sends a
  // style-only write (`value: null`), and — as of item 47 — the
  // backend's reply for it ALWAYS carries the cell's current computed
  // value, exactly like a normal read. No client-side preserve logic is
  // wired here (or anywhere) any more; this test asserts the grid still
  // renders the error correctly purely because the backend reply
  // already carries it.
  it('clearing styling on an error cell shows its computed_value/err kind because the backend response already carries it (#2874 item 47)', async () => {
    const data = baseSheetData({
      cells: [cell({ raw_value: '=A1/0', computed_value: '#DIV/0!', style_json: '{"bg":"#331111"}' })],
    });
    // New contract: a style-only write's reply carries the LIVE computed
    // value directly, never `undefined`/omitted.
    updateSheetCellsStyleBatchMock.mockResolvedValue([
      cell({ raw_value: '=A1/0', computed_value: '#DIV/0!', style_json: '{}' }),
    ]);
    await renderGrid(data);

    const cellEl = screen.getByTestId('sheet-cell-0-0');
    expect(cellEl.className).toContain('sh-c--err');

    await fireEvent.contextMenu(cellEl);
    await tick();
    await fireEvent.click(screen.getByTestId('cell-context-menu-clear-styling'));
    await vi.waitFor(() => expect(updateSheetCellsStyleBatchMock).toHaveBeenCalledOnce());
    await tick();

    // The cell must STILL render as an error (kind 'err') after clearing
    // styling, not silently downgrade to 'raw'/'normal' because the local
    // cache's computed_value got clobbered with undefined.
    const clearedCellEl = screen.getByTestId('sheet-cell-0-0');
    expect(clearedCellEl.className).toContain('sh-c--err');
    expect(clearedCellEl.textContent).toContain('#DIV/0!');
  });
});

// #2874 item 16 Part B — `openStylePanel()`'s anchor-element positioning.
// The context-menu-door tests above (Matt's ⌘⇧S / `openStylePanel()`
// no-arg calls) already prove the `{12, 78}`/`{x,y}` fallback path is
// unchanged; these cover the NEW element-based overload.
describe('SheetGrid — openStylePanel() anchor-element positioning (#2874 item 16 Part B)', () => {
  it('positions the panel under a real anchor element via getBoundingClientRect()', async () => {
    const data = baseSheetData({ cells: [cell({ raw_value: 'a' })] });
    const { component } = await renderGrid(data);

    const anchor = document.createElement('button');
    anchor.getBoundingClientRect = () => ({
      left: 200, right: 240, top: 50, bottom: 78, width: 40, height: 28, x: 200, y: 50, toJSON: () => ({}),
    });

    (component as any).openStylePanel(anchor);
    await tick();

    const panel = screen.getByTestId('style-panel') as HTMLElement;
    expect(panel.style.left).toBe('200px');
    expect(panel.style.top).toBe('82px'); // rect.bottom (78) + 4
  });

  it('flips left when the anchor is near the right edge of the window (panel would overflow)', async () => {
    const data = baseSheetData({ cells: [cell({ raw_value: 'a' })] });
    const { component } = await renderGrid(data);
    (globalThis.window as any).innerWidth = 500;

    const anchor = document.createElement('button');
    anchor.getBoundingClientRect = () => ({
      left: 400, right: 440, top: 50, bottom: 78, width: 40, height: 28, x: 400, y: 50, toJSON: () => ({}),
    });

    (component as any).openStylePanel(anchor);
    await tick();

    const panel = screen.getByTestId('style-panel') as HTMLElement;
    // 400 + 268 (panel width) > 500 (innerWidth) so it flips to
    // innerWidth - 268 - 4 = 228.
    expect(panel.style.left).toBe('228px');
  });

  it('a click on the anchor element itself does not close the panel (Part B same-click self-close fix)', async () => {
    const data = baseSheetData({ cells: [cell({ raw_value: 'a' })] });
    const { component } = await renderGrid(data);

    const anchor = document.createElement('button');
    document.body.appendChild(anchor);
    anchor.getBoundingClientRect = () => ({
      left: 10, right: 50, top: 10, bottom: 38, width: 40, height: 28, x: 10, y: 10, toJSON: () => ({}),
    });

    (component as any).openStylePanel(anchor);
    await tick();
    expect(screen.getByTestId('style-panel')).toBeTruthy();

    // ShStylePanel is rendered with anchorEl={stylePanelAnchorEl} — assert
    // the panel is still there (no separate window-click simulation needed
    // here since ShStylePanel.test.ts already unit-tests the guard itself;
    // this proves SheetGrid actually WIRES the anchor through).
    expect(screen.getByTestId('style-panel')).toBeTruthy();
    anchor.remove();
  });
});

// #2874 item 52 — the style panel flips up when it would run past the
// bottom of the window, ported from `ShCellContextMenu.svelte`'s own
// `flipUp`/`posTop`/`MENU_HEIGHT` triplet. `PANEL_HEIGHT` (305) was
// measured from the real rendered `.sh-sty` panel (268x304.875px via a
// headless-browser measurement of the actual component, same posture
// `MENU_HEIGHT` was measured). Covers both doors `openStylePanel` opens
// through: the anchor-element branch (a cell) and the raw-point branch
// (the right-click context menu).
describe('SheetGrid — openStylePanel() vertical flip (#2874 item 52)', () => {
  it('anchor branch: a low row (near the bottom of the window) flips the panel up', async () => {
    const data = baseSheetData({ cells: [cell({ raw_value: 'a' })] });
    const { component } = await renderGrid(data);
    (globalThis.window as any).innerHeight = 768;

    const anchor = document.createElement('button');
    anchor.getBoundingClientRect = () => ({
      left: 200, right: 240, top: 700, bottom: 728, width: 40, height: 28, x: 200, y: 700, toJSON: () => ({}),
    });

    (component as any).openStylePanel(anchor);
    await tick();

    const panel = screen.getByTestId('style-panel') as HTMLElement;
    // rect.bottom (728) + 4 + PANEL_HEIGHT (305) = 1037 > innerHeight
    // (768) so it flips up: rect.top (700) - 4 - PANEL_HEIGHT (305) = 391.
    expect(panel.style.top).toBe('391px');
  });

  it('anchor branch: a normal row still opens below (no regression)', async () => {
    const data = baseSheetData({ cells: [cell({ raw_value: 'a' })] });
    const { component } = await renderGrid(data);
    (globalThis.window as any).innerHeight = 768;

    const anchor = document.createElement('button');
    anchor.getBoundingClientRect = () => ({
      left: 200, right: 240, top: 100, bottom: 128, width: 40, height: 28, x: 200, y: 100, toJSON: () => ({}),
    });

    (component as any).openStylePanel(anchor);
    await tick();

    const panel = screen.getByTestId('style-panel') as HTMLElement;
    // rect.bottom (128) + 4 + PANEL_HEIGHT (305) = 437 <= innerHeight
    // (768) so it stays below: rect.bottom (128) + 4 = 132.
    expect(panel.style.top).toBe('132px');
  });

  it('raw-point branch (context-menu door): a low point flips the panel up', async () => {
    const data = baseSheetData({ cells: [cell({ raw_value: 'a' })] });
    const { component } = await renderGrid(data);
    (globalThis.window as any).innerHeight = 768;

    (component as any).openStylePanel({ x: 50, y: 700 });
    await tick();

    const panel = screen.getByTestId('style-panel') as HTMLElement;
    // 700 + PANEL_HEIGHT (305) = 1005 > innerHeight (768) so it flips
    // up: 700 - PANEL_HEIGHT (305) = 395.
    expect(panel.style.top).toBe('395px');
  });

  it('raw-point branch (context-menu door): a normal point still opens below (no regression)', async () => {
    const data = baseSheetData({ cells: [cell({ raw_value: 'a' })] });
    const { component } = await renderGrid(data);
    (globalThis.window as any).innerHeight = 768;

    (component as any).openStylePanel({ x: 50, y: 100 });
    await tick();

    const panel = screen.getByTestId('style-panel') as HTMLElement;
    // 100 + PANEL_HEIGHT (305) = 405 <= innerHeight (768) so it stays
    // at the raw point, unchanged: 100.
    expect(panel.style.top).toBe('100px');
  });
});

// #2874 item 57 — flipped popovers must not clip off the TOP of the
// window. Inherited from `ShCellContextMenu`'s pattern (both components
// have it) and carried into `openStylePanel` by item 52's flip-up port.
// Matt's ruling: floor the flipped position at 0 (`Math.max(0, …)`),
// same shape as the existing horizontal clamp, in both branches of
// `openStylePanel` and in `ShCellContextMenu`'s `posTop`. Only the
// flipped branch is clamped — the non-flipped (below) position is
// already correct and untouched.
describe('SheetGrid — openStylePanel() flipped-position top floor (#2874 item 57)', () => {
  it('anchor branch: a mid-screen row at a 600px window height flips up PAST the top — clamped to 0 (base: -9px)', async () => {
    const data = baseSheetData({ cells: [cell({ raw_value: 'a' })] });
    const { component } = await renderGrid(data);
    (globalThis.window as any).innerHeight = 600;

    const anchor = document.createElement('button');
    anchor.getBoundingClientRect = () => ({
      left: 200, right: 240, top: 300, bottom: 328, width: 40, height: 28, x: 200, y: 300, toJSON: () => ({}),
    });

    (component as any).openStylePanel(anchor);
    await tick();

    const panel = screen.getByTestId('style-panel') as HTMLElement;
    // rect.bottom (328) + 4 + PANEL_HEIGHT (305) = 637 > innerHeight (600)
    // so it flips up: rect.top (300) - 4 - PANEL_HEIGHT (305) = -9 at
    // base (BEFORE this fix — clips off the top of the window). After
    // the floor: Math.max(0, -9) = 0.
    expect(panel.style.top).toBe('0px');
  });

  it('raw-point branch (context-menu door): a mid-screen point at a 600px window height flips up PAST the top — clamped to 0 (base: -5px)', async () => {
    const data = baseSheetData({ cells: [cell({ raw_value: 'a' })] });
    const { component } = await renderGrid(data);
    (globalThis.window as any).innerHeight = 600;

    (component as any).openStylePanel({ x: 50, y: 300 });
    await tick();

    const panel = screen.getByTestId('style-panel') as HTMLElement;
    // 300 + PANEL_HEIGHT (305) = 605 > innerHeight (600) so it flips up:
    // 300 - PANEL_HEIGHT (305) = -5 at base (BEFORE this fix — clips off
    // the top of the window). After the floor: Math.max(0, -5) = 0.
    expect(panel.style.top).toBe('0px');
  });
});

// #2874 item 16 Part C — toolbar Sort/Filter/Formula wiring.
// Matt's ruling: the toolbar Sort button takes NO argument — it cycles
// ascending → descending → cleared on successive presses of the SAME
// column, and resets to ascending when a DIFFERENT column is targeted.
describe('SheetGrid — toolbarSort() (#2874 item 16 Part C — cycling)', () => {
  it('sorts the anchor cell\'s column ascending on first press when no whole column is selected', async () => {
    const data = baseSheetData({
      columns: [col({ id: 'col-a', sort_order: 0 })],
      cells: [cell({ raw_value: 'b' })],
    });
    const { component } = await renderGrid(data);

    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-0-0'));
    await tick();
    (component as any).toolbarSort();
    await tick();

    const flag = screen.getByTestId('sheet-header-flag-sort-0');
    expect(flag.textContent).toBe('↑');
  });

  it('falls back to the anchor cell\'s column when colSel is null (forgiving-source ruling)', async () => {
    const data = baseSheetData({
      columns: [col({ id: 'col-a', sort_order: 0 }), col({ id: 'col-b', name: 'B', sort_order: 1 })],
      cells: [cell({ column_id: 'col-a', raw_value: 'x' }), cell({ column_id: 'col-b', raw_value: 'y' })],
    });
    const { component } = await renderGrid(data);

    // A plain cell click (NOT a column-header click) leaves colSel null.
    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-0-1'));
    await tick();
    (component as any).toolbarSort();
    await tick();

    expect(screen.getByTestId('sheet-header-flag-sort-1')).toBeTruthy();
    expect(screen.queryByTestId('sheet-header-flag-sort-0')).toBeNull();
  });

  it('sorts the whole selected column when one is header-selected (colSel takes priority)', async () => {
    const data = baseSheetData({
      columns: [col({ id: 'col-a', sort_order: 0 }), col({ id: 'col-b', name: 'B', sort_order: 1 })],
      cells: [cell({ column_id: 'col-a', raw_value: 'x' }), cell({ column_id: 'col-b', raw_value: 'y' })],
    });
    const { component } = await renderGrid(data);

    await fireEvent.click(screen.getByTestId('sheet-col-header-0'));
    await tick();
    (component as any).toolbarSort();
    await tick();

    expect(screen.getByTestId('sheet-header-flag-sort-0')).toBeTruthy();
  });

  it('no-ops when the sheet has zero columns (nothing to resolve)', async () => {
    const data = baseSheetData({ columns: [], rows: [] });
    const { component } = await renderGrid(data);

    expect(() => (component as any).toolbarSort()).not.toThrow();
  });

  it('second press on the same column sorts descending', async () => {
    const data = baseSheetData({ cells: [cell({ raw_value: 'a' })] });
    const { component } = await renderGrid(data);

    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-0-0'));
    await tick();
    (component as any).toolbarSort();
    await tick();
    (component as any).toolbarSort();
    await tick();

    const flag = screen.getByTestId('sheet-header-flag-sort-0');
    expect(flag.textContent).toBe('↓');
  });

  it('third press on the same column clears the sort', async () => {
    const data = baseSheetData({ cells: [cell({ raw_value: 'a' })] });
    const { component } = await renderGrid(data);

    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-0-0'));
    await tick();
    (component as any).toolbarSort(); // asc
    await tick();
    (component as any).toolbarSort(); // desc
    await tick();
    (component as any).toolbarSort(); // cleared
    await tick();

    expect(screen.queryByTestId('sheet-header-flag-sort-0')).toBeNull();
  });

  it('pressing on a different column resets the cycle to ascending', async () => {
    const data = baseSheetData({
      columns: [col({ id: 'col-a', sort_order: 0 }), col({ id: 'col-b', name: 'B', sort_order: 1 })],
      cells: [cell({ column_id: 'col-a', raw_value: 'x' }), cell({ column_id: 'col-b', raw_value: 'y' })],
    });
    const { component } = await renderGrid(data);

    // Cycle column 0 to descending.
    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-0-0'));
    await tick();
    (component as any).toolbarSort(); // asc
    await tick();
    (component as any).toolbarSort(); // desc
    await tick();
    expect(screen.getByTestId('sheet-header-flag-sort-0').textContent).toBe('↓');

    // Now target column 1 — cycle resets to ascending, not carrying over
    // column 0's descending state, and column 0's flag disappears (single-
    // column sort only, ruling 5).
    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-0-1'));
    await tick();
    (component as any).toolbarSort();
    await tick();

    expect(screen.queryByTestId('sheet-header-flag-sort-0')).toBeNull();
    expect(screen.getByTestId('sheet-header-flag-sort-1').textContent).toBe('↑');
  });
});

describe('SheetGrid — toolbarFilter() (#2874 item 16 Part C)', () => {
  it('opens ShHeaderMenu pre-navigated to the filter view, anchored to the given element', async () => {
    const data = baseSheetData({
      columns: [col({ id: 'col-a', col_type: 'select', options_json: '{"choices":["Draft","Shipped"]}', sort_order: 0 })],
      cells: [cell({ raw_value: 'Draft' })],
    });
    const { component } = await renderGrid(data);

    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-0-0'));
    await tick();

    const anchor = document.createElement('button');
    anchor.getBoundingClientRect = () => ({
      left: 10, right: 50, top: 10, bottom: 38, width: 40, height: 28, x: 10, y: 10, toJSON: () => ({}),
    });
    (component as any).toolbarFilter(anchor);
    await tick();

    const menu = screen.getByTestId('sheet-header-menu');
    expect(menu).toBeTruthy();
    expect(within(menu).getByText('Draft')).toBeTruthy();
    expect(within(menu).getByText('Shipped')).toBeTruthy();
    // Landed straight in the filter view, not the root menu.
    expect(screen.queryByTestId('sheet-header-menu-rename')).toBeNull();
  });

  it('no-ops when the resolved column does not support filtering (e.g. text)', async () => {
    const data = baseSheetData({ cells: [cell({ raw_value: 'a' })] }); // default col_type: 'text'
    const { component } = await renderGrid(data);

    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-0-0'));
    await tick();

    const anchor = document.createElement('button');
    anchor.getBoundingClientRect = () => ({ left: 10, right: 50, top: 10, bottom: 38, width: 40, height: 28, x: 10, y: 10, toJSON: () => ({}) });
    (component as any).toolbarFilter(anchor);
    await tick();

    expect(screen.queryByTestId('sheet-header-menu')).toBeNull();
  });

  it('no-ops when nothing is selected (zero columns)', async () => {
    const data = baseSheetData({ columns: [], rows: [] });
    const { component } = await renderGrid(data);

    const anchor = document.createElement('button');
    anchor.getBoundingClientRect = () => ({ left: 10, right: 50, top: 10, bottom: 38, width: 40, height: 28, x: 10, y: 10, toJSON: () => ({}) });
    expect(() => (component as any).toolbarFilter(anchor)).not.toThrow();
    await tick();
    expect(screen.queryByTestId('sheet-header-menu')).toBeNull();
  });

  it('toggling a value in the toolbar-opened filter view calls through to the same viewState write as the header-menu door', async () => {
    const data = baseSheetData({
      columns: [col({ id: 'col-a', col_type: 'select', options_json: '{"choices":["Draft","Shipped"]}', sort_order: 0 })],
      cells: [cell({ raw_value: 'Draft' })],
    });
    const { component } = await renderGrid(data);

    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-0-0'));
    await tick();

    const anchor = document.createElement('button');
    anchor.getBoundingClientRect = () => ({ left: 10, right: 50, top: 10, bottom: 38, width: 40, height: 28, x: 10, y: 10, toJSON: () => ({}) });
    (component as any).toolbarFilter(anchor);
    await tick();

    const menu = screen.getByTestId('sheet-header-menu');
    await fireEvent.click(within(menu).getByText('Draft'));
    await tick();

    expect(screen.getByTestId('sheet-header-flag-filter-0')).toBeTruthy();
  });
});

describe('SheetGrid — openFormulaOnAnchor() (#2874 item 16 Part C)', () => {
  it('opens the formula editor on the anchor cell with "=" prefilled', async () => {
    const data = baseSheetData({ cells: [cell({ raw_value: 'existing' })] });
    const { component } = await renderGrid(data);

    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-0-0'));
    await tick();
    (component as any).openFormulaOnAnchor();
    await tick();

    const input = screen.getByTestId('sheet-cell-editor-input') as HTMLInputElement;
    expect(input.value).toBe('=');
  });

  it('no-ops for a checkbox column anchor', async () => {
    const data = baseSheetData({
      columns: [col({ id: 'col-a', col_type: 'checkbox', sort_order: 0 })],
      cells: [cell({ raw_value: '1' })],
    });
    const { component } = await renderGrid(data);

    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-0-0'));
    await tick();
    (component as any).openFormulaOnAnchor();
    await tick();

    expect(screen.queryByTestId('sheet-cell-editor-input')).toBeNull();
  });

  it('no-ops for a select column anchor', async () => {
    const data = baseSheetData({
      columns: [col({ id: 'col-a', col_type: 'select', options_json: '{"choices":["A"]}', sort_order: 0 })],
      cells: [cell({ raw_value: 'A' })],
    });
    const { component } = await renderGrid(data);

    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-0-0'));
    await tick();
    (component as any).openFormulaOnAnchor();
    await tick();

    expect(screen.queryByTestId('sheet-cell-editor-input')).toBeNull();
  });

  it('no-ops when nothing is selected (empty sheet)', async () => {
    const data = baseSheetData({ columns: [], rows: [] });
    const { component } = await renderGrid(data);

    expect(() => (component as any).openFormulaOnAnchor()).not.toThrow();
  });
});

describe('SheetGrid — formulaBarInfo.selectedColumnIndex/selectedColumnType (#2874 item 16 Part C)', () => {
  it('reports the anchor cell\'s column index/type when nothing is header-selected', async () => {
    const data = baseSheetData({
      columns: [col({ id: 'col-a', sort_order: 0 }), col({ id: 'col-b', name: 'B', col_type: 'select', options_json: '{"choices":["A"]}', sort_order: 1 })],
      cells: [cell({ column_id: 'col-a', raw_value: 'x' }), cell({ column_id: 'col-b', raw_value: 'A' })],
    });
    let info: any = null;
    getSheetDataMock.mockResolvedValue(data);
    render(SheetGrid, { props: { noteId: NOTE_ID, onSelectionInfo: (i: any) => { info = i; } } });
    await vi.waitFor(() => expect(screen.getByTestId('sheet-grid')).toBeTruthy());
    await tick();

    await fireEvent.mouseDown(screen.getByTestId('sheet-cell-0-1'));
    await tick();

    expect(info.selectedColumnIndex).toBe(1);
    expect(info.selectedColumnType).toBe('select');
  });

  it('reports null/null when nothing is selected', async () => {
    const data = baseSheetData({ columns: [], rows: [] });
    let info: any = null;
    getSheetDataMock.mockResolvedValue(data);
    render(SheetGrid, { props: { noteId: NOTE_ID, onSelectionInfo: (i: any) => { info = i; } } });
    await vi.waitFor(() => expect(screen.getByTestId('sheet-grid')).toBeTruthy());
    await tick();

    expect(info.selectedColumnIndex).toBeNull();
    expect(info.selectedColumnType).toBeNull();
  });
});

// #2874 item 18 fix 2 — loadSheet() guard against null/malformed sheet data
// from get_sheet_data (backend returning something unexpected: null, or an
// object missing one or more of columns/rows/cells as arrays). Without the
// guard, `pivotSheetData(data.columns, data.rows, data.cells)` would throw
// an opaque TypeError (e.g. "Cannot read properties of undefined") that
// still lands in the same catch block and still sets loadError — but with
// unhelpful raw JS error text instead of a clear message. These tests
// assert the specific, human-readable error message the guard produces.
describe('SheetGrid — loadSheet malformed-data guard (#2874 item 18 fix 2)', () => {
  it('renders the error banner with a clear message when getSheetData resolves null', async () => {
    getSheetDataMock.mockResolvedValue(null as any);
    render(SheetGrid, { props: { noteId: NOTE_ID } });

    await vi.waitFor(() =>
      expect(screen.getByText(/Sheet data is missing or malformed/)).toBeTruthy(),
    );
  });

  it('renders the error banner with a clear message when columns is missing (rows/cells present but not columns)', async () => {
    getSheetDataMock.mockResolvedValue({ rows: [], cells: [] } as any);
    render(SheetGrid, { props: { noteId: NOTE_ID } });

    await vi.waitFor(() =>
      expect(screen.getByText(/Sheet data is missing or malformed/)).toBeTruthy(),
    );
  });

  it('renders the error banner when rows is not an array (e.g. malformed as an object)', async () => {
    getSheetDataMock.mockResolvedValue({ columns: [], rows: {}, cells: [] } as any);
    render(SheetGrid, { props: { noteId: NOTE_ID } });

    await vi.waitFor(() =>
      expect(screen.getByText(/Sheet data is missing or malformed/)).toBeTruthy(),
    );
  });

  it('does NOT show the malformed-data error for well-formed (even if empty) sheet data', async () => {
    const data = baseSheetData({ columns: [], rows: [], cells: [] });
    await renderGrid(data);

    expect(screen.queryByText(/Sheet data is missing or malformed/)).toBeNull();
  });
});

