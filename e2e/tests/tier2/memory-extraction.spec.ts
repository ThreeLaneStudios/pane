import { test, expect } from '../../fixtures';

test.describe('T2.2: Memory Extraction', () => {
  test('should extract facts from conversation and recall them', async ({ tauriPage }) => {
    console.log('[MEMORY-EXTRACT] Testing memory extraction pipeline...');
    
    // Wait for app to fully load
    await tauriPage.waitForSelector('[data-testid="app-loaded"]', 10000);
    console.log('[MEMORY-EXTRACT] ✓ App loaded');
    
    // Check if Ollama is configured
    const ollamaCheck = await tauriPage.evaluate(`
      fetch('http://localhost:11434/api/tags')
        .then(r => r.ok)
        .catch(() => false)
    `);
    
    if (!ollamaCheck) {
      console.log('[MEMORY-EXTRACT] ⚠ Ollama not available — skipping test');
      test.skip();
      return;
    }
    console.log('[MEMORY-EXTRACT] ✓ Ollama available');
    
    // Locate chat input
    const chatInput = 'textarea.message-input';
    await tauriPage.waitForSelector(chatInput, 5000);
    console.log('[MEMORY-EXTRACT] ✓ Chat input found');
    
    // Send a message with extractable facts
    const factMessage = 'My favorite programming language is Rust and I started learning it in January 2025';
    console.log('[MEMORY-EXTRACT] Sending message with facts:', factMessage);
    await tauriPage.fill(chatInput, factMessage);
    await tauriPage.press(chatInput, 'Enter');
    
    // Wait for initial response
    await tauriPage.waitForSelector('.message.assistant .message-content', 30000);
    console.log('[MEMORY-EXTRACT] ✓ Got initial response');
    
    // Wait for extraction pipeline to complete (15-20s as per spec)
    console.log('[MEMORY-EXTRACT] Waiting for extraction pipeline (20s)...');
    await new Promise(resolve => setTimeout(resolve, 20000));
    
    // Now ask a question that should trigger memory recall
    console.log('[MEMORY-EXTRACT] Asking recall question...');
    await tauriPage.fill(chatInput, "What's my favorite programming language?");
    await tauriPage.press(chatInput, 'Enter');
    
    // Wait for recall response (should be faster since it's a simple question)
    await tauriPage.waitForSelector('.message.assistant:nth-of-type(2)', 30000);
    
    // Get the latest response text
    const recallResponse = await tauriPage.evaluate(`
      const messages = Array.from(document.querySelectorAll('.message.assistant .message-content'));
      messages[messages.length - 1].textContent.trim()
    `);
    
    console.log('[MEMORY-EXTRACT] Recall response:', recallResponse);
    
    // Verify the response mentions Rust (case-insensitive)
    const mentionsRust = recallResponse.toLowerCase().includes('rust');
    expect(mentionsRust).toBeTruthy();
    console.log('[MEMORY-EXTRACT] ✓ Response correctly recalled "Rust"');
    
    console.log('[MEMORY-EXTRACT] Memory extraction test passed ✓');
  });
});
