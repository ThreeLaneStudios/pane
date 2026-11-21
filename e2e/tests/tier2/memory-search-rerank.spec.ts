import { test, expect } from '../../fixtures';

test.describe('T2.3: Memory Search and Rerank', () => {
  test('should search and incorporate stored knowledge in responses', async ({ tauriPage }) => {
    console.log('[MEMORY-SEARCH] Testing memory search and reranking...');
    
    // Wait for app to fully load
    await tauriPage.waitForSelector('[data-testid="app-loaded"]', 10000);
    console.log('[MEMORY-SEARCH] ✓ App loaded');
    
    // Check if Ollama is configured
    const ollamaCheck = await tauriPage.evaluate(`
      fetch('http://localhost:11434/api/tags')
        .then(r => r.ok)
        .catch(() => false)
    `);
    
    if (!ollamaCheck) {
      console.log('[MEMORY-SEARCH] ⚠ Ollama not available — skipping test');
      test.skip();
      return;
    }
    console.log('[MEMORY-SEARCH] ✓ Ollama available');
    
    // Locate chat input
    const chatInput = 'textarea.message-input';
    await tauriPage.waitForSelector(chatInput, 5000);
    console.log('[MEMORY-SEARCH] ✓ Chat input found');
    
    // First, seed a memory fact (if not already from T2.2)
    console.log('[MEMORY-SEARCH] Seeding memory with facts...');
    await tauriPage.fill(chatInput, 'I live in San Francisco and work remotely as a software engineer');
    await tauriPage.press(chatInput, 'Enter');
    
    // Wait for response and extraction
    await tauriPage.waitForSelector('.message.assistant .message-content', 30000);
    console.log('[MEMORY-SEARCH] ✓ Initial fact stored, waiting for extraction...');
    await new Promise(resolve => setTimeout(resolve, 20000));
    
    // Ask a question that should trigger memory search
    console.log('[MEMORY-SEARCH] Asking question that requires memory search...');
    await tauriPage.fill(chatInput, 'What city do I live in?');
    await tauriPage.press(chatInput, 'Enter');
    
    // Wait for response
    await tauriPage.waitForSelector('.message.assistant:nth-of-type(2)', 30000);
    
    // Get the latest response
    const response = await tauriPage.evaluate(`
      const messages = Array.from(document.querySelectorAll('.message.assistant .message-content'));
      messages[messages.length - 1].textContent.trim()
    `);
    
    console.log('[MEMORY-SEARCH] Response:', response);
    
    // Verify response incorporates stored knowledge
    const mentionsSF = response.toLowerCase().includes('san francisco');
    expect(mentionsSF).toBeTruthy();
    console.log('[MEMORY-SEARCH] ✓ Response correctly recalled "San Francisco"');
    
    // Test a second query to verify reranking works with multiple relevant memories
    await new Promise(resolve => setTimeout(resolve, 1000));
    await tauriPage.fill(chatInput, 'Tell me about my work situation');
    await tauriPage.press(chatInput, 'Enter');
    
    await tauriPage.waitForSelector('.message.assistant:nth-of-type(3)', 30000);
    
    const workResponse = await tauriPage.evaluate(`
      const messages = Array.from(document.querySelectorAll('.message.assistant .message-content'));
      messages[messages.length - 1].textContent.trim()
    `);
    
    console.log('[MEMORY-SEARCH] Work response:', workResponse);
    
    // Should mention remote work or software engineer
    const mentionsWork = workResponse.toLowerCase().includes('remote') || 
                        workResponse.toLowerCase().includes('software engineer');
    expect(mentionsWork).toBeTruthy();
    console.log('[MEMORY-SEARCH] ✓ Response incorporated work-related memory');
    
    console.log('[MEMORY-SEARCH] Memory search and rerank test passed ✓');
  });
});
