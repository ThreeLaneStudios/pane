<script lang="ts">
  import { notebooks, activeNotebook } from '$lib/stores/app';
  import * as api from '$lib/api';
  import type { NoteMeta } from '$lib/types';

  interface Props {
    note: NoteMeta;
    onClose: () => void;
    onMoved: (notebook: string) => void;
    onDelete?: () => void;
    position: { x: number; y: number };
  }

  let { note, onClose, onMoved, onDelete, position }: Props = $props();

  async function moveToNotebook(targetNotebook: string) {
    if (targetNotebook === note.notebook) {
      onClose();
      return;
    }

    try {
      await api.moveNote(note.notebook, note.slug, targetNotebook);
      onMoved(targetNotebook);
    } catch (e) {
      console.error('Failed to move note:', e);
    }
    
    onClose();
  }

  function deleteNote() {
    // Trigger parent's delete confirm (parent handles closing context menu)
    onDelete?.();
  }
</script>

<div
  class="context-menu"
  style:left="{position.x}px"
  style:top="{position.y}px"
>
  <ul class="menu-items">
    <li>
      <button 
        class="menu-item delete-item"
        onclick={deleteNote}
        data-testid="delete-note-option"
      >
        <span class="item-icon">🗑️</span>
        <span class="item-name">Delete</span>
      </button>
    </li>
  </ul>
  
  <div class="menu-header">
    Move to Notebook
  </div>
  <ul class="menu-items">
    {#each $notebooks as notebook}
      {#if notebook !== note.notebook}
        <li>
          <button 
            class="menu-item"
            onclick={() => moveToNotebook(notebook)}
          >
            <span class="notebook-icon">📓</span>
            <span class="notebook-name">{notebook}</span>
          </button>
        </li>
      {/if}
    {/each}
  </ul>
</div>

<style>
  .context-menu {
    position: fixed;
    background: var(--bg-primary);
    border: 1px solid var(--border-color);
    border-radius: 8px;
    box-shadow: 0 4px 20px rgba(0, 0, 0, 0.15);
    z-index: 1000;
    min-width: 180px;
    max-width: 250px;
    overflow: hidden;
  }

  .menu-header {
    padding: 8px 12px;
    background: var(--bg-secondary);
    border-bottom: 1px solid var(--border-color);
    font-size: 11px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: var(--text-tertiary);
  }

  .menu-items {
    list-style: none;
    margin: 0;
    padding: 4px 0;
  }

  .menu-item {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    padding: 8px 12px;
    border: none;
    background: transparent;
    color: var(--text-primary);
    cursor: pointer;
    font-size: 13px;
    font-family: inherit;
    text-align: left;
    transition: all 0.15s ease;
  }

  .menu-item:hover {
    background: var(--bg-hover);
  }

  .notebook-icon {
    font-size: 14px;
  }

  .notebook-name,
  .item-name {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .notebook-icon,
  .item-icon {
    font-size: 14px;
  }

  .menu-item.delete-item {
    color: #e74c3c;
  }

  .menu-item.delete-item:hover {
    background: rgba(231, 76, 60, 0.1);
  }

</style>