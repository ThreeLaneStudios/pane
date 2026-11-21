<script lang="ts">
  import { onMount } from 'svelte';
  import * as api from '$lib/api';
  import type { TagInfo } from '$lib/types';

  let tags: TagInfo[] = $state([]);
  let newTagName = $state('');
  let newTagColor = $state('#00E4E4');
  let editingTag: TagInfo | null = $state(null);
  let editName = $state('');
  let editColor = $state('');
  let loading = $state(true);
  let showCustomColor = $state(false);
  let showEditCustomColor = $state(false);
  let customHex = $state('');
  let editCustomHex = $state('');

  const presetColors = [
    '#00E4E4', '#3b82f6', '#10b981', '#f59e0b', '#ef4444',
    '#8b5cf6', '#ec4899', '#f97316', '#06b6d4', '#84cc16',
    '#6366f1', '#14b8a6', '#e11d48', '#a855f7', '#6b7280'
  ];

  onMount(async () => {
    await loadTags();
  });

  async function loadTags() {
    loading = true;
    try {
      tags = await api.listTags();
    } catch (e) {
      console.error('Failed to load tags:', e);
    } finally {
      loading = false;
    }
  }

  async function createTag() {
    if (!newTagName.trim()) return;
    try {
      await api.createTag(newTagName.trim(), newTagColor);
      newTagName = '';
      newTagColor = '#00E4E4';
      showCustomColor = false;
      await loadTags();
    } catch (e) {
      console.error('Failed to create tag:', e);
    }
  }

  async function deleteTag(tag: TagInfo) {
    try {
      await api.deleteTag(tag.id);
      await loadTags();
    } catch (e) {
      console.error('Failed to delete tag:', e);
    }
  }

  function startEdit(tag: TagInfo) {
    editingTag = tag;
    editName = tag.name;
    editColor = tag.color;
    showEditCustomColor = false;
  }

  async function saveEdit() {
    if (!editingTag || !editName.trim()) return;
    try {
      await api.updateTag(editingTag.id, editName.trim(), editColor);
      editingTag = null;
      showEditCustomColor = false;
      await loadTags();
    } catch (e) {
      console.error('Failed to update tag:', e);
    }
  }

  function cancelEdit() {
    editingTag = null;
    showEditCustomColor = false;
  }

  function applyCustomHex() {
    if (/^#[0-9a-fA-F]{6}$/.test(customHex)) {
      newTagColor = customHex;
    }
  }

  function applyEditCustomHex() {
    if (/^#[0-9a-fA-F]{6}$/.test(editCustomHex)) {
      editColor = editCustomHex;
    }
  }
</script>

