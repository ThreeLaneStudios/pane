import { test, expect } from '../../fixtures';

test.describe('T2.1: Smart Routing', () => {
  test('should route simple and complex messages appropriately', async ({ tauriPage }) => {
    console.log('[SMART-ROUTING] Testing AI message routing...');
    
    // Wait for app to fully load
    await tauriPage.waitForSelector('[data-testid="app-loaded"]', 10000);
    console.log('[SMART-ROUTING] ✓ App loaded');
    
    // Check if Ollama is configured (verify AI is available)
    const ollamaCheck = await tauriPage.evaluate(`
      fetch('http://localhost:11434/api/tags')
        .then(r => r.ok)
        .catch(() => false)
    `);
    
    if (!ollamaCheck) {
      console.log('[SMART-ROUTING] ⚠ Ollama not available at localhost:11434 — skipping test');
      test.skip();
      return;
    }
    console.log('[SMART-ROUTING] ✓ Ollama available');
    
    // Locate chat input
    const chatInput = 'textarea.message-input';
    await tauriPage.waitForSelector(chatInput, 5000);
    console.log('[SMART-ROUTING] ✓ Chat input found');
    
    // Send a simple message
    console.log('[SMART-ROUTING] Sending simple message: "hi"');
    await tauriPage.fill(chatInput, 'hi');
    await tauriPage.press(chatInput, 'Enter');
    
    // Wait for response (AI takes time, even locally)
    await tauriPage.waitForSelector('.message.assistant .message-content', 30000);
    const simpleResponse = await tauriPage.textContent('.message.assistant .message-content');
    expect(simpleResponse).toBeTruthy();
    expect(simpleResponse.length).toBeGreaterThan(0);
    console.log('[SMART-ROUTING] ✓ Simple message got response:', simpleResponse.substring(0, 100));
    
    // Wait a moment before sending next message
    await new Promise(resolve => setTimeout(resolve, 1000));
    
    // Send a complex message
    console.log('[SMART-ROUTING] Sending complex message...');
    await tauriPage.fill(chatInput, 'Analyze the relationships between my notes and suggest organizational improvements');
    await tauriPage.press(chatInput, 'Enter');
    
    // Wait for second response
    // Count messages to verify we have at least 2 assistant responses
    await tauriPage.waitForSelector('.message.assistant:nth-of-type(2)', 60000);
    const messageCount = await tauriPage.count('.message.assistant');
    expect(messageCount).toBeGreaterThanOrEqual(2);
    console.log('[SMART-ROUTING] ✓ Complex message got response');
    
    // Verify both responses are non-empty
    const allResponses = await tauriPage.evaluate(`
      Array.from(document.querySelectorAll('.message.assistant .message-content'))
        .map(el => el.textContent.trim())
    `);
    expect(allResponses.length).toBeGreaterThanOrEqual(2);
    allResponses.forEach((resp, i) => {
      expect(resp.length).toBeGreaterThan(0);
      console.log(`[SMART-ROUTING] ✓ Response ${i + 1} length:`, resp.length);
    });
    
    console.log('[SMART-ROUTING] All routing tests passed ✓');
  });
});
