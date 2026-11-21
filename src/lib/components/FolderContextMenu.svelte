<script lang="ts">
  /**
   * FolderContextMenu.svelte — Right-click context menu for folders (#1751/#766)
   *
   * Items:
   *   - Edit Name (hidden when folder.is_system)
   *   - Delete — shown but DISABLED (not hidden) for the owning project's
   *     default folder, with a tooltip explaining why (#2874 item 65
   *     ruling 1: the user must see why they can't delete it, not have it
   *     silently disappear from the menu). Routes through ConfirmDialog in
   *     parent for every other folder.
   *
   * Modeled on ProjectContextMenu.svelte pattern.
   */

  import type { Project } from '$lib/types';

  const DEFAULT_FOLDER_DELETE_TOOLTIP =
    'This is the default folder. Choose a different default in Project Settings to delete it.';

  // Folder = Project with parent_id set
  interface Props {
    folder: Project;
    position: { x: number; y: number };
    onClose: () => void;
    onRename: (folder: Project) => void;
    onDelete: (folder: Project) => void;
    isDefaultFolder?: boolean;
  }

  let { folder, position, onClose, onRename, onDelete, isDefaultFolder = false }: Props = $props();

  function handleRename() {
    onRename(folder);
    onClose();
  }

  function handleDelete() {
    if (isDefaultFolder) return; // defensive — the button is also `disabled`
    onDelete(folder);
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
      <button class="menu-item" onclick={handleRename}>
        <svg class="menu-icon" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5">
          <path d="M11.5 2.5a1.414 1.414 0 012 2L5 13H2v-3L11.5 2.5z"/>
        </svg>
        <span class="item-name">Edit Name</span>
      </button>
    </li>
    <li class="menu-divider"></li>
    <li>
      <button
        class="menu-item delete-item"
        class:disabled={isDefaultFolder}
        disabled={isDefaultFolder}
        title={isDefaultFolder ? DEFAULT_FOLDER_DELETE_TOOLTIP : undefined}
        onclick={handleDelete}
      >
        <svg class="menu-icon" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5">
          <path d="M3 4h10M6 4V3a1 1 0 011-1h2a1 1 0 011 1v1M5 4v8a1 1 0 001 1h4a1 1 0 001-1V4"/>
        </svg>
        <span class="item-name">Delete Folder</span>
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

  .delete-item.disabled,
  .delete-item:disabled {
    color: rgba(239, 68, 68, 0.4);
    cursor: not-allowed;
  }

  .delete-item.disabled:hover,
  .delete-item:disabled:hover {
    background: transparent;
  }

</style>
