// ShHeaderMenu.test.ts — component tests for the column header context
// menu (#2339 PR 11, §33.3, scope report
// `docs/audits/2026-09-04-sheets-pr11-header-menu-scope.md`).

import { describe, it, expect, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/svelte/svelte5';
import ShHeaderMenu from './ShHeaderMenu.svelte';
import type { SheetColumn } from '$lib/types';

// vitest-setup.ts replaces `globalThis.window` with a plain mock object
// lacking addEventListener/removeEventListener — this component uses
// <svelte:window onclick> for outside-click close, same patch as every
// other Sheets popover's own tests (SheetCellEditorSelect.test.ts et al.).
if (typeof (globalThis.window as any).addEventListener !== 'function') {
  (globalThis.window as any).addEventListener = () => {};
  (globalThis.window as any).removeEventListener = () => {};
}
if (typeof (globalThis.window as any).innerWidth !== 'number') {
  (globalThis.window as any).innerWidth = 1200;
}

function col(overrides: Partial<SheetColumn> = {}): SheetColumn {
  return {
    id: overrides.id ?? 'col-a',
    note_id: 'note-1',
    name: overrides.name ?? 'Status',
    col_type: overrides.col_type ?? 'text',
    options_json: overrides.options_json ?? '{}',
    width: overrides.width ?? 148,
    sort_order: overrides.sort_order ?? 0,
    created_at: '2026-01-01T00:00:00Z',
    updated_at: '2026-01-01T00:00:00Z',
  };
}

function makeAnchor(left = 10): HTMLElement {
  const el = document.createElement('div');
  el.getBoundingClientRect = () => ({
    left,
    right: left + 148,
    top: 0,
    bottom: 38,
    width: 148,
    height: 38,
    x: left,
    y: 0,
    toJSON: () => ({}),
  });
  return el;
}

function baseProps(overrides: Partial<Record<string, unknown>> = {}) {
  return {
    column: col(),
    anchorEl: makeAnchor(),
    filterValues: null,
    onRename: vi.fn(),
    onTypeChange: vi.fn(),
    onInsertColumn: vi.fn(),
    onSortAsc: vi.fn(),
    onSortDesc: vi.fn(),
    onFilterChange: vi.fn(),
    onDelete: vi.fn(),
    onClose: vi.fn(),
    onOptionsChange: vi.fn(),
    ...overrides,
  };
}

describe('ShHeaderMenu — root view', () => {
  it('renders all six verb rows', () => {
    render(ShHeaderMenu, { props: baseProps() });
    expect(screen.getByTestId('sheet-header-menu-rename')).toBeTruthy();
    expect(screen.getByTestId('sheet-header-menu-type')).toBeTruthy();
    expect(screen.getByTestId('sheet-header-menu-insert-column')).toBeTruthy();
    expect(screen.getByTestId('sheet-header-menu-sort-asc')).toBeTruthy();
    expect(screen.getByTestId('sheet-header-menu-sort-desc')).toBeTruthy();
    expect(screen.getByTestId('sheet-header-menu-filter')).toBeTruthy();
    expect(screen.getByTestId('sheet-header-menu-delete')).toBeTruthy();
  });

  // #2874 item 31: "Insert column left" moved here from
  // `ShCellContextMenu.svelte` — grouped with Rename/Type above the
  // divider that precedes Sort ascending.
  it('renders Insert column left above Sort ascending, in the Rename/Type group', () => {
    render(ShHeaderMenu, { props: baseProps() });
    const menu = screen.getByTestId('sheet-header-menu');
    const rowIds = Array.from(menu.querySelectorAll('[data-testid]')).map((el) => el.getAttribute('data-testid'));
    const insertIdx = rowIds.indexOf('sheet-header-menu-insert-column');
    const sortAscIdx = rowIds.indexOf('sheet-header-menu-sort-asc');
    const renameIdx = rowIds.indexOf('sheet-header-menu-rename');
    const typeIdx = rowIds.indexOf('sheet-header-menu-type');
    expect(insertIdx).toBeGreaterThan(-1);
    expect(insertIdx).toBeGreaterThan(renameIdx);
    expect(insertIdx).toBeGreaterThan(typeIdx);
    expect(insertIdx).toBeLessThan(sortAscIdx);
    expect(screen.getByTestId('sheet-header-menu-insert-column').textContent).toContain('Insert column left');
  });

  it('Insert column left calls onInsertColumn and closes', async () => {
    const onInsertColumn = vi.fn();
    const onClose = vi.fn();
    render(ShHeaderMenu, { props: baseProps({ onInsertColumn, onClose }) });
    await fireEvent.click(screen.getByTestId('sheet-header-menu-insert-column'));
    expect(onInsertColumn).toHaveBeenCalledOnce();
    expect(onClose).toHaveBeenCalledOnce();
  });

  it('reuses the exact insert-column SVG markup used by ShCellContextMenu', () => {
    render(ShHeaderMenu, { props: baseProps() });
    const ico = screen.getByTestId('sheet-header-menu-insert-column').querySelector('.sh-pop__ico svg');
    expect(ico).toBeTruthy();
    expect(ico?.getAttribute('viewBox')).toBe('0 0 24 24');
    const rect = ico?.querySelector('rect');
    expect(rect?.getAttribute('x')).toBe('4');
    expect(rect?.getAttribute('y')).toBe('3');
    expect(rect?.getAttribute('width')).toBe('6');
    expect(rect?.getAttribute('height')).toBe('18');
    expect(rect?.getAttribute('rx')).toBe('1.6');
    const path = ico?.querySelector('path');
    expect(path?.getAttribute('d')).toBe('M16 3v18M13.5 12h5');
  });

  it('closes and calls onRename when the Rename row is clicked (ruling 1 — no embedded input)', async () => {
    const onRename = vi.fn();
    const onClose = vi.fn();
    render(ShHeaderMenu, { props: baseProps({ onRename, onClose }) });
    await fireEvent.click(screen.getByTestId('sheet-header-menu-rename'));
    expect(onRename).toHaveBeenCalledOnce();
    expect(onClose).toHaveBeenCalledOnce();
    // No text input anywhere in this component — rename is handed off,
    // never rendered here.
    expect(screen.queryByRole('textbox')).toBeNull();
  });

  it('Sort ascending calls onSortAsc and closes', async () => {
    const onSortAsc = vi.fn();
    const onClose = vi.fn();
    render(ShHeaderMenu, { props: baseProps({ onSortAsc, onClose }) });
    await fireEvent.click(screen.getByTestId('sheet-header-menu-sort-asc'));
    expect(onSortAsc).toHaveBeenCalledOnce();
    expect(onClose).toHaveBeenCalledOnce();
  });

  it('Sort descending calls onSortDesc and closes', async () => {
    const onSortDesc = vi.fn();
    const onClose = vi.fn();
    render(ShHeaderMenu, { props: baseProps({ onSortDesc, onClose }) });
    await fireEvent.click(screen.getByTestId('sheet-header-menu-sort-desc'));
    expect(onSortDesc).toHaveBeenCalledOnce();
    expect(onClose).toHaveBeenCalledOnce();
  });

  it('Delete row is styled sh-pop__danger and calls onDelete + closes', async () => {
    const onDelete = vi.fn();
    const onClose = vi.fn();
    render(ShHeaderMenu, { props: baseProps({ onDelete, onClose }) });
    const deleteRow = screen.getByTestId('sheet-header-menu-delete');
    expect(deleteRow.className).toContain('sh-pop__danger');
    await fireEvent.click(deleteRow);
    expect(onDelete).toHaveBeenCalledOnce();
    expect(onClose).toHaveBeenCalledOnce();
  });

  // Ruling 3: filter scoped to select/checkbox only.
  it('Filter row is disabled for a text column', () => {
    render(ShHeaderMenu, { props: baseProps({ column: col({ col_type: 'text' }) }) });
    const filterRow = screen.getByTestId('sheet-header-menu-filter') as HTMLButtonElement;
    expect(filterRow.disabled).toBe(true);
  });

  it('Filter row is disabled for number/currency/date columns too', () => {
    for (const colType of ['number', 'currency', 'date']) {
      const { unmount } = render(ShHeaderMenu, { props: baseProps({ column: col({ col_type: colType }) }) });
      const filterRow = screen.getByTestId('sheet-header-menu-filter') as HTMLButtonElement;
      expect(filterRow.disabled).toBe(true);
      unmount();
    }
  });

  it('Filter row is enabled (not disabled) for a select column', () => {
    render(ShHeaderMenu, { props: baseProps({ column: col({ col_type: 'select' }) }) });
    const filterRow = screen.getByTestId('sheet-header-menu-filter') as HTMLButtonElement;
    expect(filterRow.disabled).toBe(false);
  });

  it('Filter row is enabled for a checkbox column', () => {
    render(ShHeaderMenu, { props: baseProps({ column: col({ col_type: 'checkbox' }) }) });
    const filterRow = screen.getByTestId('sheet-header-menu-filter') as HTMLButtonElement;
    expect(filterRow.disabled).toBe(false);
  });

  it('clicking a disabled Filter row does not navigate to the filter view', async () => {
    render(ShHeaderMenu, { props: baseProps({ column: col({ col_type: 'text' }) }) });
    await fireEvent.click(screen.getByTestId('sheet-header-menu-filter'));
    expect(screen.queryByText(/Filter Status/)).toBeNull();
  });

  it('escape closes the menu from the root view', async () => {
    const onClose = vi.fn();
    render(ShHeaderMenu, { props: baseProps({ onClose }) });
    await fireEvent.keyDown(screen.getByTestId('sheet-header-menu'), { key: 'Escape' });
    expect(onClose).toHaveBeenCalledOnce();
  });

  // Outside-click close is wired via <svelte:window onclick>, same pattern
  // as every other Sheets popover (SheetCellEditorSelect.svelte) — not
  // independently unit-testable here since vitest-setup.ts replaces
  // `globalThis.window` with a plain mock whose `addEventListener` is a
  // no-op (so Svelte's own window-click listener never actually attaches
  // in this test environment; the SAME reason no other Sheets popover
  // test in this codebase exercises outside-click via a real DOM event).
  // What IS verified: a click on the popover's own root stopPropagation()s
  // (`onclick={(e) => e.stopPropagation()}` on the root div), which every
  // verb-row-click test above already exercises without triggering a
  // spurious close.
  it('clicking inside the popover does not call onClose (click stopPropagation guard)', async () => {
    const onClose = vi.fn();
    render(ShHeaderMenu, { props: baseProps({ onClose }) });
    await fireEvent.click(screen.getByTestId('sheet-header-menu'));
    expect(onClose).not.toHaveBeenCalled();
  });

  it('boundary-flip positioning applies flip-right class when the anchor is near the right viewport edge', () => {
    (globalThis.window as any).innerWidth = 300;
    render(ShHeaderMenu, { props: baseProps({ anchorEl: makeAnchor(250) }) });
    const menu = screen.getByTestId('sheet-header-menu');
    expect(menu.className).toContain('flip-right');
    (globalThis.window as any).innerWidth = 1200;
  });

  it('does not flip-right when there is enough room to the right', () => {
    (globalThis.window as any).innerWidth = 1200;
    render(ShHeaderMenu, { props: baseProps({ anchorEl: makeAnchor(10) }) });
    const menu = screen.getByTestId('sheet-header-menu');
    expect(menu.className).not.toContain('flip-right');
  });
});

describe('ShHeaderMenu — Edit options… row (#2339 PR 15b-beta)', () => {
  it('is NOT rendered for a text column', () => {
    render(ShHeaderMenu, { props: baseProps({ column: col({ col_type: 'text' }) }) });
    expect(screen.queryByTestId('sheet-header-menu-options')).toBeNull();
  });

  it('is NOT rendered for number/currency/date/checkbox columns', () => {
    for (const colType of ['number', 'currency', 'date', 'checkbox']) {
      const { unmount } = render(ShHeaderMenu, { props: baseProps({ column: col({ col_type: colType }) }) });
      expect(screen.queryByTestId('sheet-header-menu-options')).toBeNull();
      unmount();
    }
  });

  it('IS rendered for a select column, with the live choice count', () => {
    render(ShHeaderMenu, {
      props: baseProps({ column: col({ col_type: 'select', options_json: '{"choices":["A","B","C"]}' }) }),
    });
    const row = screen.getByTestId('sheet-header-menu-options');
    expect(row).toBeTruthy();
    expect(row.textContent).toContain('3');
  });

  it('clicking the row opens the options panel (fourth view-state) instead of closing the menu', async () => {
    const onClose = vi.fn();
    render(ShHeaderMenu, {
      props: baseProps({
        column: col({ col_type: 'select', options_json: '{"choices":["A","B"]}' }),
        onClose,
      }),
    });
    await fireEvent.click(screen.getByTestId('sheet-header-menu-options'));
    expect(onClose).not.toHaveBeenCalled();
    expect(screen.getByTestId('sheet-options-panel')).toBeTruthy();
    expect(screen.queryByTestId('sheet-header-menu-rename')).toBeNull();
  });

  it('the options panel back-chevron returns to the root menu', async () => {
    render(ShHeaderMenu, {
      props: baseProps({ column: col({ col_type: 'select', options_json: '{"choices":["A","B"]}' }) }),
    });
    await fireEvent.click(screen.getByTestId('sheet-header-menu-options'));
    await fireEvent.click(screen.getByTestId('sheet-options-back'));
    expect(screen.getByTestId('sheet-header-menu-rename')).toBeTruthy();
  });

  it('a choices change from the panel calls onOptionsChange with the new array (caller persists it)', async () => {
    const onOptionsChange = vi.fn();
    render(ShHeaderMenu, {
      props: baseProps({
        column: col({ col_type: 'select', options_json: '{"choices":["A","B"]}' }),
        onOptionsChange,
      }),
    });
    await fireEvent.click(screen.getByTestId('sheet-header-menu-options'));
    await fireEvent.click(screen.getByTestId('sheet-options-remove-0'));
    expect(onOptionsChange).toHaveBeenCalledWith(['B']);
  });

  it('escape from the options view steps back to root, not a full close', async () => {
    const onClose = vi.fn();
    render(ShHeaderMenu, {
      props: baseProps({ column: col({ col_type: 'select', options_json: '{"choices":["A"]}' }), onClose }),
    });
    await fireEvent.click(screen.getByTestId('sheet-header-menu-options'));
    await fireEvent.keyDown(screen.getByTestId('sheet-header-menu'), { key: 'Escape' });
    expect(onClose).not.toHaveBeenCalled();
    expect(screen.getByTestId('sheet-header-menu-rename')).toBeTruthy();
  });
});

describe('ShHeaderMenu — type submenu', () => {
  it('navigates to the type view on Type row click', async () => {
    render(ShHeaderMenu, { props: baseProps({ column: col({ col_type: 'number' }) }) });
    await fireEvent.click(screen.getByTestId('sheet-header-menu-type'));
    expect(screen.getByTestId('sheet-header-menu-type-text')).toBeTruthy();
    expect(screen.getByTestId('sheet-header-menu-type-number')).toBeTruthy();
    expect(screen.getByTestId('sheet-header-menu-type-currency')).toBeTruthy();
    expect(screen.getByTestId('sheet-header-menu-type-date')).toBeTruthy();
    expect(screen.getByTestId('sheet-header-menu-type-checkbox')).toBeTruthy();
    expect(screen.getByTestId('sheet-header-menu-type-select')).toBeTruthy();
  });

  it('marks the current type is-on', async () => {
    render(ShHeaderMenu, { props: baseProps({ column: col({ col_type: 'currency' }) }) });
    await fireEvent.click(screen.getByTestId('sheet-header-menu-type'));
    expect(screen.getByTestId('sheet-header-menu-type-currency').className).toContain('is-on');
    expect(screen.getByTestId('sheet-header-menu-type-text').className).not.toContain('is-on');
  });

  it('clicking a different type commits immediately via onTypeChange and closes', async () => {
    const onTypeChange = vi.fn();
    const onClose = vi.fn();
    render(ShHeaderMenu, { props: baseProps({ column: col({ col_type: 'text' }), onTypeChange, onClose }) });
    await fireEvent.click(screen.getByTestId('sheet-header-menu-type'));
    await fireEvent.click(screen.getByTestId('sheet-header-menu-type-number'));
    expect(onTypeChange).toHaveBeenCalledWith('number');
    expect(onClose).toHaveBeenCalledOnce();
  });

  it('clicking the SAME type is a no-op (no onTypeChange call) but still closes', async () => {
    const onTypeChange = vi.fn();
    const onClose = vi.fn();
    render(ShHeaderMenu, { props: baseProps({ column: col({ col_type: 'text' }), onTypeChange, onClose }) });
    await fireEvent.click(screen.getByTestId('sheet-header-menu-type'));
    await fireEvent.click(screen.getByTestId('sheet-header-menu-type-text'));
    expect(onTypeChange).not.toHaveBeenCalled();
    expect(onClose).toHaveBeenCalledOnce();
  });

  it('escape from the type view steps back to root, not a full close', async () => {
    const onClose = vi.fn();
    render(ShHeaderMenu, { props: baseProps({ onClose }) });
    await fireEvent.click(screen.getByTestId('sheet-header-menu-type'));
    await fireEvent.keyDown(screen.getByTestId('sheet-header-menu'), { key: 'Escape' });
    expect(onClose).not.toHaveBeenCalled();
    expect(screen.getByTestId('sheet-header-menu-rename')).toBeTruthy();
  });

  // #2874 item 9 (docs/audits/2026-09-07-item9-rescope-v2.md §4): the
  // type-submenu row icon used to be `label[0]` (first letter of the
  // display label) — wrong for every row (N/C/D/C/S instead of the
  // design frame's T/#/$/calendar/checkbox/list). `typeGlyphMarkup` in
  // ShHeaderMenu.svelte is the replacement; it's a private helper (no
  // export, matching this file's existing convention for `filterSupported`/
  // `typeLabel` etc.), so it's exercised here the same way those already
  // are — through the rendered DOM inside `.sh-pop__ico`, not a direct
  // unit-import.
  it('renders monospace T/#/$ characters for text/number/currency row icons (not the old label[0] letters)', async () => {
    render(ShHeaderMenu, { props: baseProps({ column: col({ col_type: 'text' }) }) });
    await fireEvent.click(screen.getByTestId('sheet-header-menu-type'));
    const textIco = screen.getByTestId('sheet-header-menu-type-text').querySelector('.sh-pop__ico');
    const numberIco = screen.getByTestId('sheet-header-menu-type-number').querySelector('.sh-pop__ico');
    const currencyIco = screen.getByTestId('sheet-header-menu-type-currency').querySelector('.sh-pop__ico');
    expect(textIco?.textContent).toBe('T');
    expect(numberIco?.textContent).toBe('#'); // old code rendered 'N' (label[0] of "Number")
    expect(currencyIco?.textContent).toBe('$'); // old code rendered 'C' (label[0] of "Currency")
    // Confirm these are monospace spans, not bare text nodes / SVGs.
    expect(textIco?.querySelector('span')).toBeTruthy();
    expect(textIco?.querySelector('svg')).toBeNull();
  });

  it('renders real SVG icons (not letters) for date/checkbox/select row icons', async () => {
    render(ShHeaderMenu, { props: baseProps({ column: col({ col_type: 'text' }) }) });
    await fireEvent.click(screen.getByTestId('sheet-header-menu-type'));
    const dateIco = screen.getByTestId('sheet-header-menu-type-date').querySelector('.sh-pop__ico');
    const checkboxIco = screen.getByTestId('sheet-header-menu-type-checkbox').querySelector('.sh-pop__ico');
    const selectIco = screen.getByTestId('sheet-header-menu-type-select').querySelector('.sh-pop__ico');
    // Old code rendered bare letters ('D', 'C', 'S') here — now real SVGs.
    expect(dateIco?.querySelector('svg')).toBeTruthy();
    expect(checkboxIco?.querySelector('svg')).toBeTruthy();
    expect(selectIco?.querySelector('svg')).toBeTruthy();
    expect(dateIco?.textContent?.trim()).toBe('');
    expect(checkboxIco?.textContent?.trim()).toBe('');
    expect(selectIco?.textContent?.trim()).toBe('');
  });

  it('renders a checkmark SVG (not a "✓" text glyph) in the tick span for the is-on row', async () => {
    render(ShHeaderMenu, { props: baseProps({ column: col({ col_type: 'currency' }) }) });
    await fireEvent.click(screen.getByTestId('sheet-header-menu-type'));
    const tick = screen.getByTestId('sheet-header-menu-type-currency').querySelector('.sh-pop__tick');
    expect(tick?.querySelector('svg')).toBeTruthy();
    expect(tick?.textContent?.trim()).toBe('');
  });
});

describe('ShHeaderMenu — filter view (ruling 3)', () => {
  it('opens a value-picker with True/False for a checkbox column', async () => {
    render(ShHeaderMenu, { props: baseProps({ column: col({ col_type: 'checkbox' }) }) });
    await fireEvent.click(screen.getByTestId('sheet-header-menu-filter'));
    const values = screen.getAllByTestId('sheet-header-menu-filter-value');
    expect(values).toHaveLength(2);
    expect(screen.getByText('True')).toBeTruthy();
    expect(screen.getByText('False')).toBeTruthy();
  });

  it('opens a value-picker from options_json.choices for a select column', async () => {
    render(ShHeaderMenu, {
      props: baseProps({ column: col({ col_type: 'select', options_json: '{"choices":["Draft","Shipped"]}' }) }),
    });
    await fireEvent.click(screen.getByTestId('sheet-header-menu-filter'));
    expect(screen.getByText('Draft')).toBeTruthy();
    expect(screen.getByText('Shipped')).toBeTruthy();
  });

  it('toggling a filter value calls onFilterChange with the updated set', async () => {
    const onFilterChange = vi.fn();
    render(ShHeaderMenu, {
      props: baseProps({
        column: col({ col_type: 'select', options_json: '{"choices":["Draft","Shipped"]}' }),
        filterValues: null,
        onFilterChange,
      }),
    });
    await fireEvent.click(screen.getByTestId('sheet-header-menu-filter'));
    await fireEvent.click(screen.getByText('Draft'));
    expect(onFilterChange).toHaveBeenCalledWith(['Draft']);
  });

  it('clicking an already-active filter value removes it from the set', async () => {
    const onFilterChange = vi.fn();
    render(ShHeaderMenu, {
      props: baseProps({
        column: col({ col_type: 'select', options_json: '{"choices":["Draft","Shipped"]}' }),
        filterValues: ['Draft', 'Shipped'],
        onFilterChange,
      }),
    });
    await fireEvent.click(screen.getByTestId('sheet-header-menu-filter'));
    await fireEvent.click(screen.getByText('Draft'));
    expect(onFilterChange).toHaveBeenCalledWith(['Shipped']);
  });

  it('the menu stays open after toggling a filter value (onClose not called)', async () => {
    const onClose = vi.fn();
    render(ShHeaderMenu, {
      props: baseProps({
        column: col({ col_type: 'select', options_json: '{"choices":["Draft"]}' }),
        onClose,
      }),
    });
    await fireEvent.click(screen.getByTestId('sheet-header-menu-filter'));
    await fireEvent.click(screen.getByText('Draft'));
    expect(onClose).not.toHaveBeenCalled();
    expect(screen.getByTestId('sheet-header-menu')).toBeTruthy();
  });

  it('shows a fallback message when a select column has no configured choices', async () => {
    render(ShHeaderMenu, { props: baseProps({ column: col({ col_type: 'select', options_json: '{}' }) }) });
    await fireEvent.click(screen.getByTestId('sheet-header-menu-filter'));
    expect(screen.getByText(/No options configured/)).toBeTruthy();
  });
});

// #2874 item 16 Part C — `initialView` prop (toolbar Filter button's
// entry point, `SheetGrid.svelte`'s `toolbarFilter()`).
describe('ShHeaderMenu — initialView prop (#2874 item 16 Part C)', () => {
  it('mounts straight into the filter view when initialView is "filter" for a supported column', () => {
    render(ShHeaderMenu, {
      props: baseProps({
        column: col({ col_type: 'select', options_json: '{"choices":["Draft","Shipped"]}' }),
        initialView: 'filter',
      }),
    });
    // The root menu's own rows are NOT present — we landed straight in
    // the filter value-picker view.
    expect(screen.queryByTestId('sheet-header-menu-rename')).toBeNull();
    expect(screen.getByText('Draft')).toBeTruthy();
    expect(screen.getByText('Shipped')).toBeTruthy();
  });

  it('falls back to the root view when initialView is "filter" but the column type does not support filtering', () => {
    render(ShHeaderMenu, {
      props: baseProps({ column: col({ col_type: 'text' }), initialView: 'filter' }),
    });
    expect(screen.getByTestId('sheet-header-menu-rename')).toBeTruthy();
  });

  it('every existing header-click entry point (no initialView prop) still lands on the root view', () => {
    render(ShHeaderMenu, { props: baseProps() });
    expect(screen.getByTestId('sheet-header-menu-rename')).toBeTruthy();
  });

  it('an explicit initialView of "default" also lands on the root view', () => {
    render(ShHeaderMenu, { props: baseProps({ initialView: 'default' }) });
    expect(screen.getByTestId('sheet-header-menu-rename')).toBeTruthy();
  });
});
