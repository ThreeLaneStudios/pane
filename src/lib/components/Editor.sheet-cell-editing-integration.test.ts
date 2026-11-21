// Editor.sheet-cell-editing-integration.test.ts — #2874 P0-1 regression
// coverage. FormulaBar's `$effect` (lines 97-105 pre-fix) unconditionally
// called `.focus()` on its own input whenever `editing` was truthy. Every
// keystroke typed into the in-cell editor re-renders FormulaBar (via
// SheetGrid's onSelectionInfo -> Editor's `formulaBarInfo` $state ->
// FormulaBar re-render chain), which re-ran that effect and stole focus
// back from the in-cell editor — the blur handler then committed and
// closed the editor, so only the FIRST typed character ever landed.
//
// These tests mount the REAL Editor.svelte, which mounts the REAL
// FormulaBar.svelte + SheetGrid.svelte (no mocks for those three) — only
// `$lib/api` is mocked, same wholesale-mock pattern as SheetGrid.test.ts,
// because that's the only module boundary these components actually
// cross. This is the level the bug lived at: two sibling components both
// reacting to the SAME shared `editing` prop in the SAME reactive flush,
// which isolated unit tests of either component in isolation cannot
// reproduce.

import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/svelte/svelte5';
import { tick } from 'svelte';
import Editor from './Editor.svelte';
import type { SheetColumn, SheetRow, SheetCell, SheetData, NoteMeta } from '$lib/types';

// vitest-setup.ts replaces `globalThis.window` with a plain mock object
// lacking addEventListener/removeEventListener. SheetGrid.svelte attaches
// `window.addEventListener('mousemove'/'mouseup', ...)` on mount/resize-
// drag and removes them again in onDestroy — bind the mock's add/remove
// EventListener through to the REAL jsdom global (still reachable via
// `globalThis.addEventListener`, since vitest-setup only replaces
// `globalThis.window`, not `globalThis` itself), matching SheetGrid.test.ts's
// own setup exactly.
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

function makeNoteMeta(): NoteMeta {
  return {
    id: NOTE_ID,
    title: 'Sheet',
    notebook: '',
    slug: 'sheet',
    created_at: '2026-01-01T00:00:00Z',
    updated_at: '2026-01-01T00:00:00Z',
    preview: '',
    note_type: 'sheet',
  };
}

const {
  getSheetDataMock,
  updateSheetCellMock,
  createSheetRowMock,
  createSheetColumnMock,
  updateSheetColumnMock,
  updateSheetRowMock,
  deleteSheetColumnMock,
  listTasksByNoteMock,
  getBacklinksMock,
} = vi.hoisted(() => ({
  getSheetDataMock: vi.fn(),
  updateSheetCellMock: vi.fn(),
  createSheetRowMock: vi.fn(),
  createSheetColumnMock: vi.fn(),
  updateSheetColumnMock: vi.fn(),
  updateSheetRowMock: vi.fn(),
  deleteSheetColumnMock: vi.fn(),
  listTasksByNoteMock: vi.fn(),
  getBacklinksMock: vi.fn(),
}));

vi.mock('$lib/api', () => ({
  getSheetData: getSheetDataMock,
  updateSheetCell: updateSheetCellMock,
  createSheetRow: createSheetRowMock,
  createSheetColumn: createSheetColumnMock,
  updateSheetColumn: updateSheetColumnMock,
  updateSheetRow: updateSheetRowMock,
  deleteSheetColumn: deleteSheetColumnMock,
  // Editor.svelte's onMount calls these two regardless of note type.
  listTasksByNote: listTasksByNoteMock,
  getBacklinks: getBacklinksMock,
}));

beforeEach(() => {
  getSheetDataMock.mockReset();
  updateSheetCellMock.mockReset();
  createSheetRowMock.mockReset();
  createSheetColumnMock.mockReset();
  updateSheetColumnMock.mockReset();
  updateSheetRowMock.mockReset();
  deleteSheetColumnMock.mockReset();
  listTasksByNoteMock.mockReset();
  getBacklinksMock.mockReset();
  listTasksByNoteMock.mockResolvedValue([]);
  getBacklinksMock.mockResolvedValue([]);
});

