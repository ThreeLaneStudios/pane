import { test, expect } from '@playwright/test';
import { setupTestProject } from './fixtures';

/**
 * Test: Rapid Note Switching
 * Creates 5 notes, clicks through them rapidly (<500ms), waits for saves to settle,
 * then verifies all content is correct. This catches race conditions.
 */
test.describe('Rapid Note Switching', () => {
  test.beforeEach(async ({ page }) => {
    await setupTestProject(page);
  });

  test('should handle rapid clicking without content mixing or corruption', async ({ page }) => {
    // Test data - 5 notes with very distinct content
    const notes = [
      { title: 'Rapid Note 1', content: 'UNIQUE_CONTENT_ONE: This is the first rapid test note with specific markers.' },
      { title: 'Rapid Note 2', content: 'UNIQUE_CONTENT_TWO: Second note contains completely different text patterns.' },
      { title: 'Rapid Note 3', content: 'UNIQUE_CONTENT_THREE: Third note has its own distinctive textual signature.' },
      { title: 'Rapid Note 4', content: 'UNIQUE_CONTENT_FOUR: Fourth note brings unique content that should never leak.' },
      { title: 'Rapid Note 5', content: 'UNIQUE_CONTENT_FIVE: Fifth and final note with unmistakable content identity.' }
    ];

    // Phase 1: Create all notes
    console.log('Creating test notes...');
    for (const note of notes) {
      await page.click('[data-testid="new-note-btn"]');
      await page.waitForTimeout(200); // Brief wait for UI to settle
      
      // Fill title
      await page.fill('[data-testid="note-title-input"]', note.title);
      
      // Fill content
      await page.click('[data-testid="note-editor"] .ProseMirror');
      await page.keyboard.type(note.content);
      
      // Wait for autosave to trigger
      await page.waitForTimeout(800);
      
      console.log(`Created: ${note.title}`);
    }

    // Phase 2: Rapid switching test
    console.log('Starting rapid switching test...');
    
    // Perform rapid clicks (< 500ms between clicks)
    for (let i = 0; i < notes.length; i++) {
      await page.click(`[data-testid="note-item"]:has-text("${notes[i].title}")`);
      await page.waitForTimeout(300); // Rapid switching - less than autosave debounce
    }

    // Reverse direction rapid switching
    for (let i = notes.length - 1; i >= 0; i--) {
      await page.click(`[data-testid="note-item"]:has-text("${notes[i].title}")`);
      await page.waitForTimeout(200); // Even faster switching
    }

    // Phase 3: Wait for all saves to settle
    console.log('Waiting for saves to settle...');
    await page.waitForTimeout(3000); // Wait for all autosaves to complete

    // Phase 4: Verify content integrity (slow, methodical verification)
    console.log('Verifying content integrity...');
    
    for (let i = 0; i < notes.length; i++) {
      // Click on the note
      await page.click(`[data-testid="note-item"]:has-text("${notes[i].title}")`);
      
      // Wait for content to load
      await page.waitForTimeout(1000);
      
      // Get the actual content
      const actualContent = await page.textContent('[data-testid="note-editor"] .ProseMirror');
      
      // Verify this note has its own content
      expect(actualContent).toContain(notes[i].content);
      expect(actualContent).toContain(`UNIQUE_CONTENT_${['ONE', 'TWO', 'THREE', 'FOUR', 'FIVE'][i]}`);
      
      // Verify it doesn't contain content from other notes
      for (let j = 0; j < notes.length; j++) {
        if (i !== j) {
          expect(actualContent).not.toContain(`UNIQUE_CONTENT_${['ONE', 'TWO', 'THREE', 'FOUR', 'FIVE'][j]}`);
        }
      }
      
      console.log(`✓ Note ${i + 1} content verified correctly`);
    }
  });

  test('should handle mixed typing and rapid switching', async ({ page }) => {
    // Create initial notes
    const notes = [
      { title: 'Type Test 1', content: 'Original content one' },
      { title: 'Type Test 2', content: 'Original content two' },
      { title: 'Type Test 3', content: 'Original content three' }
    ];

    // Create the notes
    for (const note of notes) {
      await page.click('[data-testid="new-note-btn"]');
      await page.fill('[data-testid="note-title-input"]', note.title);
      await page.click('[data-testid="note-editor"] .ProseMirror');
      await page.keyboard.type(note.content);
      await page.waitForTimeout(1000);
    }

    // Rapid switch while typing
    await page.click(`[data-testid="note-item"]:has-text("${notes[0].title}")`);
    await page.waitForTimeout(100);
    
    // Start typing in note 1
    await page.click('[data-testid="note-editor"] .ProseMirror');
    await page.type('[data-testid="note-editor"] .ProseMirror', ' ADDITION_TO_NOTE_1');
    
    // Immediately switch to note 2 (before autosave)
    await page.click(`[data-testid="note-item"]:has-text("${notes[1].title}")`);
    await page.waitForTimeout(100);
    
    // Type in note 2
    await page.click('[data-testid="note-editor"] .ProseMirror');
    await page.type('[data-testid="note-editor"] .ProseMirror', ' ADDITION_TO_NOTE_2');
    
    // Immediately switch to note 3
    await page.click(`[data-testid="note-item"]:has-text("${notes[2].title}")`);
    await page.waitForTimeout(100);
    
    // Type in note 3
    await page.click('[data-testid="note-editor"] .ProseMirror');
    await page.type('[data-testid="note-editor"] .ProseMirror', ' ADDITION_TO_NOTE_3');

    // Wait for all saves to complete
    await page.waitForTimeout(3000);

    // Verify each note has only its own additions
    await page.click(`[data-testid="note-item"]:has-text("${notes[0].title}")`);
    await page.waitForTimeout(500);
    let content1 = await page.textContent('[data-testid="note-editor"] .ProseMirror');
    expect(content1).toContain('ADDITION_TO_NOTE_1');
    expect(content1).not.toContain('ADDITION_TO_NOTE_2');
    expect(content1).not.toContain('ADDITION_TO_NOTE_3');

    await page.click(`[data-testid="note-item"]:has-text("${notes[1].title}")`);
    await page.waitForTimeout(500);
    let content2 = await page.textContent('[data-testid="note-editor"] .ProseMirror');
    expect(content2).toContain('ADDITION_TO_NOTE_2');
    expect(content2).not.toContain('ADDITION_TO_NOTE_1');
    expect(content2).not.toContain('ADDITION_TO_NOTE_3');

    await page.click(`[data-testid="note-item"]:has-text("${notes[2].title}")`);
    await page.waitForTimeout(500);
    let content3 = await page.textContent('[data-testid="note-editor"] .ProseMirror');
    expect(content3).toContain('ADDITION_TO_NOTE_3');
    expect(content3).not.toContain('ADDITION_TO_NOTE_1');
    expect(content3).not.toContain('ADDITION_TO_NOTE_2');
  });
});