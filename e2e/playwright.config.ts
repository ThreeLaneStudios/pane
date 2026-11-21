import { defineConfig } from '@playwright/test';

export default defineConfig({
  timeout: 30000,
  globalTimeout: 5 * 60 * 1000, // 5 minutes total
  testDir: './tests',
  
  projects: [
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
