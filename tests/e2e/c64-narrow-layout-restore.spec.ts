import { test, expect } from '@playwright/test';
import { setupHomeView } from './home-fixtures';

/**
 * #3182 C64 — narrow-window layout doesn't restore.
 *
 * Root cause: `$chatSidebarOpen` conflated "the user opened it" with
 * "the layout wants it open." `autoCollapsed` (`+page.svelte`) already
 * solved this for the sidebar; `autoChatCollapsed` is its exact mirror
 * for chat (`src/lib/utils/responsiveLayout.ts`). On Home specifically,
 * `ChatSidebar` also now gets `alwaysOpen={true}` (`ChatSidebar.
 * svelte:1629`'s render gate previously depended on `$chatSidebarOpen`
 * alone, which the narrow handler clobbered — so narrowing unmounted
 * Home's ChatSidebar entirely, leaving `SessionsDrawer` rendering alone
 * inside what reads as the main column).
 *
 * This is real, on-screen layout (resize events, WKWebView's actual
 * box model) — **jsdom cannot cover this at all**: no CSS layout
 * engine, and a jsdom test would pass against the broken build exactly
 * as it did for C57 (this file's own cousin, #3178's audit, hit that
 * exact failure mode once already). WebKit specifically, not Chromium
 * — Tauri on macOS is WKWebView.
 */
test.describe('#3182 C64 — narrow-window layout restore (WebKit, real layout)', () => {
  test('resize 1400 -> 800 -> 1400 on Home: chat restored, drawer stays inside the chat panel', async ({ page }) => {
    await page.setViewportSize({ width: 1400, height: 900 });
    await setupHomeView(page);

    // Baseline at 1400: ChatSidebar visible, SessionsDrawer's DOM
    // parent is .chat-panel.
    await page.waitForSelector('.chat-sidebar', { timeout: 10000 });
    const parentClassAt1400 = await page.evaluate(() =>
      document.querySelector('[data-testid="sessions-drawer"]')?.parentElement?.className ?? null,
    );
    expect(parentClassAt1400).toContain('chat-panel');
    expect(await page.locator('.chat-sidebar').isVisible()).toBe(true);

    // Narrow to 800 (<900 — the narrow breakpoint).
    await page.setViewportSize({ width: 800, height: 900 });
    // Resize events aren't auto-fired by setViewportSize in all
    // engines consistently for a window-level listener in a headless
    // context — dispatch explicitly so the app's own 'resize' handler
    // runs deterministically, same as a real user dragging the window.
    await page.evaluate(() => window.dispatchEvent(new Event('resize')));

    // Wait on the actual DOM condition, never a fixed timer: at 800,
    // SessionsDrawer's parent must STILL be .chat-panel (never
    // .main-content) — this is the "drawer in the main column" bug's
    // own assertion, checked mid-sequence, not just at the end.
    await page.waitForFunction(() => {
      const parent = document.querySelector('[data-testid="sessions-drawer"]')?.parentElement;
      return !!parent && parent.className.includes('chat-panel');
    }, undefined, { timeout: 5000 });
    const parentClassAt800 = await page.evaluate(() =>
      document.querySelector('[data-testid="sessions-drawer"]')?.parentElement?.className ?? null,
    );
    expect(parentClassAt800).toContain('chat-panel');
    expect(parentClassAt800).not.toContain('main-content');
    // Home's ChatSidebar carries `alwaysOpen` — it must still be
    // present/visible at narrow width too, not just restored later.
    expect(await page.locator('.chat-sidebar').isVisible()).toBe(true);

    // Widen back to 1400 — the restore.
    await page.setViewportSize({ width: 1400, height: 900 });
    await page.evaluate(() => window.dispatchEvent(new Event('resize')));

    await page.waitForFunction(() => {
      const el = document.querySelector('.chat-sidebar');
      return !!el && (el as HTMLElement).offsetParent !== null;
    }, undefined, { timeout: 5000 });

    expect(await page.locator('.chat-sidebar').isVisible()).toBe(true);
    const parentClassAt1400Again = await page.evaluate(() =>
      document.querySelector('[data-testid="sessions-drawer"]')?.parentElement?.className ?? null,
    );
    expect(parentClassAt1400Again).toContain('chat-panel');
    expect(parentClassAt1400Again).not.toContain('main-content');
  });
});
