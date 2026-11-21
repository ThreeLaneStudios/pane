<script lang="ts">
  import { onMount } from 'svelte';
  import type { NoteContent, TagInfo } from '$lib/types';
  import * as api from '$lib/api';
  
  interface Props {
    note: NoteContent | null;
    onClose: () => void;
  }
  
  let { note, onClose }: Props = $props();
  
  let availableTags: TagInfo[] = $state([]);
  let newTagName = $state('');
  let showTagInput = $state(false);
  let tagInputRef: HTMLInputElement = $state(null!);
  
  // Load tags when component mounts
  onMount(async () => {
    try {
      availableTags = await api.listTags();
    } catch (e) {
      console.error('Failed to load tags:', e);
    }
  });
  
  // Calculate note stats
  let noteStats = $derived(() => {
    if (!note) return null;
    
    const content = note.content;
    const wordCount = content.split(/\s+/).filter(word => word.length > 0).length;
    const charCount = content.length;
    const lineCount = content.split('\n').length;
    
    return {
      wordCount,
      charCount,
      lineCount
    };
  });
  
  function formatDate(dateStr: string): string {
    if (!dateStr) return 'Unknown';
    try {
      const date = new Date(dateStr);
      return date.toLocaleDateString('en-US', {
        weekday: 'long',
        year: 'numeric', 
        month: 'long',
        day: 'numeric',
        hour: '2-digit',
        minute: '2-digit'
      });
    } catch {
      return 'Invalid date';
    }
  }
  
  async function addNewTag() {
    if (!newTagName.trim() || !note) return;
    
    try {
      // Create tag if it doesn't exist
      let tag = availableTags.find(t => t.name === newTagName.trim());
      if (!tag) {
        tag = await api.createTag(newTagName.trim());
        availableTags = [...availableTags, tag];
      }
      
      // Add tag to note
      await api.addTag(note.meta.id, tag.name);
      
      // Update note's tags (this would ideally be reflected in the parent component)
      if (note.meta.tags) {
        note.meta.tags = [...note.meta.tags, tag.name];
      } else {
        note.meta.tags = [tag.name];
      }
      
      newTagName = '';
      showTagInput = false;
    } catch (e) {
      console.error('Failed to add tag:', e);
    }
  }
  
  async function removeTag(tagName: string) {
    if (!note) return;
    
    try {
      await api.removeTag(note.meta.id, tagName);
      
      // Update note's tags
      if (note.meta.tags) {
        note.meta.tags = note.meta.tags.filter(tag => tag !== tagName);
      }
    } catch (e) {
      console.error('Failed to remove tag:', e);
    }
  }
  
  function handleKeydown(e: KeyboardEvent) {
    if (e.key === 'Enter') {
      addNewTag();
    } else if (e.key === 'Escape') {
      showTagInput = false;
      newTagName = '';
    }
  }
  
  $effect(() => {
    if (showTagInput && tagInputRef) {
      tagInputRef.focus();
    }
  });
</script>

