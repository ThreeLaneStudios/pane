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
    {
      name: 'smoke',
      testMatch: /smoke\.spec\.ts$/,
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
  ],
  
  reporter: [
    ['list'],
    ['html', { outputFolder: 'playwright-report' }],
  ],
});
