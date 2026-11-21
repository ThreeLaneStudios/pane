import { test, expect } from '../../fixtures';

// #2339 PR 9: Sheet grid frontend smoke tests.
// Follows the note-crud.spec.ts stub pattern — full interaction is blocked on
// the same auth-bypass gap tracked at #469; each test documents the exact
// steps + selectors to wire up once that bypass lands, using the real
// data-testid hooks emitted by SheetGrid.svelte/SheetToolbar.svelte/
// SheetStatusBar.svelte and the "+ New Sheet" buttons added to each host view.

test.describe('T1.9: Sheet Grid (PR 9, read-only)', () => {
  test('should render a sheet grid in Kanban board view', async ({ tauriPage }) => {
    // Requires auth bypass in test mode — see #469
    console.log('[SHEET-GRID] This test requires authenticated state');

    // IMPLEMENTATION (once auth bypass exists):
    // 1. Wait for app to load, navigate to a project's Kanban board.
    // await tauriPage.waitForSelector('[data-testid="app-loaded"]', 10000);
    // 2. Click "+ New Sheet" beside "+ New Note".
    // await tauriPage.click('button.add-sheet-btn[title="New Sheet"]');
    // 3. Verify the sheet grid renders with default 5 blank text columns.
    // await tauriPage.waitForSelector('[data-testid="sheet-grid"]', 5000);
    // const headerCount = await tauriPage.locator('[data-testid^="sheet-col-header-"]').count();
    // expect(headerCount).toBe(5);
    // 4. Verify the toolbar and status bar render.
    // await expect(tauriPage.locator('[data-testid="sheet-toolbar"]')).toBeVisible();
    // await expect(tauriPage.locator('[data-testid="sheet-status-bar"]')).toBeVisible();
  });

  test('should render a sheet grid in All Tasks view', async ({ tauriPage }) => {
    // Requires auth bypass in test mode — see #469
    console.log('[SHEET-GRID] This test requires authenticated state');

    // IMPLEMENTATION (once auth bypass exists):
    // 1. Navigate to the All Tasks view.
    // 2. Click "+ New Sheet", pick a project from the picker dropdown.
    // await tauriPage.click('button.add-sheet-btn[title="New Sheet"]');
    // await tauriPage.click('.sheet-picker-item >> nth=0');
    // 3. Verify the grid renders (same assertions as Kanban test above).
  });

  test('should render a sheet grid in Notes Browser view', async ({ tauriPage }) => {
    // Requires auth bypass in test mode — see #469
    console.log('[SHEET-GRID] This test requires authenticated state');

    // IMPLEMENTATION (once auth bypass exists):
    // 1. Navigate to a project's Notes Browser (folder view).
    // 2. Click "+ New Sheet" beside "+ New Note" in the folder header.
    // await tauriPage.click('button.new-sheet-btn[title="New Sheet"]');
    // 3. Verify the grid renders inside the .fv-note overlay panel.
  });

  test('should keep sticky headers pinned while scrolling the grid', async ({ tauriPage }) => {
    // Requires auth bypass in test mode — see #469
    console.log('[SHEET-GRID] This test requires authenticated state');

    // IMPLEMENTATION (once auth bypass exists):
    // 1. Open a sheet with enough rows to overflow the viewport (or rely on
    //    ghost rows, which always extend past the real grid).
    // 2. Scroll the .sh-gridwrap container.
    // await tauriPage.locator('.sh-gridwrap').evaluate(el => el.scrollTop = 200);
    // 3. Assert the column header row's bounding box top stays pinned at the
    //    scroll container's top (sticky positioning), not scrolled out of view.
    // const headerBox = await tauriPage.locator('.sh-h').first().boundingBox();
    // const wrapBox = await tauriPage.locator('.sh-gridwrap').boundingBox();
    // expect(headerBox.y).toBeGreaterThanOrEqual(wrapBox.y);
  });

  test('should select a cell and show the anchor ring on click', async ({ tauriPage }) => {
    // Requires auth bypass in test mode — see #469
    console.log('[SHEET-GRID] This test requires authenticated state');

    // IMPLEMENTATION (once auth bypass exists):
    // 1. Open a sheet (A1 is pre-selected by default per sheetSelection.ts's
    //    INITIAL_SHEET_SELECTION).
    // await expect(tauriPage.locator('[data-testid="sheet-cell-0-0"]')).toHaveClass(/is-anchor/);
    // 2. Click a different cell and verify the anchor ring moves, with no
    //    update_sheet_cell network/invoke call fired (PR 9 is read-only).
    // await tauriPage.click('[data-testid="sheet-cell-1-2"]');
    // await expect(tauriPage.locator('[data-testid="sheet-cell-1-2"]')).toHaveClass(/is-anchor/);
    // await expect(tauriPage.locator('[data-testid="sheet-cell-0-0"]')).not.toHaveClass(/is-anchor/);
  });

  test('should navigate cells with arrow keys and extend range with shift+arrow', async ({ tauriPage }) => {
    // Requires auth bypass in test mode — see #469
    console.log('[SHEET-GRID] This test requires authenticated state');

    // IMPLEMENTATION (once auth bypass exists):
    // 1. Open a sheet, focus the grid (.sh-gridwrap, tabindex=0, role=grid).
    // await tauriPage.locator('[data-testid="sheet-grid"]').focus();
    // 2. Press ArrowRight, assert anchor moved from A1 to B1.
    // await tauriPage.keyboard.press('ArrowRight');
    // await expect(tauriPage.locator('[data-testid="sheet-cell-0-1"]')).toHaveClass(/is-anchor/);
    // 3. Press Shift+ArrowDown, assert a multi-cell range (.is-range) appears
    //    spanning B1:B2, with no cell content mutated (still read-only).
    // await tauriPage.keyboard.press('Shift+ArrowDown');
    // await expect(tauriPage.locator('[data-testid="sheet-cell-1-1"]')).toHaveClass(/is-range/);
  });
});
