/**
 * TODO: #1295 component test — Back navigation
 *
 * This test was intentionally skipped during the #1295+#1341 PR because
 * Onboarding.svelte (~2900 lines) has no existing component test infrastructure,
 * and bootstrapping a full @testing-library/svelte harness that mocks every
 * `invoke` call and seeds internal $state would exceed the ~50-line threshold
 * established in the task spec. A follow-up issue should be filed to add this.
 *
 * When this test is written it should:
 *  1. Render Onboarding.svelte with vi.mock('@tauri-apps/api/core') so that
 *     `invoke` is fully stubbed.
 *  2. Force the component into `screen = 'oc-sync'` and `ocSyncError = 'fake error'`.
 *     (This may require exposing $state via a test prop, or using the component's
 *     internal bind: mechanism if Svelte 5 allows it.)
 *  3. Assert the Back button exists in the rendered DOM.
 *  4. Click the Back button and assert:
 *       - `screen` transitions to `'oc-verify-connect'`
 *       - `ocSyncError` is cleared (empty string)
 *       - `ocSyncing` is false
 *  5. Assert `ocFingerprint`, `ocCertPem`, and `ocToken` are NOT cleared
 *     (pairing already succeeded; only sync state resets).
 */
import { it } from 'vitest';

it.skip('#1295 Back button on oc-sync error navigates to oc-verify-connect and clears sync error state', () => {
  // See TODO comment above — full implementation deferred to follow-up.
});
