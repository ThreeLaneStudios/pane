/**
 * T1.10: C27 (#3144) — `.app` sizing, real layout.
 *
 * LOCAL-ONLY. Do not add to any CI job; `e2e.yml` ("E2E — Playwright
 * (self-hosted)") is `disabled_manually` (Matt, 2026-10-30) and, even if
 * re-enabled, it would not catch this bug anyway: Playwright's bundled
 * WebKit (v2248) is not WKWebView and does not reproduce this symptom —
 * a byte-faithful repro of the exact shipped CSS chain passed clean,
 * 6/6 structural variants, in both Playwright WebKit and Chromium (see
 * docs/audits/2026-11-02-scope-3144-c27-scroll.md, §3). The one
 * mechanism in this repo that exercises the REAL WKWebView is this
 * file's own fixture (`../../fixtures.ts`, the `@srsholmes/tauri-
 * playwright` socket bridge against a locally-running, real Tauri
 * build) — scoped local-only by the project's own #514 hybrid-E2E ADR,
 * independently of this issue. Run this file by hand against the real
 * built app whenever `.app`'s sizing changes; this is the only test in
 * the repo capable of proving anything about it.
 *
 * Confirmation beyond this file is Matt's live re-walk of the built app
 * (`4d-skip: Matt`) — the full CI-skip reasoning lives in the PR
 * description, not here, to keep this file's source clean.
 */
import { test, expect } from '../../fixtures';
test.describe('T1.10: C27 — .app sizing (local-only, real WKWebView)', () => {
  test('`.app`\'s height equals the real viewport height', async ({ tauriPage }) => {
    // Test fixture already waits for app-loaded and auth bypass; still
    // wait explicitly, same convention as every other tier1/tier2 test.
    await tauriPage.waitForSelector('[data-testid="app-loaded"]', 10000);

    const appHeightMatchesViewport = await tauriPage.evaluate<boolean>(`
      (() => {
        const app = document.querySelector('.app');
        return !!app && app.clientHeight === window.innerHeight;
      })()
    `);

    expect(appHeightMatchesViewport).toBe(true);
  });

  test('`.messages-area` scrolls once overfilled with content', async ({ tauriPage }) => {
    await tauriPage.waitForSelector('[data-testid="app-loaded"]', 10000);

    const chatInput = 'textarea.message-input';
    await tauriPage.waitForSelector(chatInput, 5000);

    // Enough user messages to overflow .messages-area regardless of
    // whether any AI response ever arrives — this test asserts layout,
    // not chat completion, so it never waits on a provider being
    // configured.
    const messageCount = 25;
    for (let i = 1; i <= messageCount; i++) {
      await tauriPage.fill(chatInput, `C27 layout filler message ${i}`);
      await tauriPage.press(chatInput, 'Enter');
    }

    // Wait on the actual DOM condition (every filler row rendered),
    // never a fixed timer — and never poll a mock/proxy for the
    // condition under test; the condition IS the row count.
    await tauriPage.waitForFunction(
      `document.querySelectorAll('.message.user').length >= ${messageCount}`,
      10000,
    );

    const messagesAreaOverflows = await tauriPage.evaluate<boolean>(`
      (() => {
        const el = document.querySelector('.messages-area');
        return !!el && el.scrollHeight > el.clientHeight;
      })()
    `);

    expect(messagesAreaOverflows).toBe(true);
  });
});
