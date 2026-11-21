import { test, expect } from '@playwright/test';
import { setupTestProject } from './fixtures';

/**
 * Test: Create/Delete Operations
 * Creates notes A+B, deletes A, verifies B is unaffected,
 * creates C, verifies B is still correct.
 */
test.describe('Create/Delete Operations', () => {
  test.beforeEach(async ({ page }) => {
    await setupTestProject(page);
  });

  test('should handle note deletion without affecting other notes', async ({ page }) => {
    // Create Note A
    await page.click('[data-testid="new-note-btn"]');
    await page.fill('[data-testid="note-title-input"]', 'Note A - To Delete');
    await page.click('[data-testid="note-editor"] .ProseMirror');
    await page.keyboard.type('Content A - This will be deleted');
    await page.waitForTimeout(1000); // Wait for save

    // Create Note B
    await page.click('[data-testid="new-note-btn"]');
    await page.fill('[data-testid="note-title-input"]', 'Note B - Keep This');
    await page.click('[data-testid="note-editor"] .ProseMirror');
    await page.keyboard.type('Content B - This should remain unchanged');
    await page.waitForTimeout(1000); // Wait for save

    // Verify both notes exist initially
    await expect(page.locator('[data-testid="note-item"]:has-text("Note A - To Delete")')).toBeVisible();
    await expect(page.locator('[data-testid="note-item"]:has-text("Note B - Keep This")')).toBeVisible();

    // Verify Note B content before deletion
    await page.click(`[data-testid="note-item"]:has-text("Note B - Keep This")`);
    await page.waitForTimeout(500);
    const noteBContentBefore = await page.textContent('[data-testid="note-editor"] .ProseMirror');
    expect(noteBContentBefore).toContain('Content B - This should remain unchanged');

    // Delete Note A
    await page.click(`[data-testid="note-item"]:has-text("Note A - To Delete")`);
    await page.waitForTimeout(300);
    
    // Right-click or use delete button (adjust based on UI implementation)
    await page.click('[data-testid="note-item"]:has-text("Note A - To Delete")', { button: 'right' });
    await page.click('[data-testid="delete-note-option"]');
    
    // Confirm deletion if there's a confirmation dialog
    try {
      await page.click('[data-testid="confirm-delete"]', { timeout: 2000 });
    } catch {
      // No confirmation dialog, deletion was immediate
    }

    await page.waitForTimeout(1000); // Wait for deletion to process

    // Verify Note A is deleted
    await expect(page.locator('[data-testid="note-item"]:has-text("Note A - To Delete")')).not.toBeVisible();

    // Verify Note B still exists and has correct content
    await expect(page.locator('[data-testid="note-item"]:has-text("Note B - Keep This")')).toBeVisible();
    
    await page.click(`[data-testid="note-item"]:has-text("Note B - Keep This")`);
    await page.waitForTimeout(500);
    
    const noteBContentAfter = await page.textContent('[data-testid="note-editor"] .ProseMirror');
    expect(noteBContentAfter).toContain('Content B - This should remain unchanged');
    expect(noteBContentAfter).not.toContain('Content A');
    expect(noteBContentAfter).not.toContain('This will be deleted');

    console.log('✓ Note B preserved correctly after A deletion');
  });

  test('should handle creation of new notes after deletion', async ({ page }) => {
    // Create Note A
    await page.click('[data-testid="new-note-btn"]');
    await page.fill('[data-testid="note-title-input"]', 'Original Note A');
    await page.click('[data-testid="note-editor"] .ProseMirror');
    await page.keyboard.type('Original content A');
    await page.waitForTimeout(1000);

    // Create Note B
    await page.click('[data-testid="new-note-btn"]');
    await page.fill('[data-testid="note-title-input"]', 'Original Note B');
    await page.click('[data-testid="note-editor"] .ProseMirror');
    await page.keyboard.type('Original content B - persistent');
    await page.waitForTimeout(1000);

    // Delete Note A
    await page.click(`[data-testid="note-item"]:has-text("Original Note A")`);
    await page.waitForTimeout(300);
    await page.click('[data-testid="note-item"]:has-text("Original Note A")', { button: 'right' });
    await page.click('[data-testid="delete-note-option"]');
    
    try {
      await page.click('[data-testid="confirm-delete"]', { timeout: 2000 });
    } catch {
      // No confirmation dialog
    }
    
    await page.waitForTimeout(1000);

    // Create Note C (new note after deletion)
    await page.click('[data-testid="new-note-btn"]');
    await page.fill('[data-testid="note-title-input"]', 'New Note C');
    await page.click('[data-testid="note-editor"] .ProseMirror');
    await page.keyboard.type('Fresh content C created after deletion');
    await page.waitForTimeout(1000);

    // Verify the current state:
    // - Note A should not exist
    // - Note B should have original content
    // - Note C should have new content

    // Check Note A is gone
    await expect(page.locator('[data-testid="note-item"]:has-text("Original Note A")')).not.toBeVisible();

    // Check Note B is still correct
    await page.click(`[data-testid="note-item"]:has-text("Original Note B")`);
    await page.waitForTimeout(500);
    
    const noteBContent = await page.textContent('[data-testid="note-editor"] .ProseMirror');
    expect(noteBContent).toContain('Original content B - persistent');
    expect(noteBContent).not.toContain('Original content A');
    expect(noteBContent).not.toContain('Fresh content C');

    // Check Note C has correct content
    await page.click(`[data-testid="note-item"]:has-text("New Note C")`);
    await page.waitForTimeout(500);
    
    const noteCContent = await page.textContent('[data-testid="note-editor"] .ProseMirror');
    expect(noteCContent).toContain('Fresh content C created after deletion');
    expect(noteCContent).not.toContain('Original content A');
    expect(noteCContent).not.toContain('Original content B');

    console.log('✓ New note creation works correctly after deletion');
  });

  test('should handle rapid create/delete operations', async ({ page }) => {
    const testNotes = [
      { title: 'Rapid Note 1', content: 'Content that will be deleted quickly' },
      { title: 'Rapid Note 2', content: 'Content that should persist through deletions' },
      { title: 'Rapid Note 3', content: 'Another note to delete' },
      { title: 'Rapid Note 4', content: 'Final persistent note' }
    ];

    // Create all notes rapidly
    for (const note of testNotes) {
      await page.click('[data-testid="new-note-btn"]');
      await page.fill('[data-testid="note-title-input"]', note.title);
      await page.click('[data-testid="note-editor"] .ProseMirror');
      await page.keyboard.type(note.content);
      await page.waitForTimeout(800);
    }

    // Verify all notes exist
    for (const note of testNotes) {
      await expect(page.locator(`[data-testid="note-item"]:has-text("${note.title}")`)).toBeVisible();
    }

    // Delete notes 1 and 3 (keep 2 and 4)
    const notesToDelete = [testNotes[0], testNotes[2]]; // Index 0 and 2
    
    for (const note of notesToDelete) {
      await page.click(`[data-testid="note-item"]:has-text("${note.title}")`);
      await page.waitForTimeout(200);
      await page.click(`[data-testid="note-item"]:has-text("${note.title}")`, { button: 'right' });
      await page.click('[data-testid="delete-note-option"]');
      
      try {
        await page.click('[data-testid="confirm-delete"]', { timeout: 1000 });
      } catch {
        // No confirmation
      }
      
      await page.waitForTimeout(500);
    }

    // Wait for all operations to settle
    await page.waitForTimeout(2000);

    // Verify deleted notes are gone
    await expect(page.locator('[data-testid="note-item"]:has-text("Rapid Note 1")')).not.toBeVisible();
    await expect(page.locator('[data-testid="note-item"]:has-text("Rapid Note 3")')).not.toBeVisible();

    // Verify remaining notes still exist and have correct content
    await page.click(`[data-testid="note-item"]:has-text("Rapid Note 2")`);
    await page.waitForTimeout(500);
    const note2Content = await page.textContent('[data-testid="note-editor"] .ProseMirror');
    expect(note2Content).toContain('Content that should persist through deletions');
    expect(note2Content).not.toContain('Content that will be deleted quickly');
    expect(note2Content).not.toContain('Another note to delete');

    await page.click(`[data-testid="note-item"]:has-text("Rapid Note 4")`);
    await page.waitForTimeout(500);
    const note4Content = await page.textContent('[data-testid="note-editor"] .ProseMirror');
    expect(note4Content).toContain('Final persistent note');
    expect(note4Content).not.toContain('Content that will be deleted quickly');
    expect(note4Content).not.toContain('Another note to delete');

    console.log('✓ Rapid create/delete operations handled correctly');
  });

  test('should handle deletion of currently active note', async ({ page }) => {
    // Create two notes
    await page.click('[data-testid="new-note-btn"]');
    await page.fill('[data-testid="note-title-input"]', 'Active Note');
    await page.click('[data-testid="note-editor"] .ProseMirror');
    await page.keyboard.type('This note is currently being edited');
    await page.waitForTimeout(1000);

    await page.click('[data-testid="new-note-btn"]');
    await page.fill('[data-testid="note-title-input"]', 'Backup Note');
    await page.click('[data-testid="note-editor"] .ProseMirror');
    await page.keyboard.type('This note should become active after deletion');
    await page.waitForTimeout(1000);

    // Make sure the Active Note is selected and we're editing it
    await page.click(`[data-testid="note-item"]:has-text("Active Note")`);
    await page.waitForTimeout(500);
    
    // Add some text to make sure we're actively editing
    await page.click('[data-testid="note-editor"] .ProseMirror');
    await page.press('[data-testid="note-editor"] .ProseMirror', 'End');
    await page.type('[data-testid="note-editor"] .ProseMirror', ' - actively editing this');
    
    // Now delete the currently active note
    await page.click('[data-testid="note-item"]:has-text("Active Note")', { button: 'right' });
    await page.click('[data-testid="delete-note-option"]');
    
    try {
      await page.click('[data-testid="confirm-delete"]', { timeout: 2000 });
    } catch {
      // No confirmation dialog
    }

    await page.waitForTimeout(1500); // Wait for deletion and UI update

    // Verify the active note is deleted
    await expect(page.locator('[data-testid="note-item"]:has-text("Active Note")')).not.toBeVisible();

    // Verify the backup note still exists and can be selected
    await expect(page.locator('[data-testid="note-item"]:has-text("Backup Note")')).toBeVisible();
    
    await page.click(`[data-testid="note-item"]:has-text("Backup Note")`);
    await page.waitForTimeout(500);
    
    const backupContent = await page.textContent('[data-testid="note-editor"] .ProseMirror');
    expect(backupContent).toContain('This note should become active after deletion');
    expect(backupContent).not.toContain('This note is currently being edited');
    expect(backupContent).not.toContain('actively editing this');

    console.log('✓ Active note deletion handled correctly');
  });
});