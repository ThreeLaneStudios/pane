// SheetGrid.focus-after-commit.test.ts — component tests for item 36
// (#2874): the grid wrapper (`data-testid="sheet-grid-wrap"`, bound to
// `gridEl`) must reclaim DOM focus after a keyboard-driven editor-closing
// path (Enter/Tab/Shift-Tab commit, Escape cancel, select-column commit,
// select-column cancel) unmounts the currently-focused editor. Per the DOM
// spec, removing the focused element from the document drops focus to
// `<body>`, not to any focusable ancestor — none of `refocusGrid()`'s four
// call sites existed before this fix, so `document.activeElement` fell to
// `document.body` and the very next keystroke silently did nothing.
//
// Per Matt's amendment (overriding the original scope doc), `refocusGrid()`
// must NOT run for `cause === 'blur'` — a blur means the user clicked
// somewhere else on purpose, and focus should stay wherever they clicked.
// Test 5 below is this file's regression guard for that carve-out.

import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/svelte/svelte5';
import { tick } from 'svelte';
import SheetGrid from './SheetGrid.svelte';
import type { SheetColumn, SheetRow, SheetCell, SheetData } from '$lib/types';

// vitest-setup.ts replaces `globalThis.window` with a plain mock object
// lacking addEventListener/removeEventListener. SheetGrid.svelte's resize-
// grip effect and its onDestroy teardown both call window.addEventListener/
// removeEventListener directly, so — matching SheetGrid.test.ts's own
// setup — bind the mock's addEventListener/removeEventListener through to
// the real jsdom global (still reachable via `globalThis.addEventListener`,
// since vitest-setup only replaces `globalThis.window`, not `globalThis`
// itself).
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
  createSheetRowMock: vi.fn(),
  createSheetColumnMock: vi.fn(),
  createSheetRowsBatchMock: vi.fn(),
  createSheetColumnsBatchMock: vi.fn(),
  updateSheetColumnMock: vi.fn(),
  updateSheetRowMock: vi.fn(),
  deleteSheetColumnMock: vi.fn(),
}));

vi.mock('$lib/api', () => ({
  getSheetData: getSheetDataMock,
  updateSheetCell: updateSheetCellMock,
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
  createSheetRowMock.mockReset();
  createSheetColumnMock.mockReset();
  createSheetRowsBatchMock.mockReset();
  createSheetColumnsBatchMock.mockReset();
  updateSheetColumnMock.mockReset();
  updateSheetRowMock.mockReset();
  deleteSheetColumnMock.mockReset();
  updateSheetCellMock.mockResolvedValue(cell({ raw_value: 'new value' }));
});

async function renderGrid(data: SheetData) {
  getSheetDataMock.mockResolvedValue(data);
  const result = render(SheetGrid, { props: { noteId: NOTE_ID } });
  await vi.waitFor(() => expect(screen.getByTestId('sheet-grid')).toBeTruthy());
  await tick();
  return result;
}

describe('SheetGrid — refocusGrid() after keyboard editor close (item 36, #2874)', () => {
  it('grid receives focus after Enter commit', async () => {
    const data = baseSheetData({ cells: [cell({ raw_value: 'old' })] });
    await renderGrid(data);

    await fireEvent.dblClick(screen.getByTestId('sheet-cell-0-0'));
    await tick();
    const input = screen.getByTestId('sheet-cell-editor-input') as HTMLInputElement;
    await fireEvent.input(input, { target: { value: 'new value' } });
    await fireEvent.keyDown(input, { key: 'Enter' });
    await tick();

    expect(screen.queryByTestId('sheet-cell-editor-input')).toBeNull();
    expect(document.activeElement).toBe(screen.getByTestId('sheet-grid-wrap'));
  });

  it('grid receives focus after Escape cancel', async () => {
    const data = baseSheetData({ cells: [cell({ raw_value: 'original' })] });
    await renderGrid(data);

    await fireEvent.dblClick(screen.getByTestId('sheet-cell-0-0'));
    await tick();
    const input = screen.getByTestId('sheet-cell-editor-input') as HTMLInputElement;
    await fireEvent.keyDown(input, { key: 'Escape' });
    await tick();

    expect(screen.queryByTestId('sheet-cell-editor-input')).toBeNull();
    expect(document.activeElement).toBe(screen.getByTestId('sheet-grid-wrap'));
  });

  it('grid receives focus after select commit', async () => {
    const data = baseSheetData({
      columns: [col({ id: 'col-a', col_type: 'select', options_json: '{"choices":["Draft","Shipped"]}', sort_order: 0 })],
      cells: [cell({ raw_value: 'Draft' })],
    });
    await renderGrid(data);

    await fireEvent.dblClick(screen.getByTestId('sheet-cell-0-0'));
    await tick();
    const choices = screen.getAllByTestId('sheet-select-choice');
    await fireEvent.click(choices[1]); // "Shipped"
    await tick();

    expect(screen.queryByTestId('sheet-cell-editor')).toBeNull();
    expect(document.activeElement).toBe(screen.getByTestId('sheet-grid-wrap'));
  });

  it('grid receives focus after select cancel', async () => {
    const data = baseSheetData({
      columns: [col({ id: 'col-a', col_type: 'select', options_json: '{"choices":["Draft","Shipped"]}', sort_order: 0 })],
      cells: [cell({ raw_value: 'Draft' })],
    });
    await renderGrid(data);

    await fireEvent.dblClick(screen.getByTestId('sheet-cell-0-0'));
    await tick();
    const popover = screen.getByTestId('sheet-cell-editor');
    await fireEvent.keyDown(popover, { key: 'Escape' });
    await tick();

    expect(screen.queryByTestId('sheet-cell-editor')).toBeNull();
    expect(document.activeElement).toBe(screen.getByTestId('sheet-grid-wrap'));
  });

  it('focus stays on target element after blur commit', async () => {
    const data = baseSheetData({ cells: [cell({ raw_value: 'old' })] });
    await renderGrid(data);

    await fireEvent.dblClick(screen.getByTestId('sheet-cell-0-0'));
    await tick();
    const input = screen.getByTestId('sheet-cell-editor-input') as HTMLInputElement;
    expect(document.activeElement).toBe(input);

    const outsideButton = document.createElement('button');
    outsideButton.textContent = 'Somewhere else';
    document.body.appendChild(outsideButton);

    outsideButton.focus();
    await fireEvent.blur(input);
    await tick();

    expect(screen.queryByTestId('sheet-cell-editor-input')).toBeNull();
    expect(document.activeElement).toBe(outsideButton);
    expect(document.activeElement).not.toBe(screen.getByTestId('sheet-grid-wrap'));

    document.body.removeChild(outsideButton);
  });
});
