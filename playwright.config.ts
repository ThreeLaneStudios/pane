import { defineConfig, devices } from '@playwright/test';

/**
 * @see https://playwright.dev/docs/test-configuration
 */
export default defineConfig({
  testDir: './tests/e2e',
  /* Run tests in files in parallel */
  fullyParallel: true,
  /* Fail the build on CI if you accidentally left test.only in the source code. */
  forbidOnly: !!process.env.CI,
  /* Retry on CI only */
  retries: process.env.CI ? 2 : 0,
  /* Opt out of parallel tests on CI. */
  workers: process.env.CI ? 1 : undefined,
  /* Reporter to use. See https://playwright.dev/docs/test-reporters */
  reporter: 'html',
  /* Shared settings for all the projects below. See https://playwright.dev/docs/api/class-testoptions. */
  use: {
    /* Base URL to use in actions like `await page.goto('/')`. */
    // baseURL: 'http://127.0.0.1:3000',

    /* Collect trace when retrying the failed test. See https://playwright.dev/docs/trace-viewer */
    trace: 'on-first-retry',
  },

  /* Configure projects for major browsers */
  projects: [
    {
      name: 'chromium',
      use: { ...devices['Desktop Chrome'] },
      // #3182: the two WebKit-only specs below assert real computed
      // CSS layout that depends on the rendering engine (C27's own
      // history is why engine choice is named explicitly) — excluded
      // here so a chromium pass/fail on them never gets read as
      // confirming or denying anything about the WebKit-specific claim.
      testIgnore: [
        'c64-narrow-layout-restore.spec.ts',
        'c57-inline-list-height-cap.spec.ts',
        'c65-sidebar-overlay-visual.spec.ts',
      ],
    },

    // NOTE: For Tauri apps, we'll need to configure this differently
    // Tauri apps run as desktop applications, not web browsers
    // This config is a starting point - actual Tauri integration may require:
    // - Using tauri-driver or similar
    // - Connecting to the Tauri webview via custom setup
    // - Different launch/connection approach

    /* 
    {
      name: 'firefox',
      use: { ...devices['Desktop Firefox'] },
    },
    */

    // #3182: WebKit specifically (not Chromium) for the two specs that
    // assert real computed CSS layout Tauri-on-macOS actually renders
    // with (WKWebView) — C27's own history is the reason engine choice
    // gets named explicitly rather than left to whatever's default.
    // Scoped via testMatch to just these two files so the rest of this
    // directory's (pre-existing, chromium-only) specs are unaffected.
    {
      name: 'webkit',
      use: { ...devices['Desktop Safari'] },
      testMatch: [
        'c64-narrow-layout-restore.spec.ts',
        'c57-inline-list-height-cap.spec.ts',
        'c65-sidebar-overlay-visual.spec.ts',
      ],
    },
  ],

  /* Run your local dev server before starting the tests */
  webServer: {
    command: 'pnpm dev',
    url: 'http://localhost:5173', // SvelteKit dev port
    reuseExistingServer: !process.env.CI,
    timeout: 60000,
    stdout: 'pipe',
    stderr: 'pipe',
  },
});