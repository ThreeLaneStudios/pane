// ShSheetMenu.test.ts — component tests for the sheet toolbar's "More"
// overflow menu (#2339 PR 15b-alpha, export menu).

import { describe, it, expect, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/svelte/svelte5';
import ShSheetMenu from './ShSheetMenu.svelte';

// vitest-setup.ts replaces `globalThis.window` with a plain mock object
// lacking addEventListener/removeEventListener — same patch as every
// other Sheets popover's own tests (ShHeaderMenu.test.ts et al.).
if (typeof (globalThis.window as any).addEventListener !== 'function') {
  (globalThis.window as any).addEventListener = () => {};
  (globalThis.window as any).removeEventListener = () => {};
}
if (typeof (globalThis.window as any).innerWidth !== 'number') {
  (globalThis.window as any).innerWidth = 1200;
}

function makeAnchor(left = 10): HTMLElement {
  const el = document.createElement('div');
  el.getBoundingClientRect = () => ({
    left,
    right: left + 28,
    top: 0,
    bottom: 34,
    width: 28,
    height: 34,
    x: left,
    y: 0,
    toJSON: () => ({}),
  });
  return el;
}

function baseProps(overrides: Partial<Record<string, unknown>> = {}) {
  return {
    anchorEl: makeAnchor(),
    onExportCsv: vi.fn(),
    onVersionHistory: vi.fn(),
    onDelete: vi.fn(),
    onClose: vi.fn(),
    ...overrides,
  };
}

describe('ShSheetMenu', () => {
  it('renders all four rows in design order: export, history, hr, delete', () => {
    render(ShSheetMenu, { props: baseProps() });
    const menu = screen.getByTestId('sheet-more-menu');
    const rowIds = Array.from(menu.querySelectorAll('[data-testid]')).map((el) => el.getAttribute('data-testid'));
    expect(rowIds).toEqual([
      'sheet-more-menu-export',
      'sheet-more-menu-history',
      'sheet-more-menu-delete',
    ]);
  });

  it('does not render a Move to… row (#2874 item 49 — folder pill covers it)', () => {
    render(ShSheetMenu, { props: baseProps() });
    expect(screen.queryByTestId('sheet-more-menu-move')).toBeNull();
  });

  it('does not render a Rename sheet row', () => {
    render(ShSheetMenu, { props: baseProps() });
    expect(screen.queryByTestId('sheet-more-menu-rename')).toBeNull();
  });

  it('Export as CSV… calls onExportCsv and closes', async () => {
    const onExportCsv = vi.fn();
    const onClose = vi.fn();
    render(ShSheetMenu, { props: baseProps({ onExportCsv, onClose }) });
    await fireEvent.click(screen.getByTestId('sheet-more-menu-export'));
    expect(onExportCsv).toHaveBeenCalledOnce();
    expect(onClose).toHaveBeenCalledOnce();
  });

  it('Version history calls onVersionHistory and closes', async () => {
    const onVersionHistory = vi.fn();
    const onClose = vi.fn();
    render(ShSheetMenu, { props: baseProps({ onVersionHistory, onClose }) });
    await fireEvent.click(screen.getByTestId('sheet-more-menu-history'));
    expect(onVersionHistory).toHaveBeenCalledOnce();
    expect(onClose).toHaveBeenCalledOnce();
  });

  it('Delete sheet is styled sh-pop__danger and calls onDelete + closes', async () => {
    const onDelete = vi.fn();
    const onClose = vi.fn();
    render(ShSheetMenu, { props: baseProps({ onDelete, onClose }) });
    const deleteRow = screen.getByTestId('sheet-more-menu-delete');
    expect(deleteRow.className).toContain('sh-pop__danger');
    await fireEvent.click(deleteRow);
    expect(onDelete).toHaveBeenCalledOnce();
    expect(onClose).toHaveBeenCalledOnce();
  });

  it('Escape closes the menu', async () => {
    const onClose = vi.fn();
    render(ShSheetMenu, { props: baseProps({ onClose }) });
    await fireEvent.keyDown(screen.getByTestId('sheet-more-menu'), { key: 'Escape' });
    expect(onClose).toHaveBeenCalledOnce();
  });

  it('clicking inside the popover does not call onClose (click stopPropagation guard)', async () => {
    const onClose = vi.fn();
    render(ShSheetMenu, { props: baseProps({ onClose }) });
    await fireEvent.click(screen.getByTestId('sheet-more-menu'));
    expect(onClose).not.toHaveBeenCalled();
  });

  it('is unconditionally right-anchored and never applies a flip-right class', () => {
    render(ShSheetMenu, { props: baseProps({ anchorEl: makeAnchor(250) }) });
    const menu = screen.getByTestId('sheet-more-menu');
    expect(menu.className).not.toContain('flip-right');
  });
});
