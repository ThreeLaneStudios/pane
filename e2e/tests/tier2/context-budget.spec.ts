import { test, expect } from '../../fixtures';

test.describe('T2.4: Context Budget Management', () => {
  test('should handle multiple rapid messages without crashing', async ({ tauriPage }) => {
    console.log('[CONTEXT-BUDGET] Testing context budget management...');
    
    // Wait for app to fully load
    await tauriPage.waitForSelector('[data-testid="app-loaded"]', 10000);
    console.log('[CONTEXT-BUDGET] ✓ App loaded');
    
    // Check if Ollama is configured
    const ollamaCheck = await tauriPage.evaluate(`
      fetch('http://localhost:11434/api/tags')
        .then(r => r.ok)
        .catch(() => false)
    `);
    
    if (!ollamaCheck) {
      console.log('[CONTEXT-BUDGET] ⚠ Ollama not available — skipping test');
      test.skip();
      return;
    }
    console.log('[CONTEXT-BUDGET] ✓ Ollama available');
    
    // Locate chat input
    const chatInput = 'textarea.message-input';
    await tauriPage.waitForSelector(chatInput, 5000);
    console.log('[CONTEXT-BUDGET] ✓ Chat input found');
    
    // Send 12 rapid messages
    const messageCount = 12;
    console.log(`[CONTEXT-BUDGET] Sending ${messageCount} rapid messages...`);
    
    for (let i = 1; i <= messageCount; i++) {
      await tauriPage.fill(chatInput, `Message number ${i}`);
      await tauriPage.press(chatInput, 'Enter');
      console.log(`[CONTEXT-BUDGET] ✓ Sent message ${i}/${messageCount}`);
      
      // Brief pause between sends to avoid overwhelming the UI
      await new Promise(resolve => setTimeout(resolve, 500));
    }
    
    console.log('[CONTEXT-BUDGET] All messages sent, waiting for responses...');
    
    // Wait for at least some responses to arrive (not all — AI is slow)
    // We should have at least 3-4 responses after a reasonable wait
    await new Promise(resolve => setTimeout(resolve, 15000));
    
    // Verify the app didn't crash
    const appStillLoaded = await tauriPage.isVisible('[data-testid="app-loaded"]');
    expect(appStillLoaded).toBeTruthy();
    console.log('[CONTEXT-BUDGET] ✓ App still loaded');
    
    // Verify chat input is still functional
    const inputEnabled = await tauriPage.evaluate(`
      !document.querySelector('textarea.message-input')?.disabled
    `);
    expect(inputEnabled).toBeTruthy();
    console.log('[CONTEXT-BUDGET] ✓ Chat input still enabled');
    
    // Check message count (user messages)
    const userMessageCount = await tauriPage.count('.message.user');
    expect(userMessageCount).toBe(messageCount);
    console.log(`[CONTEXT-BUDGET] ✓ All ${messageCount} user messages visible`);
    
    // Verify messages are scrollable
    const messagesArea = '.messages-area';
    const isScrollable = await tauriPage.evaluate(`
      const el = document.querySelector('${messagesArea}');
      el && el.scrollHeight > el.clientHeight
    `);
    console.log('[CONTEXT-BUDGET] Messages area scrollable:', isScrollable);
    
    // Scroll to top and verify older messages are accessible
    await tauriPage.evaluate(`
      document.querySelector('${messagesArea}')?.scrollTo(0, 0)
    `);
    
    // Wait a moment for scroll to complete
    await new Promise(resolve => setTimeout(resolve, 500));
    
    // Verify first message is visible after scroll
    const firstMessageVisible = await tauriPage.isVisible('.message.user:first-of-type');
    expect(firstMessageVisible).toBeTruthy();
    console.log('[CONTEXT-BUDGET] ✓ Older messages accessible via scroll');
    
    // Verify we got at least some AI responses
    const assistantMessageCount = await tauriPage.count('.message.assistant');
    expect(assistantMessageCount).toBeGreaterThan(0);
    console.log(`[CONTEXT-BUDGET] ✓ Received ${assistantMessageCount} AI responses`);
    
    console.log('[CONTEXT-BUDGET] Context budget test passed ✓');
  });
});
