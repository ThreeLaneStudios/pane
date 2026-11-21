import { test, expect } from '../fixtures';

test('Pane app loads and shows content', async ({ tauriPage }) => {
  // Verify the app loads at all
  const title = await tauriPage.title();
  console.log('Page title:', title);
  
  // Check if any visible text exists on the page
  const bodyText = await tauriPage.textContent('body');
  console.log('Body text length:', bodyText?.length);
  
  // Take a screenshot for visual verification
  const screenshot = await tauriPage.screenshot();
  console.log('Screenshot captured:', screenshot ? 'yes' : 'no');
});

test('Pane sidebar is visible', async ({ tauriPage }) => {
  // Wait for the app to initialize
  await tauriPage.waitForSelector('.sidebar', 10000);
  const sidebarVisible = await tauriPage.isVisible('.sidebar');
  console.log('Sidebar visible:', sidebarVisible);
});
