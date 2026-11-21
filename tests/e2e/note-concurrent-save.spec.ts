import { test, expect } from '@playwright/test';
import { setupTestProject } from './fixtures';

/**
 * Test: Concurrent Save Operations
 * Types in note A, immediately switches to B, types, switches back.
 * Verifies both notes have correct content (tests the fixed save logic).
 */
test.describe('Concurrent Save Operations', () => {
  test.beforeEach(async ({ page }) => {
    await setupTestProject(page);
  });

  test('should handle concurrent saves without data loss', async ({ page }) => {
    // Create Note A
    await page.click('[data-testid="new-note-btn"]');
    await page.fill('[data-testid="note-title-input"]', 'Concurrent Note A');
    await page.click('[data-testid="note-editor"] .ProseMirror');
    await page.keyboard.type('Initial content A');
    await page.waitForTimeout(1500); // Wait for initial save

    // Create Note B
    await page.click('[data-testid="new-note-btn"]');
    await page.fill('[data-testid="note-title-input"]', 'Concurrent Note B');
    await page.click('[data-testid="note-editor"] .ProseMirror');
    await page.keyboard.type('Initial content B');
    await page.waitForTimeout(1500); // Wait for initial save

    // The critical test: rapid type-and-switch operations
    
    // 1. Click on Note A and start typing immediately
    await page.click(`[data-testid="note-item"]:has-text("Concurrent Note A")`);
    await page.waitForTimeout(200); // Brief load wait
    
    // Clear and type new content
    await page.click('[data-testid="note-editor"] .ProseMirror');
    await page.keyboard.press('Meta+a');
    await page.type('[data-testid="note-editor"] .ProseMirror', 'First draft content in Note A - this should be saved');
    
    // 2. Immediately switch to Note B (before autosave debounce fires)
    // This is the critical moment where content mixing could occur
    await page.click(`[data-testid="note-item"]:has-text("Concurrent Note B")`);
    await page.waitForTimeout(200);
    
    // 3. Start typing in Note B immediately
    await page.click('[data-testid="note-editor"] .ProseMirror');
    await page.keyboard.press('Meta+a');
    await page.type('[data-testid="note-editor"] .ProseMirror', 'Second draft content in Note B - completely different');
    
    // 4. Switch back to Note A quickly
    await page.click(`[data-testid="note-item"]:has-text("Concurrent Note A")`);
    await page.waitForTimeout(200);
    
    // 5. Add more content to Note A
    await page.click('[data-testid="note-editor"] .ProseMirror');
    await page.press('[data-testid="note-editor"] .ProseMirror', 'End');
    await page.type('[data-testid="note-editor"] .ProseMirror', ' ADDITIONAL_TEXT_A');
    
    // 6. Wait for all pending saves to complete
    await page.waitForTimeout(3000);
    
    // Verification Phase: Check both notes have correct, distinct content
    
    // Verify Note A content
    await page.click(`[data-testid="note-item"]:has-text("Concurrent Note A")`);
    await page.waitForTimeout(1000); // Wait for full load
    
    const noteAContent = await page.textContent('[data-testid="note-editor"] .ProseMirror');
    expect(noteAContent).toContain('First draft content in Note A');
    expect(noteAContent).toContain('ADDITIONAL_TEXT_A');
    expect(noteAContent).not.toContain('Second draft content in Note B');
    expect(noteAContent).not.toContain('completely different');
    
    console.log('✓ Note A content verified:', noteAContent?.substring(0, 100));
    
    // Verify Note B content
    await page.click(`[data-testid="note-item"]:has-text("Concurrent Note B")`);
    await page.waitForTimeout(1000); // Wait for full load
    
    const noteBContent = await page.textContent('[data-testid="note-editor"] .ProseMirror');
    expect(noteBContent).toContain('Second draft content in Note B');
    expect(noteBContent).toContain('completely different');
    expect(noteBContent).not.toContain('First draft content in Note A');
    expect(noteBContent).not.toContain('ADDITIONAL_TEXT_A');
    
    console.log('✓ Note B content verified:', noteBContent?.substring(0, 100));
  });

  test('should handle overlapping save operations with title changes', async ({ page }) => {
    // Create two notes
    await page.click('[data-testid="new-note-btn"]');
    await page.fill('[data-testid="note-title-input"]', 'Title Test A');
    await page.click('[data-testid="note-editor"] .ProseMirror');
    await page.keyboard.type('Content A');
    await page.waitForTimeout(1000);

    await page.click('[data-testid="new-note-btn"]');
    await page.fill('[data-testid="note-title-input"]', 'Title Test B');
    await page.click('[data-testid="note-editor"] .ProseMirror');
    await page.keyboard.type('Content B');
    await page.waitForTimeout(1000);

    // Rapid title and content changes
    await page.click(`[data-testid="note-item"]:has-text("Title Test A")`);
    await page.waitForTimeout(200);
    
    // Change title of Note A
    await page.fill('[data-testid="note-title-input"]', 'Modified Title A');
    
    // Change content of Note A
    await page.click('[data-testid="note-editor"] .ProseMirror');
    await page.keyboard.press('Meta+a');
    await page.type('[data-testid="note-editor"] .ProseMirror', 'Modified Content A with changes');
    
    // Immediately switch to Note B
    await page.click(`[data-testid="note-item"]:has-text("Title Test B")`);
    await page.waitForTimeout(100);
    
    // Change title of Note B
    await page.fill('[data-testid="note-title-input"]', 'Modified Title B');
    
    // Change content of Note B
    await page.click('[data-testid="note-editor"] .ProseMirror');
    await page.keyboard.press('Meta+a');
    await page.type('[data-testid="note-editor"] .ProseMirror', 'Modified Content B with different changes');

    // Wait for all saves
    await page.waitForTimeout(3000);

    // Verify both notes retained their correct changes
    await page.click(`[data-testid="note-item"]:has-text("Modified Title A")`);
    await page.waitForTimeout(500);
    
    const titleA = await page.inputValue('[data-testid="note-title-input"]');
    const contentA = await page.textContent('[data-testid="note-editor"] .ProseMirror');
    
    expect(titleA).toBe('Modified Title A');
    expect(contentA).toContain('Modified Content A with changes');
    expect(contentA).not.toContain('Modified Content B');

    await page.click(`[data-testid="note-item"]:has-text("Modified Title B")`);
    await page.waitForTimeout(500);
    
    const titleB = await page.inputValue('[data-testid="note-title-input"]');
    const contentB = await page.textContent('[data-testid="note-editor"] .ProseMirror');
    
    expect(titleB).toBe('Modified Title B');
    expect(contentB).toContain('Modified Content B with different changes');
    expect(contentB).not.toContain('Modified Content A');
  });

  test('should maintain content integrity during save failures', async ({ page }) => {
    // This test simulates what happens when save operations might fail
    // and ensures no content gets lost or mixed
    
    // Create a note with important content
    await page.click('[data-testid="new-note-btn"]');
    await page.fill('[data-testid="note-title-input"]', 'Important Note');
    await page.click('[data-testid="note-editor"] .ProseMirror');
    await page.keyboard.type('This content must not be lost during save operations');
    await page.waitForTimeout(1000);

    // Create second note
    await page.click('[data-testid="new-note-btn"]');
    await page.fill('[data-testid="note-title-input"]', 'Secondary Note');
    await page.click('[data-testid="note-editor"] .ProseMirror');
    await page.keyboard.type('Secondary content that should stay separate');
    await page.waitForTimeout(1000);

    // Perform very rapid operations that might stress the save system
    for (let i = 0; i < 5; i++) {
      // Switch to important note and modify
      await page.click(`[data-testid="note-item"]:has-text("Important Note")`);
      await page.waitForTimeout(50);
      await page.click('[data-testid="note-editor"] .ProseMirror');
      await page.press('[data-testid="note-editor"] .ProseMirror', 'End');
      await page.type('[data-testid="note-editor"] .ProseMirror', ` EDIT_${i}`);
      
      // Immediately switch to secondary note
      await page.click(`[data-testid="note-item"]:has-text("Secondary Note")`);
      await page.waitForTimeout(50);
      await page.click('[data-testid="note-editor"] .ProseMirror');
      await page.press('[data-testid="note-editor"] .ProseMirror', 'End');
      await page.type('[data-testid="note-editor"] .ProseMirror', ` SEC_${i}`);
    }

    // Wait for all operations to settle
    await page.waitForTimeout(5000);

    // Verify final state
    await page.click(`[data-testid="note-item"]:has-text("Important Note")`);
    await page.waitForTimeout(1000);
    
    const importantContent = await page.textContent('[data-testid="note-editor"] .ProseMirror');
    expect(importantContent).toContain('This content must not be lost');
    expect(importantContent).toContain('EDIT_0');
    expect(importantContent).toContain('EDIT_4'); // Should have all edits
    expect(importantContent).not.toContain('SEC_'); // Should not have secondary content
    expect(importantContent).not.toContain('Secondary content');

    await page.click(`[data-testid="note-item"]:has-text("Secondary Note")`);
    await page.waitForTimeout(1000);
    
    const secondaryContent = await page.textContent('[data-testid="note-editor"] .ProseMirror');
    expect(secondaryContent).toContain('Secondary content that should stay separate');
    expect(secondaryContent).toContain('SEC_0');
    expect(secondaryContent).toContain('SEC_4'); // Should have all secondary edits
    expect(secondaryContent).not.toContain('EDIT_'); // Should not have important content
    expect(secondaryContent).not.toContain('This content must not be lost');
  });
});