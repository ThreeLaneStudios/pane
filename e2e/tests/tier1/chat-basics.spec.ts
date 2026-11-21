import { test, expect } from '../../fixtures';

test.describe('T1.5: Chat Basics', () => {
  test('should display chat sidebar', async ({ tauriPage }) => {
    // Test fixture already waits for app-loaded and auth bypass
    
    // Verify chat sidebar is visible by default
    const chatVisible = await tauriPage.isVisible('.chat-panel, [class*="ChatSidebar"]');
    expect(chatVisible).toBeTruthy();
    
    // Verify chat input exists
    const inputVisible = await tauriPage.isVisible('textarea.message-input');
    expect(inputVisible).toBeTruthy();
  });
  
  test('should toggle chat sidebar visibility', async ({ tauriPage }) => {
    // Press Cmd+\ to toggle chat closed
    await tauriPage.press('body', 'Meta+\\');
    
    // Verify chat is collapsed
    const chatCollapsed = await tauriPage.isVisible('.chat-panel.collapsed');
    expect(chatCollapsed).toBeTruthy();
    
    // Toggle again
    await tauriPage.press('body', 'Meta+\\');
    
    // Verify chat is open again
    const chatVisible = await tauriPage.isVisible('.chat-panel:not(.collapsed)');
    expect(chatVisible).toBeTruthy();
  });
  
  test('should send a chat message', async ({ tauriPage }) => {
    // Focus chat input (Cmd+/)
    await tauriPage.press('body', 'Meta+/');
    
    // Type a message
    await tauriPage.fill('textarea.message-input', 'Hello from E2E test');
    
    // Send (Enter)
    await tauriPage.press('textarea.message-input', 'Enter');
    
    // Verify message appears in chat history
    await tauriPage.waitForSelector('text=Hello from E2E test', 5000);
    
    // Verify AI response eventually appears (may take several seconds)
    await tauriPage.waitForSelector('.chat-message:not(.user-message)', 15000);
  });
});
