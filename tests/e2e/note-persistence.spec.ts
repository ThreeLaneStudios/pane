import { test, expect } from '@playwright/test';
import { setupTestProject } from './fixtures';

/**
 * Test: Note Persistence
 * Creates 3 notes with distinct content, switches between them, 
 * and verifies content persists correctly across switches.
 */
test.describe('Note Persistence', () => {
  test.beforeEach(async ({ page }) => {
    await setupTestProject(page);
  });

  test('should maintain distinct content when switching between notes', async ({ page }) => {
    // Test data
    const noteData = [
      { title: 'Alpha Note', content: 'This is the alpha note content with unique text.' },
      { title: 'Beta Note', content: 'Beta note contains different information entirely.' },
      { title: 'Gamma Note', content: 'Gamma note has its own special content that should not mix.' }
    ];

    // Create the first note
    await page.click('[data-testid="new-note-btn"]');
    await page.fill('[data-testid="note-title-input"]', noteData[0].title);
    await page.click('[data-testid="note-editor"] .ProseMirror');
    await page.keyboard.type(noteData[0].content);
    await page.waitForTimeout(1000); // Wait for autosave

    // Create the second note
    await page.click('[data-testid="new-note-btn"]');
    await page.fill('[data-testid="note-title-input"]', noteData[1].title);
    await page.click('[data-testid="note-editor"] .ProseMirror');
    await page.keyboard.type(noteData[1].content);
    await page.waitForTimeout(1000); // Wait for autosave

    // Create the third note
    await page.click('[data-testid="new-note-btn"]');
    await page.fill('[data-testid="note-title-input"]', noteData[2].title);
    await page.click('[data-testid="note-editor"] .ProseMirror');
    await page.keyboard.type(noteData[2].content);
    await page.waitForTimeout(1000); // Wait for autosave

    // Now verify content persistence by switching between notes
    
    // Click on Alpha note in the sidebar
    await page.click(`[data-testid="note-item"]:has-text("${noteData[0].title}")`);
    await page.waitForTimeout(500);
    
    // Verify Alpha content is displayed
    const alphaContent = await page.textContent('[data-testid="note-editor"] .ProseMirror');
    expect(alphaContent).toContain(noteData[0].content);
    expect(alphaContent).not.toContain(noteData[1].content);
    expect(alphaContent).not.toContain(noteData[2].content);

    // Click on Beta note
    await page.click(`[data-testid="note-item"]:has-text("${noteData[1].title}")`);
    await page.waitForTimeout(500);
    
    // Verify Beta content is displayed
    const betaContent = await page.textContent('[data-testid="note-editor"] .ProseMirror');
    expect(betaContent).toContain(noteData[1].content);
    expect(betaContent).not.toContain(noteData[0].content);
    expect(betaContent).not.toContain(noteData[2].content);

    // Click on Gamma note
    await page.click(`[data-testid="note-item"]:has-text("${noteData[2].title}")`);
    await page.waitForTimeout(500);
    
    // Verify Gamma content is displayed
    const gammaContent = await page.textContent('[data-testid="note-editor"] .ProseMirror');
    expect(gammaContent).toContain(noteData[2].content);
    expect(gammaContent).not.toContain(noteData[0].content);
    expect(gammaContent).not.toContain(noteData[1].content);

    // Switch back to Alpha to ensure it's still correct
    await page.click(`[data-testid="note-item"]:has-text("${noteData[0].title}")`);
    await page.waitForTimeout(500);
    
    const finalAlphaContent = await page.textContent('[data-testid="note-editor"] .ProseMirror');
    expect(finalAlphaContent).toContain(noteData[0].content);
    expect(finalAlphaContent).not.toContain(noteData[1].content);
    expect(finalAlphaContent).not.toContain(noteData[2].content);
  });

  test('should handle empty notes without content mixing', async ({ page }) => {
    // Create a note with content
    await page.click('[data-testid="new-note-btn"]');
    await page.fill('[data-testid="note-title-input"]', 'Content Note');
    await page.click('[data-testid="note-editor"] .ProseMirror');
    await page.keyboard.type('This note has content.');
    await page.waitForTimeout(1000);

    // Create an empty note
    await page.click('[data-testid="new-note-btn"]');
    await page.fill('[data-testid="note-title-input"]', 'Empty Note');
    // Don't add content - leave it empty
    await page.waitForTimeout(1000);

    // Switch back to content note
    await page.click('[data-testid="note-item"]:has-text("Content Note")');
    await page.waitForTimeout(500);

    // Verify content note still has its content
    const contentNoteText = await page.textContent('[data-testid="note-editor"] .ProseMirror');
    expect(contentNoteText).toContain('This note has content.');

    // Switch to empty note
    await page.click('[data-testid="note-item"]:has-text("Empty Note")');
    await page.waitForTimeout(500);

    // Verify empty note is actually empty (or just has title)
    const emptyNoteText = await page.textContent('[data-testid="note-editor"] .ProseMirror');
    expect(emptyNoteText?.trim() || '').not.toContain('This note has content.');
  });
});