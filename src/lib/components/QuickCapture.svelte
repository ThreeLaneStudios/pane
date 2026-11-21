<script lang="ts">
  import { onMount } from 'svelte';
  import * as api from '$lib/api';
  import { getCurrentWindow } from '@tauri-apps/api/window';

  let content = $state('');
  let isSaving = $state(false);
  let textarea: HTMLTextAreaElement;

  onMount(() => {
    // Focus the textarea when component mounts
    textarea?.focus();
  });

  async function handleSave() {
    if (!content.trim()) return;
    
    try {
      isSaving = true;
      await api.quickCaptureNote(content);
      content = '';
      
      // Close the window after saving
      const window = await getCurrentWindow();
      await window.hide();
    } catch (error) {
      console.error('Failed to save quick note:', error);
    } finally {
      isSaving = false;
    }
  }

  function handleKeydown(e: KeyboardEvent) {
    if ((e.metaKey || e.ctrlKey) && e.key === 'Enter') {
      e.preventDefault();
      handleSave();
    } else if (e.key === 'Escape') {
      // Close window on escape
      (async () => {
        const window = await getCurrentWindow();
        (window as any).hide();
      })();
    }
  }

  async function handleCancel() {
    const window = await getCurrentWindow();
    await window.hide();
  }
</script>

<div class="quick-capture">
  <div class="header">
    <h2>Quick Capture</h2>
    <button class="close-btn" onclick={handleCancel} title="Close (Esc)">
      ×
    </button>
  </div>
  
  <div class="content">
    <textarea
      bind:this={textarea}
      bind:value={content}
      onkeydown={handleKeydown}
      placeholder="What's on your mind?"
      rows="8"
    ></textarea>
  </div>
  
  <div class="actions">
    <button class="cancel-btn" onclick={handleCancel}>
      Cancel
    </button>
    <button 
      class="save-btn" 
      onclick={handleSave}
      disabled={!content.trim() || isSaving}
    >
      {isSaving ? 'Saving...' : 'Save to Inbox'}
    </button>
  </div>
  
  <div class="hint">
    <span><kbd>⌘</kbd><kbd>Enter</kbd> to save</span>
    <span><kbd>Esc</kbd> to close</span>
  </div>
</div>

<style>
  .quick-capture {
    display: flex;
    flex-direction: column;
    height: 100vh;
    background: var(--bg-primary);
    color: var(--text-primary);
    font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
  }

  .header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 16px 20px 12px;
    border-bottom: 1px solid var(--border-color);
    -webkit-app-region: drag;
  }

  .header h2 {
    margin: 0;
    font-size: 16px;
    font-weight: 600;
    color: var(--text-primary);
  }

  .close-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 24px;
    height: 24px;
    border: none;
    border-radius: 4px;
    background: transparent;
    color: var(--text-secondary);
    font-size: 20px;
    font-weight: 300;
    cursor: pointer;
    -webkit-app-region: no-drag;
  }

  .close-btn:hover {
    background: var(--bg-hover);
    color: var(--text-primary);
  }

  .content {
    flex: 1;
    padding: 20px;
  }

  textarea {
    width: 100%;
    height: 100%;
    border: none;
    outline: none;
    background: transparent;
    color: var(--text-primary);
    font-family: inherit;
    font-size: 14px;
    line-height: 1.5;
    resize: none;
  }

  textarea::placeholder {
    color: var(--text-tertiary);
  }

  .actions {
    display: flex;
    gap: 12px;
    padding: 16px 20px;
    border-top: 1px solid var(--border-color);
  }

  .cancel-btn,
  .save-btn {
    padding: 8px 16px;
    border: 1px solid var(--border-color);
    border-radius: 6px;
    font-size: 13px;
    font-weight: 500;
    cursor: pointer;
    transition: all 0.15s ease;
  }

  .cancel-btn {
    background: transparent;
    color: var(--text-secondary);
  }

  .cancel-btn:hover {
    background: var(--bg-hover);
    color: var(--text-primary);
  }

  .save-btn {
    background: var(--accent);
    color: white;
    border-color: var(--accent);
  }

  .save-btn:hover:not(:disabled) {
    background: var(--accent-hover);
    border-color: var(--accent-hover);
  }

  .save-btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .hint {
    display: flex;
    gap: 16px;
    justify-content: center;
    padding: 8px 20px 12px;
    font-size: 11px;
    color: var(--text-tertiary);
  }

  .hint span {
    display: flex;
    align-items: center;
    gap: 4px;
  }

  .hint kbd {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    min-width: 18px;
    height: 18px;
    padding: 0 4px;
    border: 1px solid var(--border-color);
    border-radius: 3px;
    background: var(--bg-secondary);
    font-size: 10px;
    font-family: inherit;
    color: var(--text-secondary);
  }

  /* Dark/light theme variables */
  :global([data-theme="light"]) {
    --bg-primary: #ffffff;
    --bg-secondary: #f8f9fa;
    --bg-hover: #f1f3f4;
    --text-primary: #202124;
    --text-secondary: #5f6368;
    --text-tertiary: #9aa0a6;
    --border-color: #dadce0;
    --accent: #1a73e8;
    --accent-hover: #1557b0;
  }

  :global([data-theme="dark"]) {
    --bg-primary: #1a1a1a;
    --bg-secondary: #2d2d2d;
    --bg-hover: #3c3c3c;
    --text-primary: #e8eaed;
    --text-secondary: #9aa0a6;
    --text-tertiary: #5f6368;
    --border-color: #3c4043;
    --accent: #8ab4f8;
    --accent-hover: #aecbfa;
  }
</style>