// SheetToolbar.test.ts — component tests for the sheet toolbar's
// insert-row/insert-column wiring (PR 10) and the "More" overflow menu
// wiring (#2339 PR 15b-alpha).

import { describe, it, expect, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/svelte/svelte5';
import SheetToolbar from './SheetToolbar.svelte';

// vitest-setup.ts replaces `globalThis.window` with a plain mock object
// lacking addEventListener/removeEventListener — `ShSheetMenu.svelte`
// (mounted here once the More menu opens) uses `<svelte:window onclick>`
// for outside-click close, same patch as its own test file.
if (typeof (globalThis.window as any).addEventListener !== 'function') {
  (globalThis.window as any).addEventListener = () => {};
  (globalThis.window as any).removeEventListener = () => {};
}
if (typeof (globalThis.window as any).innerWidth !== 'number') {
  (globalThis.window as any).innerWidth = 1200;
}

describe('SheetToolbar — insert row/column (PR 10)', () => {
  it('calls onInsertColumn when the insert-column button is clicked', async () => {
    const onInsertColumn = vi.fn();
    render(SheetToolbar, { props: { onInsertColumn } });
    await fireEvent.click(screen.getByTestId('sheet-tb-insert-column'));
    expect(onInsertColumn).toHaveBeenCalledOnce();
  });

  it('calls onInsertRow when the insert-row button is clicked', async () => {
    const onInsertRow = vi.fn();
    render(SheetToolbar, { props: { onInsertRow } });
    await fireEvent.click(screen.getByTestId('sheet-tb-insert-row'));
    expect(onInsertRow).toHaveBeenCalledOnce();
  });
});

describe('SheetToolbar — More menu (#2339 PR 15b-alpha)', () => {
  it('the More button is no longer disabled', () => {
    render(SheetToolbar, {});
    const btn = screen.getByTestId('sheet-tb-more') as HTMLButtonElement;
    expect(btn.disabled).toBeFalsy();
  });

  it('clicking More opens the ShSheetMenu popover', async () => {
    render(SheetToolbar, {});
    expect(screen.queryByTestId('sheet-more-menu')).toBeNull();
    await fireEvent.click(screen.getByTestId('sheet-tb-more'));
    expect(screen.getByTestId('sheet-more-menu')).toBeTruthy();
  });

  it('clicking More again closes it (toggle)', async () => {
    render(SheetToolbar, {});
    await fireEvent.click(screen.getByTestId('sheet-tb-more'));
    expect(screen.getByTestId('sheet-more-menu')).toBeTruthy();
    await fireEvent.click(screen.getByTestId('sheet-tb-more'));
    expect(screen.queryByTestId('sheet-more-menu')).toBeNull();
  });

  it('Export as CSV… in the popover calls onExportCsv and closes the menu', async () => {
    const onExportCsv = vi.fn();
    render(SheetToolbar, { props: { onExportCsv } });
    await fireEvent.click(screen.getByTestId('sheet-tb-more'));
    await fireEvent.click(screen.getByTestId('sheet-more-menu-export'));
    expect(onExportCsv).toHaveBeenCalledOnce();
    expect(screen.queryByTestId('sheet-more-menu')).toBeNull();
  });

  it('Version history in the popover calls onVersionHistory', async () => {
    const onVersionHistory = vi.fn();
    render(SheetToolbar, { props: { onVersionHistory } });
    await fireEvent.click(screen.getByTestId('sheet-tb-more'));
    await fireEvent.click(screen.getByTestId('sheet-more-menu-history'));
    expect(onVersionHistory).toHaveBeenCalledOnce();
  });

  it('does not render a Rename sheet row in the popover', async () => {
    render(SheetToolbar, {});
    await fireEvent.click(screen.getByTestId('sheet-tb-more'));
    expect(screen.queryByTestId('sheet-more-menu-rename')).toBeNull();
  });

  it('does not render a Move to… row in the popover (#2874 item 49 — folder pill covers it)', async () => {
    render(SheetToolbar, {});
    await fireEvent.click(screen.getByTestId('sheet-tb-more'));
    expect(screen.queryByTestId('sheet-more-menu-move')).toBeNull();
  });

  it('Delete sheet in the popover calls onDeleteSheet', async () => {
    const onDeleteSheet = vi.fn();
    render(SheetToolbar, { props: { onDeleteSheet } });
    await fireEvent.click(screen.getByTestId('sheet-tb-more'));
    await fireEvent.click(screen.getByTestId('sheet-more-menu-delete'));
    expect(onDeleteSheet).toHaveBeenCalledOnce();
  });
});

// #2874 item 20 — the three separate fill/text/border buttons are
// replaced with a single unified "Style cells" button (bucket+droplet
// glyph). Also closes item 13 (the old fill button's glyph was missing
// the droplet detail — the new glyph includes it). Matt's callback-
// pattern ruling still applies: this toolbar never receives or reasons
// about selection state, it just fires a plain callback for the button.
describe('SheetToolbar — unified style button (#2874 item 20, closes item 13)', () => {
  it('renders exactly one style button, not three', () => {
    render(SheetToolbar, {});
    expect(screen.queryByTestId('sheet-tb-fill')).toBeNull();
    expect(screen.queryByTestId('sheet-tb-text')).toBeNull();
    expect(screen.queryByTestId('sheet-tb-border')).toBeNull();
    expect(screen.getByTestId('sheet-tb-style')).toBeTruthy();
  });

  it('has title "Style cells"', () => {
    render(SheetToolbar, {});
    expect(screen.getByTestId('sheet-tb-style').getAttribute('title')).toBe('Style cells');
  });

  it('clicking the style button calls onStyleClick', async () => {
    const onStyleClick = vi.fn();
    render(SheetToolbar, { props: { onStyleClick } });
    await fireEvent.click(screen.getByTestId('sheet-tb-style'));
    expect(onStyleClick).toHaveBeenCalledOnce();
  });

  it('clicking the style button passes its own button element to onStyleClick', async () => {
    const onStyleClick = vi.fn();
    render(SheetToolbar, { props: { onStyleClick } });
    const btn = screen.getByTestId('sheet-tb-style');
    await fireEvent.click(btn);
    expect(onStyleClick).toHaveBeenCalledWith(btn);
  });

  it('renders the bucket-with-droplet glyph (two path elements)', () => {
    render(SheetToolbar, {});
    const btn = screen.getByTestId('sheet-tb-style');
    expect(btn.querySelectorAll('svg path').length).toBe(2);
  });
});

// #2874 item 16 Part A — Version History toolbar button.
describe('SheetToolbar — Version history button (#2874 item 16 Part A)', () => {
  it('the version history button is no longer disabled', () => {
    render(SheetToolbar, {});
    const btn = screen.getByTestId('sheet-tb-history') as HTMLButtonElement;
    expect(btn.disabled).toBeFalsy();
  });

  it('clicking the version history button calls onVersionHistory', async () => {
    const onVersionHistory = vi.fn();
    render(SheetToolbar, { props: { onVersionHistory } });
    await fireEvent.click(screen.getByTestId('sheet-tb-history'));
    expect(onVersionHistory).toHaveBeenCalledOnce();
  });
});

// #2874 item 16 Part C — fx/Sort/Filter toolbar wiring.
describe('SheetToolbar — fx/Sort/Filter buttons (#2874 item 16 Part C)', () => {
  it('formula/sort/filter buttons are no longer disabled by default', () => {
    render(SheetToolbar, {});
    expect((screen.getByTestId('sheet-tb-formula') as HTMLButtonElement).disabled).toBeFalsy();
    expect((screen.getByTestId('sheet-tb-sort') as HTMLButtonElement).disabled).toBeFalsy();
    expect((screen.getByTestId('sheet-tb-filter') as HTMLButtonElement).disabled).toBeFalsy();
  });

  it('clicking the formula button calls onFormulaClick', async () => {
    const onFormulaClick = vi.fn();
    render(SheetToolbar, { props: { onFormulaClick } });
    await fireEvent.click(screen.getByTestId('sheet-tb-formula'));
    expect(onFormulaClick).toHaveBeenCalledOnce();
  });

  it('clicking the sort button calls onSortClick', async () => {
    const onSortClick = vi.fn();
    render(SheetToolbar, { props: { onSortClick } });
    await fireEvent.click(screen.getByTestId('sheet-tb-sort'));
    expect(onSortClick).toHaveBeenCalledOnce();
  });

  it('clicking the filter button calls onFilterClick with its own button element', async () => {
    const onFilterClick = vi.fn();
    render(SheetToolbar, { props: { onFilterClick } });
    const btn = screen.getByTestId('sheet-tb-filter');
    await fireEvent.click(btn);
    expect(onFilterClick).toHaveBeenCalledWith(btn);
  });

  it('the formula button is disabled when formulaEnabled is false (no cell selected)', () => {
    render(SheetToolbar, { props: { formulaEnabled: false } });
    expect((screen.getByTestId('sheet-tb-formula') as HTMLButtonElement).disabled).toBe(true);
  });

  it('the sort button is disabled when sortEnabled is false (no column resolvable)', () => {
    render(SheetToolbar, { props: { sortEnabled: false } });
    expect((screen.getByTestId('sheet-tb-sort') as HTMLButtonElement).disabled).toBe(true);
  });

  it('the filter button is disabled when filterEnabled is false (unsupported column type)', () => {
    render(SheetToolbar, { props: { filterEnabled: false } });
    expect((screen.getByTestId('sheet-tb-filter') as HTMLButtonElement).disabled).toBe(true);
  });
});
