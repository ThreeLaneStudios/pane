import { test as base, expect } from '@playwright/test';
import { PluginClient, TauriPage } from '@srsholmes/tauri-playwright';

type TauriTestFixtures = {
  tauriPage: TauriPage;
  client: PluginClient;
};

export const test = base.extend<TauriTestFixtures>({
  client: async ({}, use) => {
    console.log('[FIXTURE] Creating PluginClient...');
    const client = new PluginClient('/tmp/tauri-playwright.sock');
    await client.connect();
    console.log('[FIXTURE] PluginClient connected');
    await use(client);
    await client.disconnect();
    console.log('[FIXTURE] PluginClient disconnected');
  },
  
  tauriPage: async ({ client }, use) => {
    console.log('[FIXTURE] Creating TauriPage...');
    const page = new TauriPage(client);
    
    // ADR-0070 (#3195 PR B): the __PANE_TEST_MODE__ bypass this fixture
    // used to inject is removed with the AuthScreen gate it existed to
    // satisfy — the harness boots to Home the same way production does.
    // Wait briefly for the app to finish its (now network-free) auth init.
    await new Promise(r => setTimeout(r, 2000));
    console.log('[FIXTURE] Ready for tests');
    
    await use(page);
    console.log('[FIXTURE] TauriPage fixture cleanup complete');
  },
});

export { expect };
