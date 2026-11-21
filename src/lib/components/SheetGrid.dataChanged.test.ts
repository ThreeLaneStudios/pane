/**
 * SheetGrid.dataChanged.test.ts — #2972 stage 2 (`docs/spec/data-refresh.md`
 * §34.5/§34.10).
 *
 * `SheetGrid.svelte` had zero refresh listeners before this stage (verified
 * at spec time: no `pane-data-changed`/`pane-note-changed`/`listen(` hits in
 * the file) — the exact gap both the scope doc and the #2970 `mutating_tools`
 * stopgap comment named as "NOT a live-UI-refresh fix": an agent editing an
 * open sheet never showed up until the user navigated away and back.
 *
 * This test mounts the REAL SheetGrid.svelte and simulates the backend
 * emitting the new, entity-identified `pane-data-changed` event on the
 * Tauri channel (via `simulateTauriEvent`, since backend emits are
 * invisible to window.addEventListener, #2215), scoped to the grid's own
 * `noteId`. It asserts the grid re-reads and re-renders the sheet's data —
 * via `refetchSheetData()` (cell/row/column diff), not a full remount — and
 * that an event for a DIFFERENT note_id is ignored.
 */

import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen } from '@testing-library/svelte/svelte5';
import SheetGrid from './SheetGrid.svelte';
import type { SheetColumn, SheetRow, SheetCell, SheetData } from '$lib/types';
import { simulateTauriEvent } from '../../../vitest-setup';
import { EVENT_DATA_CHANGED, ENTITY_SHEET, REASON_EDITED, dispatchDataChanged } from '$lib/refreshEvent';

const win = globalThis.window as any;
if (typeof win.addEventListener !== 'function') {
  win.addEventListener = globalThis.addEventListener?.bind(globalThis) ?? (() => {});
}
if (typeof win.removeEventListener !== 'function') {
  win.removeEventListener = globalThis.removeEventListener?.bind(globalThis) ?? (() => {});
}
// #2972 stage 3 (item 4): unconditional — vitest-setup.ts's fake `window`
// already defines a `dispatchEvent` property (a no-op stub); a
// conditional guard would see that as "already present" and never wire it
// to the real jsdom EventTarget.
win.dispatchEvent = globalThis.dispatchEvent.bind(globalThis);

const NOTE_ID = 'note-sheetgrid-datachanged-1';
const OTHER_NOTE_ID = 'note-sheetgrid-other-1';

function col(overrides: Partial<SheetColumn> = {}): SheetColumn {
  return {
    id: overrides.id ?? 'col-a',
    note_id: NOTE_ID,
    name: overrides.name ?? 'Column A',
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
    columns: overrides.columns ?? [col()],
    rows: overrides.rows ?? [row()],
    cells: overrides.cells ?? [cell({ raw_value: 'original' })],
    formula_count: overrides.formula_count ?? 0,
    formulas_paused: overrides.formulas_paused ?? false,
  };
}

const { getSheetDataMock } = vi.hoisted(() => ({
  getSheetDataMock: vi.fn(),
}));

vi.mock('$lib/api', () => ({
  getSheetData: getSheetDataMock,
}));

beforeEach(() => {
  getSheetDataMock.mockReset();
});

describe('SheetGrid pane-data-changed listener (#2972 stage 2, §34.5)', () => {
  it('re-fetches and re-renders sheet data when the event fires for its own note_id', async () => {
    getSheetDataMock.mockResolvedValueOnce(baseSheetData());

    render(SheetGrid, { props: { noteId: NOTE_ID } });

    await vi.waitFor(() => {
      expect(screen.getByTestId('sheet-cell-0-0').textContent).toContain('original');
    });
    expect(getSheetDataMock).toHaveBeenCalledTimes(1);

    getSheetDataMock.mockResolvedValueOnce(baseSheetData({
      cells: [cell({ raw_value: 'changed-by-agent' })],
    }));

    simulateTauriEvent(EVENT_DATA_CHANGED, {
      entity_id: NOTE_ID,
      entity_type: ENTITY_SHEET,
      reason: REASON_EDITED,
      origin_tag: 'test-origin-tag',
    });

    await vi.waitFor(() => {
      expect(getSheetDataMock).toHaveBeenCalledTimes(2);
    });
    await vi.waitFor(() => {
      expect(screen.getByTestId('sheet-cell-0-0').textContent).toContain('changed-by-agent');
    });
  });

  it('re-fetches the SAME way via the APP channel (DOM CustomEvent), not just the backend Tauri channel (#2972 stage 3, item 4)', async () => {
    getSheetDataMock.mockResolvedValueOnce(baseSheetData());

    render(SheetGrid, { props: { noteId: NOTE_ID } });

    await vi.waitFor(() => {
      expect(screen.getByTestId('sheet-cell-0-0').textContent).toContain('original');
    });
    expect(getSheetDataMock).toHaveBeenCalledTimes(1);

    getSheetDataMock.mockResolvedValueOnce(baseSheetData({
      cells: [cell({ raw_value: 'changed-via-app-channel' })],
    }));

    dispatchDataChanged(NOTE_ID, ENTITY_SHEET, REASON_EDITED, 'test-origin-tag-app');

    await vi.waitFor(() => {
      expect(getSheetDataMock).toHaveBeenCalledTimes(2);
    });
    await vi.waitFor(() => {
      expect(screen.getByTestId('sheet-cell-0-0').textContent).toContain('changed-via-app-channel');
    });
  });

  it('does NOT re-fetch when the event is for a different note_id', async () => {
    getSheetDataMock.mockResolvedValueOnce(baseSheetData());

    render(SheetGrid, { props: { noteId: NOTE_ID } });

    await vi.waitFor(() => {
      expect(screen.getByTestId('sheet-cell-0-0').textContent).toContain('original');
    });
    expect(getSheetDataMock).toHaveBeenCalledTimes(1);

    simulateTauriEvent(EVENT_DATA_CHANGED, {
      entity_id: OTHER_NOTE_ID,
      entity_type: ENTITY_SHEET,
      reason: REASON_EDITED,
      origin_tag: 'test-origin-tag',
    });

    // Give the (incorrectly-firing, if the bug regresses) handler a tick.
    await new Promise(r => setTimeout(r, 30));
    expect(getSheetDataMock).toHaveBeenCalledTimes(1);
  });
});