<div class="tag-management">
  <div class="tm-header">
    <h2>Tags</h2>
    <p class="tm-subtitle">Create, edit, and color-code your tags</p>
  </div>

  <div class="tm-content">
    <!-- Create new tag -->
    <div class="create-tag-form">
      <div class="create-row">
        <input
          bind:value={newTagName}
          placeholder="New tag name..."
          class="tag-input"
          onkeydown={(e) => { if (e.key === 'Enter') createTag(); }}
        />
        <button class="create-btn" onclick={createTag} disabled={!newTagName.trim()}>
          Create
        </button>
      </div>
      <div class="color-section">
        <span class="color-label">Color</span>
        <div class="color-picker-grid">
          {#each presetColors as c}
            <button
              class="color-swatch"
              class:selected={newTagColor === c}
              style:background={c}
              onclick={() => { newTagColor = c; showCustomColor = false; }}
              title={c}
            ></button>
          {/each}
          <button
            class="color-swatch custom-swatch"
            class:selected={showCustomColor}
            onclick={() => { showCustomColor = !showCustomColor; customHex = newTagColor; }}
            title="Custom color"
          >⋯</button>
        </div>
        {#if showCustomColor}
          <div class="custom-color-row">
            <input type="color" bind:value={newTagColor} class="native-color-input" />
            <input
              bind:value={customHex}
              placeholder="#00E4E4"
              class="hex-input"
              oninput={() => applyCustomHex()}
              onkeydown={(e) => { if (e.key === 'Enter') applyCustomHex(); }}
            />
          </div>
        {/if}
        <div class="color-preview">
          <span class="preview-bubble" style:background={`color-mix(in srgb, ${newTagColor} 18%, transparent)`} style:color={newTagColor}>
            {newTagName || 'preview'}
          </span>
        </div>
      </div>
    </div>

    <!-- Tag list -->
    {#if loading}
      <div class="loading">Loading tags...</div>
    {:else if tags.length === 0}
      <div class="empty">
        <p>No tags yet. Create your first tag above.</p>
      </div>
    {:else}
      <div class="tag-list">
        {#each tags as tag (tag.id)}
          <div class="tag-row">
            {#if editingTag?.id === tag.id}
              <div class="tag-edit-row">
                <input bind:value={editName} class="edit-input" onkeydown={(e) => { if (e.key === 'Enter') saveEdit(); if (e.key === 'Escape') cancelEdit(); }} />
                <div class="color-picker-grid small">
                  {#each presetColors as c}
                    <button
                      class="color-swatch small"
                      class:selected={editColor === c}
                      style:background={c}
                      onclick={() => { editColor = c; showEditCustomColor = false; }}
                    ></button>
                  {/each}
                  <button
                    class="color-swatch small custom-swatch"
                    class:selected={showEditCustomColor}
                    onclick={() => { showEditCustomColor = !showEditCustomColor; editCustomHex = editColor; }}
                  >⋯</button>
                </div>
                {#if showEditCustomColor}
                  <div class="custom-color-row">
                    <input type="color" bind:value={editColor} class="native-color-input" />
                    <input
                      bind:value={editCustomHex}
                      placeholder="#00E4E4"
                      class="hex-input"
                      oninput={() => applyEditCustomHex()}
                    />
                  </div>
                {/if}
                <div class="edit-actions">
                  <button class="save-edit-btn" onclick={saveEdit}>Save</button>
                  <button class="cancel-edit-btn" onclick={cancelEdit}>Cancel</button>
                </div>
              </div>
            {:else}
              <div class="tag-display">
                <span class="tag-pill" style:background={`color-mix(in srgb, ${tag.color} 18%, transparent)`} style:color={tag.color}>
                  {tag.name}
                </span>
              </div>
              <div class="tag-actions">
                <button class="edit-btn" onclick={() => startEdit(tag)}>Edit</button>
                <button class="delete-btn" onclick={() => deleteTag(tag)}>Delete</button>
              </div>
            {/if}
          </div>
        {/each}
      </div>
    {/if}
  </div>
</div>

<style>
  .tag-management {
    flex: 1;
    display: flex;
    flex-direction: column;
    background: var(--bg-primary);
    height: 100%;
    font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Text', 'Segoe UI', sans-serif;
  }

  .tm-header {
    padding: 24px;
    border-bottom: 1px solid var(--border-color);
  }

  .tm-header h2 {
    font-size: 20px;
    font-weight: 600;
    color: var(--text-primary);
    margin: 0 0 4px;
  }

  .tm-subtitle {
    font-size: 13px;
    color: var(--text-tertiary);
    margin: 0;
  }

  .tm-content {
    flex: 1;
    overflow-y: auto;
    padding: 24px;
  }

  .create-tag-form {
    margin-bottom: 24px;
    padding-bottom: 24px;
    border-bottom: 1px solid var(--border-color);
  }

  .create-row {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-bottom: 12px;
  }

  .tag-input, .edit-input {
    flex: 1;
    min-width: 150px;
    padding: 8px 12px;
    border: 1px solid var(--border-color);
    border-radius: 6px;
    background: var(--bg-primary);
    color: var(--text-primary);
    font-size: 14px;
    font-family: inherit;
    outline: none;
  }

  .tag-input:focus, .edit-input:focus {
    border-color: var(--accent);
  }

  .color-section {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .color-label {
    font-size: 11px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.08em;
    color: var(--text-tertiary);
  }

  .color-picker-grid {
    display: flex;
    gap: 5px;
    align-items: center;
    flex-wrap: wrap;
  }

  .color-swatch {
    width: 22px;
    height: 22px;
    border-radius: 6px;
    border: 2px solid transparent;
    cursor: pointer;
    transition: all 0.15s ease;
    font-size: 10px;
    color: var(--text-tertiary);
    display: flex;
    align-items: center;
    justify-content: center;
  }

  .color-swatch.selected {
    border-color: var(--text-primary);
    transform: scale(1.15);
    box-shadow: 0 0 0 1px var(--bg-primary);
  }

  .color-swatch.small {
    width: 16px;
    height: 16px;
    border-radius: 4px;
    font-size: 8px;
  }

  .custom-swatch {
    background: var(--bg-tertiary) !important;
    font-weight: 700;
  }

  .custom-color-row {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .native-color-input {
    width: 32px;
    height: 28px;
    border: 1px solid var(--border-color);
    border-radius: 4px;
    background: none;
    cursor: pointer;
    padding: 0;
  }

  .hex-input {
    width: 90px;
    padding: 4px 8px;
    border: 1px solid var(--border-color);
    border-radius: 4px;
    background: var(--bg-primary);
    color: var(--text-primary);
    font-size: 12px;
    font-family: 'SF Mono', 'Fira Code', monospace;
    outline: none;
  }
  .hex-input:focus { border-color: var(--accent); }

  .color-preview {
    padding-top: 4px;
  }

  .preview-bubble, .tag-pill {
    display: inline-flex;
    align-items: center;
    padding: 3px 10px;
    border-radius: 10px;
    font-size: 12px;
    font-weight: 500;
    line-height: 1.4;
  }

  .create-btn {
    padding: 8px 16px;
    border: none;
    border-radius: 6px;
    background: var(--accent);
    color: #000;
    font-size: 13px;
    font-weight: 600;
    cursor: pointer;
    font-family: inherit;
    transition: background 0.15s;
  }

  .create-btn:hover:not(:disabled) { background: var(--accent-hover); }
  .create-btn:disabled { opacity: 0.4; cursor: not-allowed; }

  .tag-list {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .tag-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 8px 12px;
    border-radius: 6px;
    transition: background 0.15s;
  }

  .tag-row:hover {
    background: var(--bg-hover);
  }

  .tag-edit-row {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    flex-wrap: wrap;
  }

  .edit-actions {
    display: flex;
    gap: 6px;
    margin-left: auto;
  }

  .tag-display {
    display: flex;
    align-items: center;
    gap: 10px;
  }

  .tag-actions {
    display: flex;
    gap: 8px;
    opacity: 0;
    transition: opacity 0.15s;
  }

  .tag-row:hover .tag-actions {
    opacity: 1;
  }

  .edit-btn, .delete-btn, .save-edit-btn, .cancel-edit-btn {
    padding: 4px 10px;
    border: none;
    border-radius: 4px;
    font-size: 12px;
    cursor: pointer;
    font-family: inherit;
  }

  .edit-btn { background: var(--bg-secondary); color: var(--text-secondary); }
  .edit-btn:hover { background: var(--bg-hover); color: var(--text-primary); }
  .delete-btn { background: transparent; color: #ef4444; }
  .delete-btn:hover { background: rgba(239, 68, 68, 0.1); }
  .save-edit-btn { background: var(--accent); color: #000; font-weight: 500; }
  .cancel-edit-btn { background: var(--bg-secondary); color: var(--text-secondary); }

  .loading, .empty {
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 48px;
    color: var(--text-tertiary);
    font-size: 14px;
  }
</style>