{#if note}
  <div class="metadata-panel">
    <div class="metadata-header">
      <h3>Note Information</h3>
      <button class="close-btn" onclick={onClose}>✕</button>
    </div>
    
    <div class="metadata-content">
      <section class="metadata-section">
        <h4>Details</h4>
        <div class="metadata-row">
          <span class="label">Title:</span>
          <span class="value">{note.meta.title || 'Untitled'}</span>
        </div>
        <div class="metadata-row">
          <span class="label">Notebook:</span>
          <span class="value">{note.meta.notebook}</span>
        </div>
        <div class="metadata-row">
          <span class="label">Created:</span>
          <span class="value">{formatDate(note.meta.created_at)}</span>
        </div>
        <div class="metadata-row">
          <span class="label">Modified:</span>
          <span class="value">{formatDate(note.meta.updated_at)}</span>
        </div>
      </section>

      {#if noteStats}
        <section class="metadata-section">
          <h4>Statistics</h4>
          <div class="metadata-row">
            <span class="label">Words:</span>
            <span class="value">{noteStats()?.wordCount.toLocaleString()}</span>
          </div>
          <div class="metadata-row">
            <span class="label">Characters:</span>
            <span class="value">{noteStats()?.charCount.toLocaleString()}</span>
          </div>
          <div class="metadata-row">
            <span class="label">Lines:</span>
            <span class="value">{noteStats()?.lineCount.toLocaleString()}</span>
          </div>
        </section>
      {/if}

      <section class="metadata-section">
        <div class="section-header">
          <h4>Tags</h4>
          <button 
            class="add-tag-btn"
            onclick={() => showTagInput = true}
            title="Add tag"
          >
            +
          </button>
        </div>
        
        <div class="tags-container">
          {#if note.meta.tags && note.meta.tags.length > 0}
            {#each note.meta.tags as tag}
              <div class="tag">
                <span class="tag-name">{tag}</span>
                <button 
                  class="remove-tag-btn"
                  onclick={() => removeTag(tag)}
                  title="Remove tag"
                >
                  ✕
                </button>
              </div>
            {/each}
          {:else}
            <span class="no-tags">No tags</span>
          {/if}
          
          {#if showTagInput}
            <input
              bind:this={tagInputRef}
              bind:value={newTagName}
              onkeydown={handleKeydown}
              onblur={() => { showTagInput = false; newTagName = ''; }}
              placeholder="Tag name..."
              class="tag-input"
            />
          {/if}
        </div>
      </section>
    </div>
  </div>
{/if}

<style>
  .metadata-panel {
    position: fixed;
    top: 50%;
    left: 50%;
    transform: translate(-50%, -50%);
    width: 400px;
    max-height: 600px;
    background: var(--bg-primary);
    border: 1px solid var(--border-color);
    border-radius: 12px;
    box-shadow: 0 8px 32px rgba(0, 0, 0, 0.12);
    z-index: 1000;
    overflow: hidden;
  }

  .metadata-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 16px 20px;
    border-bottom: 1px solid var(--border-color);
    background: var(--bg-secondary);
  }

  .metadata-header h3 {
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
    color: var(--text-tertiary);
    cursor: pointer;
    font-size: 14px;
    transition: all 0.15s ease;
  }

  .close-btn:hover {
    background: var(--bg-hover);
    color: var(--text-primary);
  }

  .metadata-content {
    padding: 20px;
    overflow-y: auto;
    max-height: 500px;
  }

  .metadata-section {
    margin-bottom: 24px;
  }

  .metadata-section:last-child {
    margin-bottom: 0;
  }

  .metadata-section h4 {
    margin: 0 0 12px 0;
    font-size: 14px;
    font-weight: 600;
    color: var(--text-primary);
  }

  .section-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 12px;
  }

  .add-tag-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 20px;
    height: 20px;
    border: none;
    border-radius: 4px;
    background: var(--accent);
    color: white;
    cursor: pointer;
    font-size: 14px;
    line-height: 1;
    transition: all 0.15s ease;
  }

  .add-tag-btn:hover {
    background: var(--accent-hover);
  }

  .metadata-row {
    display: flex;
    align-items: flex-start;
    gap: 12px;
    margin-bottom: 8px;
  }

  .metadata-row:last-child {
    margin-bottom: 0;
  }

  .label {
    font-size: 13px;
    font-weight: 500;
    color: var(--text-secondary);
    min-width: 80px;
  }

  .value {
    font-size: 13px;
    color: var(--text-primary);
    flex: 1;
    word-break: break-word;
  }

  .tags-container {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    align-items: center;
  }

  .tag {
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 4px 8px;
    background: var(--accent-light);
    border: 1px solid var(--accent-light-border);
    border-radius: 12px;
    font-size: 12px;
    color: var(--accent);
  }

  .tag-name {
    font-weight: 500;
  }

  .remove-tag-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 14px;
    height: 14px;
    border: none;
    border-radius: 50%;
    background: transparent;
    color: var(--accent);
    cursor: pointer;
    font-size: 10px;
    transition: all 0.15s ease;
  }

  .remove-tag-btn:hover {
    background: rgba(255, 255, 255, 0.2);
  }

  .no-tags {
    font-size: 13px;
    color: var(--text-tertiary);
    font-style: italic;
  }

  .tag-input {
    padding: 4px 8px;
    border: 1px solid var(--accent);
    border-radius: 12px;
    background: var(--bg-primary);
    color: var(--text-primary);
    font-size: 12px;
    font-family: inherit;
    outline: none;
    min-width: 80px;
  }
</style>