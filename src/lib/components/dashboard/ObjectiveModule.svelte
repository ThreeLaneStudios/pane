<script lang="ts">
  import { invoke } from '@tauri-apps/api/core';
  import type { DashboardModule } from '$lib/types/dashboard';
  
  interface Props {
    module: DashboardModule;
  }
  
  let { module }: Props = $props();
  
  let objectiveText = $state(module.static_content || '');
  let editing = $state(false);
  
  async function saveObjective() {
    try {
      await invoke('update_module_content', {
        moduleId: module.id,
        content: objectiveText
      });
      editing = false;
    } catch (e) {
      console.error('Failed to save objective:', e);
    }
  }
  
  function handleKeydown(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      editing = false;
      objectiveText = module.static_content || '';
    }
  }
</script>

<div class="card">
  <div class="card-label">3–5 Year Objective</div>
  
  {#if editing}
    <textarea
      bind:value={objectiveText}
      onkeydown={handleKeydown}
      onblur={saveObjective}
      class="objective-input"
      placeholder="Describe where you want to be in 3–5 years..."
      autofocus
    ></textarea>
  {:else}
    <div
      class="objective-text"
      onclick={() => editing = true}
      role="button"
      tabindex="0"
    >
      {objectiveText || 'Click to set your objective...'}
    </div>
  {/if}
</div>

<style>
  .card {
    background: rgba(255,255,255,0.05);
    border: 1px solid rgba(255, 255, 255, 0.06);
    border-radius: 4px;
    padding: 22px 24px;
    position: relative;
  }
  
  .card-label {
    font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Text', 'Segoe UI', sans-serif;
    font-size: 14px;
    font-weight: 700;
    color: #666666;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    margin-bottom: 12px;
  }
  
  .objective-text {
    font-size: 14px;
    line-height: 1.7;
    color: #e0e0e0;
    cursor: pointer;
    min-height: 60px;
    padding: 8px;
    border-radius: 6px;
    transition: background 0.2s;
  }
  
  .objective-text:hover {
    background: rgba(255, 255, 255, 0.02);
  }
  
  .objective-input {
    width: 100%;
    min-height: 120px;
    padding: 12px 16px;
    border-radius: 8px;
    border: 1px solid rgba(0, 228, 228, 0.3);
    background: rgba(255, 255, 255, 0.02);
    color: #e0e0e0;
    font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Text', 'Segoe UI', sans-serif;
    font-size: 14px;
    line-height: 1.7;
    outline: none;
    resize: vertical;
  }
  
  .objective-input:focus {
    border-color: rgba(0, 228, 228, 0.6);
    box-shadow: 0 0 12px rgba(0, 228, 228, 0.1);
  }
</style>
