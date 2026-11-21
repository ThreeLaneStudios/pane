import type { Page } from '@playwright/test';
import { getTauriMockScript } from './tauri-mock';

/**
 * Set up test environment with Tauri mocks and navigate to FolderView.
 */
export async function setupTestProject(page: Page) {
  const mockScript = getTauriMockScript();
  const seedScript = `
    ${mockScript}
    
    // Pre-seed a root project and subfolder
    const now = new Date().toISOString();
    const NOTEBOOK = 'Test Project/Notes';
    window.__TAURI_INTERNALS__._mockData.projects = [
      {
        id: 'proj_root', name: 'Test Project', description: null, color: null,
        parent_id: null, sort_order: 0, created_at: now, updated_at: now, archived_at: null,
      },
      {
        id: 'proj_folder', name: 'Notes', description: null, color: null,
        parent_id: 'proj_root', sort_order: 0, created_at: now, updated_at: now, archived_at: null,
      }
    ];
    // Store notes under every possible notebook key the app might use
    window.__TAURI_INTERNALS__._mockData.notebooks = ['Test Project', 'Notes', NOTEBOOK];
    window.__TAURI_INTERNALS__._mockData.notes['Notes'] = [];
    window.__TAURI_INTERNALS__._mockData.notes[NOTEBOOK] = [];
    
    // Make notes stored under 'Notes' also visible under 'Test Project/Notes' and vice versa
    // by using a shared array reference
    const sharedNotes = [];
    window.__TAURI_INTERNALS__._mockData.notes['Notes'] = sharedNotes;
    window.__TAURI_INTERNALS__._mockData.notes[NOTEBOOK] = sharedNotes;
    
    // Pin the root project
    try {
      localStorage.setItem('pane_pinned_projects', JSON.stringify(['proj_root']));
    } catch(e) {}
  `;
  
  await page.addInitScript({ content: seedScript });
  await page.goto('http://localhost:5173');
  
  // Wait for the app shell to load
  await page.waitForSelector('[data-testid="app-loaded"]', { timeout: 15000 });
  
  // Wait for sidebar to load projects
  await page.waitForTimeout(1000);
  
  // Expand the project tree
  const expandBtn = page.locator('button:has-text("▸")').first();
  if (await expandBtn.isVisible({ timeout: 2000 }).catch(() => false)) {
    await expandBtn.click();
    await page.waitForTimeout(500);
  }
  
  // Click on the "Notes" subfolder
  const folderBtn = page.locator('button:has-text("Notes")').first();
  await folderBtn.waitFor({ state: 'visible', timeout: 5000 });
  await folderBtn.click();
  
  // Wait for FolderView to load
  await page.waitForSelector('[data-testid="new-note-btn"]', { timeout: 10000 });
}
