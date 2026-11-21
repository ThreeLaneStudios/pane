// ShCellContextMenu.test.ts — component tests for the Sheets grid's
// right-click cell/range context menu (#2339 PR 15b-alpha (alpha)).

import { describe, it, expect, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/svelte/svelte5';
import ShCellContextMenu from './ShCellContextMenu.svelte';

// vitest-setup.ts replaces `globalThis.window` with a plain mock object
// lacking addEventListener/removeEventListener — same patch as every
// other Sheets popover's own tests (ShSheetMenu.test.ts et al.).
if (typeof (globalThis.window as any).addEventListener !== 'function') {
  (globalThis.window as any).addEventListener = () => {};
  (globalThis.window as any).removeEventListener = () => {};
}
if (typeof (globalThis.window as any).innerWidth !== 'number') {
  (globalThis.window as any).innerWidth = 1200;
}
if (typeof (globalThis.window as any).innerHeight !== 'number') {
  (globalThis.window as any).innerHeight = 900;
}

function baseProps(overrides: Partial<Record<string, unknown>> = {}) {
  return {
    x: 100,
    y: 100,
    cellCount: 1,
    onStyleCells: vi.fn(),
    onClearStyling: vi.fn(),
    onInsertRow: vi.fn(),
    onDeleteContents: vi.fn(),
    onClose: vi.fn(),
    ...overrides,
  };
}

describe('ShCellContextMenu', () => {
  it('renders all four action rows in design order', () => {
    render(ShCellContextMenu, { props: baseProps() });
    const menu = screen.getByTestId('cell-context-menu');
    const rowIds = Array.from(menu.querySelectorAll('[data-testid]')).map((el) => el.getAttribute('data-testid'));
    expect(rowIds).toEqual([
      'cell-context-menu-style',
      'cell-context-menu-clear-styling',
      'cell-context-menu-insert-row',
      'cell-context-menu-delete-contents',
    ]);
  });

  // #2874 item 31: "Insert column left" moved to `ShHeaderMenu.svelte`'s
  // column-operations group — regression guard that it does not reappear
  // here (removed entirely, not merely duplicated).
  it('does not render Insert column left', () => {
    render(ShCellContextMenu, { props: baseProps() });
    expect(screen.queryByTestId('cell-context-menu-insert-column')).toBeNull();
    expect(screen.queryByText('Insert column left')).toBeNull();
  });

  it('renders the ⌘⇧S shortcut hint on the Style cells… row', () => {
    render(ShCellContextMenu, { props: baseProps() });
    const styleRow = screen.getByTestId('cell-context-menu-style');
    expect(styleRow.textContent).toContain('⌘⇧S');
    expect(styleRow.textContent).toContain('Style cells…');
  });

  it('Style cells… calls onStyleCells and closes', async () => {
    const onStyleCells = vi.fn();
    const onClose = vi.fn();
    render(ShCellContextMenu, { props: baseProps({ onStyleCells, onClose }) });
    await fireEvent.click(screen.getByTestId('cell-context-menu-style'));
    expect(onStyleCells).toHaveBeenCalledOnce();
    expect(onClose).toHaveBeenCalledOnce();
  });

  it('Clear styling calls onClearStyling and closes', async () => {
    const onClearStyling = vi.fn();
    const onClose = vi.fn();
    render(ShCellContextMenu, { props: baseProps({ onClearStyling, onClose }) });
    await fireEvent.click(screen.getByTestId('cell-context-menu-clear-styling'));
    expect(onClearStyling).toHaveBeenCalledOnce();
    expect(onClose).toHaveBeenCalledOnce();
  });

  it('Insert row above calls onInsertRow and closes', async () => {
    const onInsertRow = vi.fn();
    const onClose = vi.fn();
    render(ShCellContextMenu, { props: baseProps({ onInsertRow, onClose }) });
    await fireEvent.click(screen.getByTestId('cell-context-menu-insert-row'));
    expect(onInsertRow).toHaveBeenCalledOnce();
    expect(onClose).toHaveBeenCalledOnce();
  });

  it('Delete N cells\' contents calls onDeleteContents and closes', async () => {
    const onDeleteContents = vi.fn();
    const onClose = vi.fn();
    render(ShCellContextMenu, { props: baseProps({ onDeleteContents, onClose }) });
    await fireEvent.click(screen.getByTestId('cell-context-menu-delete-contents'));
    expect(onDeleteContents).toHaveBeenCalledOnce();
    expect(onClose).toHaveBeenCalledOnce();
  });

  it('the danger row is styled sh-pop__danger', () => {
    render(ShCellContextMenu, { props: baseProps() });
    const deleteRow = screen.getByTestId('cell-context-menu-delete-contents');
    expect(deleteRow.className).toContain('sh-pop__danger');
  });

  it('the danger row label uses the singular "cell" for cellCount 1', () => {
    render(ShCellContextMenu, { props: baseProps({ cellCount: 1 }) });
    const deleteRow = screen.getByTestId('cell-context-menu-delete-contents');
    expect(deleteRow.textContent).toContain("Delete 1 cell's contents");
  });

  it('the danger row label uses the plural "cells" and the real count for a multi-cell range', () => {
    render(ShCellContextMenu, { props: baseProps({ cellCount: 6 }) });
    const deleteRow = screen.getByTestId('cell-context-menu-delete-contents');
    expect(deleteRow.textContent).toContain("Delete 6 cells' contents");
  });

  it('Escape closes the menu', async () => {
    const onClose = vi.fn();
    render(ShCellContextMenu, { props: baseProps({ onClose }) });
    await fireEvent.keyDown(screen.getByTestId('cell-context-menu'), { key: 'Escape' });
    expect(onClose).toHaveBeenCalledOnce();
  });

  it('clicking inside the popover does not call onClose (click stopPropagation guard)', async () => {
    const onClose = vi.fn();
    render(ShCellContextMenu, { props: baseProps({ onClose }) });
    await fireEvent.click(screen.getByTestId('cell-context-menu'));
    expect(onClose).not.toHaveBeenCalled();
  });

  it('positions at the given x/y coordinate when there is room on every side', () => {
    render(ShCellContextMenu, { props: baseProps({ x: 100, y: 100 }) });
    const menu = screen.getByTestId('cell-context-menu');
    expect(menu.style.left).toBe('100px');
    expect(menu.style.top).toBe('100px');
  });

  it('flips left (menu width subtracted) when x is near the right viewport edge', () => {
    (globalThis.window as any).innerWidth = 300;
    render(ShCellContextMenu, { props: baseProps({ x: 280, y: 100 }) });
    const menu = screen.getByTestId('cell-context-menu');
    expect(menu.style.left).toBe(`${280 - 240}px`);
    (globalThis.window as any).innerWidth = 1200;
  });

  it('flips up (menu height subtracted) when y is near the bottom viewport edge', () => {
    (globalThis.window as any).innerHeight = 400;
    render(ShCellContextMenu, { props: baseProps({ x: 100, y: 380 }) });
    const menu = screen.getByTestId('cell-context-menu');
    expect(menu.style.top).toBe(`${380 - 257}px`);
    (globalThis.window as any).innerHeight = 900;
  });

  // #2874 item 57 (Matt's ruling): floor the flipped `posTop` at 0, same
  // shape as the horizontal clamp, so a mid-screen flip-up on a short
  // window can never clip off the TOP of the window. MENU_HEIGHT (257)
  // is more than half of the app's literal 600px minimum window height,
  // so no y value at exactly 600px can produce a negative flipped top
  // (the flip-trigger and negative-result ranges don't overlap once
  // viewportHeight > 2 * MENU_HEIGHT). 400px is the smallest window
  // already exercised by this file's own flip-up test above, and it
  // genuinely reproduces the unbounded-flip bug for a MID-SCREEN y
  // (200, the center of 400) — not just a near-bottom one.
  it('mid-screen y at a 400px window height flips up PAST the top — clamped to 0 (base: -57px)', () => {
    (globalThis.window as any).innerHeight = 400;
    render(ShCellContextMenu, { props: baseProps({ x: 100, y: 200 }) });
    const menu = screen.getByTestId('cell-context-menu');
    // 200 + MENU_HEIGHT (257) = 457 > innerHeight (400) so it flips up:
    // 200 - MENU_HEIGHT (257) = -57 at base (BEFORE this fix — clips off
    // the top of the window). After the floor: Math.max(0, -57) = 0.
    expect(menu.style.top).toBe('0px');
    (globalThis.window as any).innerHeight = 900;
  });
});
