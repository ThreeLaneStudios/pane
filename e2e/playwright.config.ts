import { defineConfig } from '@playwright/test';

export default defineConfig({
  timeout: 30000,
  globalTimeout: 5 * 60 * 1000, // 5 minutes total
  testDir: './tests',
  
  projects: [
    // #470 PR 2: lets the existing e2e/tests/smoke.spec.ts (not under any
    // tier*/ testDir, so untouched by the tier1/2/3 projects below) run via
    // `--project=smoke` from .github/workflows/e2e-linux.yml. No new test
    // content -- this only makes the pre-existing file discoverable.
    //
    // #470 (fonts PR): e2e/tests/fonts.spec.ts added to this same
    // testMatch -- a CI/environment-correctness proof (is the font stack
    // actually resolving to Inter?), same category as the existing
    // "Pane app loads"/"Pane sidebar is visible" checks, not journey
    // content. Runs via the same already-existing `--project=smoke` step
    // in e2e-linux.yml -- no new workflow step needed.
    {
      name: 'smoke',
      testMatch: /(smoke|fonts)\.spec\.ts$/,
      timeout: 30000,
    },
    {
      name: 'tier1',
      testDir: './tests/tier1',
      timeout: 30000,
    },
    {
      name: 'tier2',
      testDir: './tests/tier2',
      timeout: 45000,
    },
    {
      name: 'tier3',
      testDir: './tests/tier3',
      timeout: 60000,
    },
    // #470 PR 4: journeys live under their own testDir (e2e/journeys/),
    // sibling to tests/ -- a journey is a *.journey.ts file, never swept
    // into tier1/2/3 by that directory split. retries: 1 is Matt's
    // ruling (journey-level retry limit) -- step-level flake is already
    // absorbed by TauriPage's own auto-waiting click/fill; this is a
    // whole-journey safety net, not a new per-step mechanism.
    {
      name: 'journeys',
      testDir: './journeys',
      testMatch: /\.journey\.ts$/,
      timeout: 60000,
      retries: 1,
    },
  ],
  
  reporter: [
    ['list'],
    ['html', { outputFolder: 'playwright-report' }],
  ],
});
