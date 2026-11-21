import { test } from '@playwright/test';
import { setupHomeView, assertLocalFirstBoot } from './home-fixtures';

/**
 * ADR-0070 Decision 1 (#3195 PR B) — red-first test T1.
 *
 * "Pane opens to Home — Home chat plus the Jobs panel — on every launch,
 * with no sign-in and no entitlement check." Before this PR: with no
 * stored session (the default — `__PANE_TEST_MODE__` is gone, and this
 * harness never set a mock session), `initAuth()` resolves with
 * `authSession = null` (`account_enabled` reads `null` from the mocked
 * `get_ai_setting`, treated as off), so `isAuthenticated` is `false`.
 * `+page.svelte`'s old gate (`{#if !authReady}...{:else if
 * !$isAuthenticated}<AuthScreen/>{:else}...`) rendered `AuthScreen`
 * instead of Home — `app-loaded` never appeared, `.auth-screen` was
 * present, and `assertLocalFirstBoot`'s wait for `app-loaded` timed out.
 *
 * Red output (captured against the pre-PR-B gate, pasted verbatim in the
 * PR body — not narrated in a commit message):
 *
 *   1) tests/e2e/local-first-boot.spec.ts:XX:X › T1: local-first boot ›
 *      boots straight to Home — no AuthScreen, no auth-loading, no
 *      account/sync IPC call
 *
 *      Test timeout of 15000ms exceeded while waiting for selector
 *      "[data-testid=\"app-loaded\"]"
 *
 * `assertLocalFirstBoot` (home-fixtures.ts) is the deliverable itself —
 * this spec is the test that exercises it.
 */
test.describe('T1: local-first boot (ADR-0070 Decision 1)', () => {
  test('boots straight to Home — no AuthScreen, no auth-loading, no account/sync IPC call', async ({ page }) => {
    await setupHomeView(page);
    await assertLocalFirstBoot(page);
  });
});
