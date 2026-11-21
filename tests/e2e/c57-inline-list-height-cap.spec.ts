import { test, expect } from '@playwright/test';
import { setupHomeView } from './home-fixtures';

/**
 * #3182 — inline list height cap (C64's sibling bug).
 *
 * `SearchableSelect.svelte`'s `.dropdown-list-inline` rule had
 * `max-height: none; overflow-y: visible` (#3178 C57) — correct for
 * C57's no-measurement rule, but meant the New job dialog's Agent
 * list had no cap at all and grew to fit every agent, pushing the
 * rest of the dialog's content down with it. Fix is CSS-only: a
 * `calc()`-derived ~5-row cap (same shape as `Card.svelte`'s
 * `calc(11px * 1.4 * 3)` precedent) plus `overflow-y: auto` so the
 * list scrolls internally instead of growing unbounded.
 *
 * This is real computed layout (row height, scrollHeight vs
 * clientHeight) — jsdom has no layout engine, so this can only be
 * proven in a real browser. WebKit specifically, per Reed's scope,
 * since Tauri on macOS is WKWebView and engine choice has mattered
 * for this exact component before (C27).
 */
test.describe('#3182 — SearchableSelect inline dropdown height cap', () => {
  test('Agent list (8 agents) is capped to ~5 rows and scrolls, not unbounded', async ({ page }) => {
    await setupHomeView(page, { agentCount: 8 });

    await page.getByRole('button', { name: 'New job' }).click();
    await page.waitForSelector('[data-testid="new-schedule-modal"]', { timeout: 10000 });

    const agentInput = page.locator('[data-testid="agent-search"]');
    await agentInput.waitFor({ state: 'visible', timeout: 5000 });
    await agentInput.click();

    // The dropdown is in flow (inline mode, #3178) — no own scroll
    // container in floating's sense, but IS now its own capped,
    // scrollable box (the fix). Wait on the actual DOM condition
    // (all 8 rows rendered), never a fixed timer.
    await page.waitForFunction(
      () => document.querySelectorAll('.dropdown-list-inline .dropdown-item').length >= 8,
      undefined,
      { timeout: 5000 },
    );

    const metrics = await page.evaluate(() => {
      const list = document.querySelector('.dropdown-list-inline');
      if (!list) return null;
      const rect = list.getBoundingClientRect();
      return {
        clientHeight: (list as HTMLElement).clientHeight,
        scrollHeight: (list as HTMLElement).scrollHeight,
        rectHeight: rect.height,
      };
    });

    expect(metrics).not.toBeNull();
    // ~5 rows ≈ 203px per Reed's derivation (14px*1.4 + 20px padding +
    // 1px border, ×5) — assert it's capped well below the 8-row
    // unbounded height (8 rows ≈ 325px) and scrolls internally.
    expect(metrics!.rectHeight).toBeLessThan(230);
    expect(metrics!.scrollHeight).toBeGreaterThan(metrics!.clientHeight);

    // The dialog itself: its own max-height:86vh;overflow-y:auto was
    // already correct before this fix — assert it still holds (the
    // dialog's bounding box stays within the viewport), i.e. this fix
    // didn't need to touch, and didn't break, that separate rule.
    const viewportHeight = page.viewportSize()?.height ?? 0;
    const modalBox = await page.locator('[data-testid="new-schedule-modal"]').boundingBox();
    expect(modalBox).not.toBeNull();
    expect(modalBox!.y + modalBox!.height).toBeLessThanOrEqual(viewportHeight + 1);
  });
});
