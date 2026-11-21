<script lang="ts">
  import type { Task } from '$lib/types';

  interface Props {
    task: Task;
    position: { x: number; y: number };
    onClose: () => void;
    onDelete: (task: Task) => void;
  }

  let { task, position, onClose, onDelete }: Props = $props();

  function handleDelete() {
    onDelete(task);   // callback BEFORE close — avoid the Svelte 5 reactivity bug
    onClose();
  }
</script>

<div
  class="context-menu"
  style:left="{position.x}px"
  style:top="{position.y}px"
  onclick={(e) => e.stopPropagation()}
>
  <ul class="menu-items">
    <li>
      <button class="menu-item delete-item" onclick={handleDelete}>
        <svg class="menu-icon" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5">
          <path d="M3 4h10M6 4V3a1 1 0 011-1h2a1 1 0 011 1v1M5 4v8a1 1 0 001 1h4a1 1 0 001-1V4"/>
        </svg>
        <span class="item-name">Move to Trash</span>
      </button>
    </li>
  </ul>
</div>

<style>
  .context-menu {
    position: fixed;
    background: #0a0a0f;
    border: 1px solid rgba(0, 228, 228, 0.2);
    border-radius: 8px;
    box-shadow: 0 4px 20px rgba(0, 0, 0, 0.4);
    z-index: 1000;
    min-width: 160px;
    overflow: visible;
    font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Text', 'Segoe UI', sans-serif;
  }

  .menu-items {
    list-style: none;
    margin: 0;
    padding: 4px 0;
  }

  .menu-item {
    display: flex;
    align-items: center;
    width: 100%;
    padding: 8px 12px;
    border: none;
    background: transparent;
    color: #e0e0e0;
    cursor: pointer;
    font-size: 13px;
    font-family: inherit;
    text-align: left;
    transition: background 0.1s ease;
  }

  .menu-item:hover {
    background: rgba(255, 255, 255, 0.04);
  }

  .menu-icon {
    width: 14px;
    height: 14px;
    margin-right: 10px;
    flex-shrink: 0;
    opacity: 0.7;
  }

  .item-name {
    flex: 1;
    font-weight: 400;
  }

  .menu-divider {
    height: 1px;
    background: rgba(255, 255, 255, 0.06);
    margin: 4px 8px;
  }

  .delete-item {
    color: #EF4444;
  }

  .delete-item:hover {
    background: rgba(239, 68, 68, 0.1);
  }
</style>
