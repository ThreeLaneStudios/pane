<script lang="ts">
  import { onMount } from 'svelte';
  import * as api from '$lib/api';
  import type { DashboardModule } from '$lib/types/dashboard';
  import type { GratitudeEntry } from '$lib/types';
  
  interface Props {
    module: DashboardModule;
  }
  
  let { module }: Props = $props();
  
  let entries: GratitudeEntry[] = $state([]);
  let newEntry = $state('');
  
  const todayStr = new Date().toISOString().split('T')[0];
  
  onMount(async () => {
    await loadEntries();
  });
  
  async function loadEntries() {
    try {
      entries = await api.getGratitudeEntries(todayStr);
    } catch (e) {
      console.error('Failed to load gratitude entries:', e);
      entries = [];
    }
  }
  
  async function addEntry() {
    if (!newEntry.trim()) return;
    try {
      await api.addGratitudeEntry(todayStr, newEntry.trim());
      newEntry = '';
      await loadEntries();
    } catch (e) {
      console.error('Failed to add gratitude entry:', e);
    }
  }
  
  async function deleteEntry(entryId: string) {
    try {
      await api.deleteGratitudeEntry(entryId);
      await loadEntries();
    } catch (e) {
      console.error('Failed to delete gratitude entry:', e);
    }
  }
  
  function handleKeydown(e: KeyboardEvent) {
    if (e.key === 'Enter') addEntry();
  }
</script>

<div class="card">
  <div class="card-label">GRATITUDE</div>
  
  {#if entries.length > 0}
    <div class="entries">
      {#each entries as entry}
        <div class="entry">
          <span class="bullet"></span>
          <span class="entry-text">{entry.text}</span>
          <button class="delete-btn" onclick={() => deleteEntry(entry.id)} aria-label="Delete entry">×</button>
        </div>
      {/each}
    </div>
  {/if}
  
  <div class="input-row">
    <input
      class="gratitude-input"
      type="text"
      placeholder="What are you grateful for? Press Enter to add"
      bind:value={newEntry}
      onkeydown={handleKeydown}
    />
  </div>
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
  
  .entries {
    display: flex;
    flex-direction: column;
    gap: 6px;
    margin-bottom: 14px;
  }
  
  .entry {
    display: flex;
    align-items: center;
    gap: 10px;
    font-size: 13px;
    color: #e0e0e0;
    font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Text', 'Segoe UI', sans-serif;
    line-height: 1.5;
  }
  
  .bullet {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: #00E4E4;
    flex-shrink: 0;
    margin-top: 1px; /* Slight adjustment for vertical centering */
  }
  
  .entry-text {
    flex: 1;
  }
  
  .delete-btn {
    background: none;
    border: none;
    color: #666666;
    font-size: 18px;
    line-height: 1;
    cursor: pointer;
    padding: 4px 6px;
    opacity: 0;
    transition: all 0.2s;
  }
  
  .entry:hover .delete-btn {
    opacity: 1;
  }
  
  .delete-btn:hover {
    color: #00E4E4;
  }
  
  .input-row {
    margin-top: 4px;
  }
  
  .gratitude-input {
    width: 100%;
    padding: 10px 14px;
    border-radius: 4px;
    border: 1px solid rgba(255, 255, 255, 0.08);
    background: #0a0a0f;
    color: #e0e0e0;
    font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Text', 'Segoe UI', sans-serif;
    font-size: 13px;
    outline: none;
    transition: border-color 0.2s;
  }
  
  .gratitude-input:focus {
    border-color: rgba(0, 228, 228, 0.4);
  }
  
  .gratitude-input::placeholder {
    color: #666666;
  }
</style>
