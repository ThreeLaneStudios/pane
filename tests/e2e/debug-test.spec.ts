import { test, expect } from '@playwright/test';
import { setupTestProject } from './fixtures';

test('debug: note switching', async ({ page }) => {
  page.on('console', msg => {
    const text = msg.text();
    if (text.includes('EDITOR') || text.includes('NOTE-S') || text.includes('ERROR')) {
      console.log(`[B]`, text);
    }
  });
  page.on('pageerror', err => console.log(`[PAGE-ERROR]`, err.message));
  
  await setupTestProject(page);
  
  // Create note A
  await page.click('[data-testid="new-note-btn"]');
  await page.waitForTimeout(500);
  await page.fill('[data-testid="note-title-input"]', 'Alpha Note');
  await page.click('[data-testid="note-editor"] .ProseMirror');
  await page.keyboard.type('ALPHA_CONTENT');
  await page.waitForTimeout(1500);
  
  // Create note B  
  await page.click('[data-testid="new-note-btn"]');
  await page.waitForTimeout(500);
  await page.fill('[data-testid="note-title-input"]', 'Beta Note');
  await page.click('[data-testid="note-editor"] .ProseMirror');
  await page.keyboard.type('BETA_CONTENT');
  await page.waitForTimeout(1500);
  
  console.log('--- SWITCHING TO ALPHA ---');
  
  // Click Alpha in note list
  const items = await page.locator('[data-testid="note-item"]').all();
  console.log('Note items count:', items.length);
  for (const item of items) {
    const text = await item.textContent();
    console.log('Note item text:', text?.substring(0, 50));
  }
  
  await page.locator('[data-testid="note-item"]:has-text("Alpha")').click();
  await page.waitForTimeout(2000);
  
  // Check title input value
  const titleVal = await page.inputValue('[data-testid="note-title-input"]');
  console.log('Title after switch:', titleVal);
  
  const content = await page.textContent('[data-testid="note-editor"] .ProseMirror');
  console.log('Content after switch:', content);
  
  // Check if the editor was recreated by looking at the key
  const noteId = await page.evaluate(() => {
    const el = document.querySelector('[data-testid="note-title-input"]');
    return el ? (el as HTMLInputElement).value : 'NOT FOUND';
  });
  console.log('Note title from DOM:', noteId);
  
  expect(content).toContain('ALPHA_CONTENT');
});
