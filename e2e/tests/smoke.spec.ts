import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from '../fixtures';
import { captureAndValidateScreenshot } from '../helpers/screenshot';

// Resolved relative to this file rather than process.cwd(), so the path is
// correct regardless of where `playwright test` is invoked from -- it
// always lands at e2e/test-results/smoke.png, matching
// .github/workflows/e2e-linux.yml's "Upload smoke screenshot" step
// (artifact `pane-smoke-screenshot`, path `e2e/test-results/smoke.png`)
// character-for-character. Playwright's own default `outputDir` is not
// used here because it nests per-test subfolders; the workflow uploads a
// single fixed path, so the test writes to that fixed path directly.
const SCREENSHOT_PATH = path.join(
  path.dirname(fileURLToPath(import.meta.url)),
  '..',
  'test-results',
  'smoke.png',
);

// PNG decode/validate logic lives in e2e/helpers/screenshot.ts (#470 PR 3
// extraction) -- the shared helper every journey will import, per the
// convention posted on issue #470 (comment 7169240575). No second copy of
// the decoder here.

test('Pane app loads and shows content', async ({ tauriPage }) => {
  // Verify the app loads at all
  const title = await tauriPage.title();
  console.log('Page title:', title);

  // Check if any visible text exists on the page
  const bodyText = await tauriPage.textContent('body');
  console.log('Body text length:', bodyText?.length);

  // Take a screenshot for visual verification and validate it from disk
  // (never from the returned buffer -- tauri-plugin-playwright 0.4.1
  // returns an empty buffer whenever a `path` is passed, see the helper's
  // own doc comment). The workflow's "Upload smoke screenshot" step
  // uploads this exact path as the pane-smoke-screenshot artifact.
  const { onDisk, variation } = await captureAndValidateScreenshot(tauriPage, SCREENSHOT_PATH);
  // Printed from the on-disk read, not the (always-empty-when-path-is-
  // passed) buffer screenshot() returns -- this line used to read
  // "bytes: 0" on every successful run, which looked like a failure.
  console.log('Screenshot captured: on-disk bytes:', onDisk.length);
  console.log('Screenshot variation:', JSON.stringify(variation));
});

test('Pane sidebar is visible', async ({ tauriPage }) => {
  // Wait for the app to initialize
  await tauriPage.waitForSelector('.sidebar', 10000);
  const sidebarVisible = await tauriPage.isVisible('.sidebar');
  console.log('Sidebar visible:', sidebarVisible);
});