describe('Editor + FormulaBar + SheetGrid — #2874 P0-1 focus-steal regression', () => {
  it('Case 1: double-click then type 3 chars — editor stays open with all 3 chars', async () => {
    const data = baseSheetData({ cells: [cell({ raw_value: '' })] });
    getSheetDataMock.mockResolvedValue(data);
    render(Editor, { props: { noteId: NOTE_ID, noteMeta: makeNoteMeta(), slug: 'sheet' } });
    await vi.waitFor(() => expect(screen.getByTestId('sheet-grid')).toBeTruthy());
    await tick();
    await fireEvent.dblClick(screen.getByTestId('sheet-cell-0-0'));
    await tick();
    const input = screen.getByTestId('sheet-cell-editor-input') as HTMLInputElement;
    await fireEvent.input(input, { target: { value: 'a' } });
    await tick();
    await fireEvent.input(input, { target: { value: 'ab' } });
    await tick();
    await fireEvent.input(input, { target: { value: 'abc' } });
    await tick();
    const stillOpenInput = screen.getByTestId('sheet-cell-editor-input') as HTMLInputElement;
    expect(stillOpenInput.value).toBe('abc');
  });

  it('Case 2: single-click then type (type-to-edit) — editor opens and stays open with all 3 chars', async () => {
    const data = baseSheetData({ cells: [cell({ raw_value: '' })] });
    getSheetDataMock.mockResolvedValue(data);
    render(Editor, { props: { noteId: NOTE_ID, noteMeta: makeNoteMeta(), slug: 'sheet' } });
    await vi.waitFor(() => expect(screen.getByTestId('sheet-grid')).toBeTruthy());
    await tick();
    const cellEl = screen.getByTestId('sheet-cell-0-0');
    await fireEvent.mouseDown(cellEl);
    await fireEvent.click(cellEl);
    await tick();
    expect(screen.queryByTestId('sheet-cell-editor-input')).toBeNull();
    const gridWrap = screen.getByTestId('sheet-grid-wrap');
    await fireEvent.keyDown(gridWrap, { key: 'a' });
    await tick();
    const input = screen.getByTestId('sheet-cell-editor-input') as HTMLInputElement;
    await fireEvent.input(input, { target: { value: 'a' } });
    await tick();
    await fireEvent.input(input, { target: { value: 'ab' } });
    await tick();
    await fireEvent.input(input, { target: { value: 'abc' } });
    await tick();
    const stillOpenInput = screen.getByTestId('sheet-cell-editor-input') as HTMLInputElement;
    expect(stillOpenInput.value).toBe('abc');
  });

  it('Case 3: reopen after Enter — double-click, type "abc", Enter, double-click same cell again, type "xyz"', async () => {
    const data = baseSheetData({ cells: [cell({ raw_value: '' })] });
    getSheetDataMock.mockResolvedValue(data);
    updateSheetCellMock.mockResolvedValue(cell({ raw_value: 'abc' }));
    render(Editor, { props: { noteId: NOTE_ID, noteMeta: makeNoteMeta(), slug: 'sheet' } });
    await vi.waitFor(() => expect(screen.getByTestId('sheet-grid')).toBeTruthy());
    await tick();
    await fireEvent.dblClick(screen.getByTestId('sheet-cell-0-0'));
    await tick();
    let input = screen.getByTestId('sheet-cell-editor-input') as HTMLInputElement;
    await fireEvent.input(input, { target: { value: 'a' } });
    await tick();
    await fireEvent.input(input, { target: { value: 'ab' } });
    await tick();
    await fireEvent.input(input, { target: { value: 'abc' } });
    await tick();
    await fireEvent.keyDown(input, { key: 'Enter' });
    await tick();
    expect(screen.queryByTestId('sheet-cell-editor-input')).toBeNull();
    await fireEvent.dblClick(screen.getByTestId('sheet-cell-0-0'));
    await tick();
    input = screen.getByTestId('sheet-cell-editor-input') as HTMLInputElement;
    await fireEvent.input(input, { target: { value: 'x' } });
    await tick();
    await fireEvent.input(input, { target: { value: 'xy' } });
    await tick();
    await fireEvent.input(input, { target: { value: 'xyz' } });
    await tick();
    const stillOpenInput = screen.getByTestId('sheet-cell-editor-input') as HTMLInputElement;
    expect(stillOpenInput.value).toBe('xyz');
  });

  it('Case 4: click idle bar, type 3 chars, Enter — cell shows the committed value', async () => {
    const data = baseSheetData({ cells: [cell({ raw_value: '' })] });
    getSheetDataMock.mockResolvedValue(data);
    updateSheetCellMock.mockResolvedValue(cell({ raw_value: 'xyz' }));
    render(Editor, { props: { noteId: NOTE_ID, noteMeta: makeNoteMeta(), slug: 'sheet' } });
    await vi.waitFor(() => expect(screen.getByTestId('sheet-grid')).toBeTruthy());
    await tick();

    const cellEl = screen.getByTestId('sheet-cell-0-0');
    await fireEvent.mouseDown(cellEl);
    await fireEvent.click(cellEl);
    await tick();
    expect(screen.queryByTestId('sheet-cell-editor-input')).toBeNull();

    // jsdom doesn't replicate native click-focus on tabindex=0 elements,
    // so simulate it explicitly before the click
    const idleBarSpan = screen.getByText('Empty cell');
    idleBarSpan.focus();
    await fireEvent.click(idleBarSpan);
    await tick();

    const barInput = screen.getByTestId('sheet-formula-bar-input') as HTMLInputElement;
    expect(document.activeElement).toBe(barInput);

    await fireEvent.input(barInput, { target: { value: 'x' } });
    await tick();
    await fireEvent.input(barInput, { target: { value: 'xy' } });
    await tick();
    await fireEvent.input(barInput, { target: { value: 'xyz' } });
    await tick();
    expect(barInput.value).toBe('xyz');

    await fireEvent.keyDown(barInput, { key: 'Enter' });
    await tick();

    expect(screen.queryByTestId('sheet-cell-editor-input')).toBeNull();
    await vi.waitFor(() => {
      expect(screen.getByTestId('sheet-cell-0-0').textContent).toContain('xyz');
    });
  });
});
