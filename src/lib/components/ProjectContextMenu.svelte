<script lang="ts">
  import { projects } from '$lib/stores/app';
  import { invoke } from '@tauri-apps/api/core';
  import * as api from '$lib/api';
  import { canvasState } from '$lib/stores/canvas';
  import type { Project, CategoryWithColor } from '$lib/types';
  import { onMount } from 'svelte';
  import { navigateTo } from '$lib/services/navigation';

  interface Props {
    project: Project;
    onClose: () => void;
    onNewCategory?: (projectId: string) => void;
    onNewFolder?: (projectId: string) => void;
    onUnfavorite?: () => void;
    onArchive?: () => void;
    onDelete?: () => void;
    position: { x: number; y: number };
  }

  let { project, onClose, onNewCategory, onNewFolder, onUnfavorite, onArchive, onDelete, position }: Props = $props();

  // Check if project is favorited (DB-backed via is_favorite column)
  let isFavorited = $derived(project.is_favorite ?? false);
  
  let showCategoryDropdown = $state(false);
  let categories: CategoryWithColor[] = $state([]);

  onMount(async () => {
    try {
      categories = await api.listCategories();
    } catch (e) {
      console.error('Failed to load categories:', e);
      categories = [];
    }
  });

  function openProject() {
    // Navigate to project detail view
    navigateTo({ view: 'notes_browser', projectId: project.id });
    onClose();
  }

  async function toggleFavorite() {
    const newFavorite = !isFavorited;
    try {
      await invoke('update_project_settings', { projectId: project.id, isFavorite: newFavorite });
      // Refresh projects store so sidebar and other views react
      $projects = await api.listProjects();
      if (!newFavorite && onUnfavorite) {
        onUnfavorite();
      }
    } catch (e) {
      console.error('Failed to toggle favorite:', e);
    }
    onClose();
  }

  function archiveProject() {
    if (onArchive) {
      onArchive();
    }
  }

  function deleteProject() {
    if (onDelete) {
      onDelete();
    }
  }

  async function assignCategory(categoryName: string, categoryColor: string) {
    try {
      await api.setProjectCategory(project.id, categoryName);
      // Refresh projects list to show the updated category
      $projects = await api.listProjects();
      onClose();
    } catch (e) {
      console.error('Failed to assign category:', e);
    }
  }

  async function removeCategory() {
    try {
      await api.setProjectCategory(project.id, null);
      // Refresh projects list
      $projects = await api.listProjects();
      onClose();
    } catch (e) {
      console.error('Failed to remove category:', e);
    }
  }

  function openSettings() {
    navigateTo({ view: 'settings' });
    onClose();
  }

  function openProjectSettings() {
    navigateTo({ view: 'project_config', projectId: project.id });
    onClose();
  }

  function newFolder() {
    if (onNewFolder) {
      onNewFolder(project.id);
    }
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
        <button class="menu-item" onclick={openProject}>
          <svg class="menu-icon" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5"><path d="M6 3l5 5-5 5"/></svg>
          <span class="item-name">Open</span>
        </button>
      </li>
      
      <li class="menu-divider"></li>
      
      <li>
        <button class="menu-item" onclick={(e) => { e.stopPropagation(); showCategoryDropdown = !showCategoryDropdown; }}>
          <svg class="menu-icon" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5"><circle cx="8" cy="8" r="5"/></svg>
          <span class="item-name">Set Category</span>
          <svg class="menu-arrow" class:rotated={showCategoryDropdown} viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5"><path d="M6 9l2 2 2-2"/></svg>
        </button>
      </li>
      
      {#if showCategoryDropdown}
        <li class="category-dropdown">
          {#if project.category}
            <button class="category-option remove-category" onclick={removeCategory}>
              <svg class="category-icon" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5"><path d="M4 4l8 8m0-8l-8 8"/></svg>
              <span>Remove Category</span>
            </button>
          {/if}
          
          {#if categories.length === 0}
            <button class="category-option no-categories" onclick={openSettings}>
              <svg class="category-icon" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5"><path d="M8 3v10M3 8h10"/></svg>
              <span>Define categories in Settings</span>
            </button>
          {:else}
            {#each categories as category (category.category)}
              <button 
                class="category-option"
                class:active={project.category === category.category}
                onclick={() => assignCategory(category.category, category.color)}
              >
                <span class="category-color-dot" style:background={category.color}></span>
                <span>{category.category}</span>
              </button>
            {/each}
          {/if}
        </li>
      {/if}
      
      <li>
        <button class="menu-item" onclick={toggleFavorite}>
          <svg class="menu-icon" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5"><path d="M8 2l2 4 4.5.7-3.3 3.2.8 4.6L8 12.2 3.9 14.5l.8-4.6L1.5 6.7 6 6z"/></svg>
          <span class="item-name">{isFavorited ? 'Unfavorite' : 'Favorite'}</span>
        </button>
      </li>

      <li>
        <button class="menu-item" onclick={openProjectSettings}>
          <svg class="menu-icon" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5"><path d="M8 2.5a1 1 0 0 1 1 1v.2a1 1 0 0 0 .6.9l.3.1a1 1 0 0 0 1.1-.3l.1-.1a1 1 0 0 1 1.5 0 1 1 0 0 1 0 1.5l-.1.1a1 1 0 0 0-.3 1.1l.1.3a1 1 0 0 0 .9.6h.2a1 1 0 1 1 0 2h-.2a1 1 0 0 0-.9.6l-.1.3a1 1 0 0 0 .3 1.1l.1.1a1 1 0 1 1-1.5 1.5l-.1-.1a1 1 0 0 0-1.1-.3l-.3.1a1 1 0 0 0-.6.9v.2a1 1 0 1 1-2 0v-.2a1 1 0 0 0-.6-.9l-.3-.1a1 1 0 0 0-1.1.3l-.1.1a1 1 0 0 1-1.5-1.5l.1-.1a1 1 0 0 0 .3-1.1l-.1-.3a1 1 0 0 0-.9-.6h-.2a1 1 0 0 1 0-2h.2a1 1 0 0 0 .9-.6l.1-.3a1 1 0 0 0-.3-1.1l-.1-.1a1 1 0 0 1 1.5-1.5l.1.1a1 1 0 0 0 1.1.3l.3-.1a1 1 0 0 0 .6-.9v-.2a1 1 0 0 1 1-1z"/><circle cx="8" cy="8" r="2"/></svg>
          <span class="item-name">Project Settings</span>
        </button>
      </li>
      
      <li class="menu-divider"></li>

      <li>
        <button class="menu-item" onclick={newFolder}>
          <svg class="menu-icon" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5">
            <path d="M2 4a1 1 0 011-1h4l1.5 2H13a1 1 0 011 1v7a1 1 0 01-1 1H3a1 1 0 01-1-1V4z"/>
            <path d="M8 7v4M6 9h4" stroke-linecap="round"/>
          </svg>
          <span class="item-name">New Folder</span>
        </button>
      </li>

      <li class="menu-divider"></li>
      <li>
        {#if project.name !== 'Pane System'}
        <button class="menu-item delete-item" onclick={deleteProject}>
          <svg class="menu-icon" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5"><path d="M3 4h10M6 4V3a1 1 0 011-1h2a1 1 0 011 1v1M5 4v8a1 1 0 001 1h4a1 1 0 001-1V4"/></svg>
          <span class="item-name">Delete</span>
        </button>
      {/if}
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

  .menu-arrow {
    width: 12px;
    height: 12px;
    margin-left: auto;
    opacity: 0.5;
    transition: transform 0.2s ease;
  }
  .menu-arrow.rotated {
    transform: rotate(180deg);
  }

  /* Category Dropdown */
  .category-dropdown {
    background: #0A0A0B;
    border-top: 1px solid rgba(255, 255, 255, 0.06);
    margin: 4px 0 0;
  }

  .category-option {
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

  .category-option:hover {
    background: rgba(255, 255, 255, 0.04);
  }

  .category-option.active {
    background: rgba(0, 228, 228, 0.15);
    color: #00E4E4;
  }

  .category-option.remove-category {
    color: #F87171;
  }
  .category-option.remove-category:hover {
    background: rgba(248, 113, 113, 0.1);
  }

  .category-option.no-categories {
    color: #666666;
    font-style: italic;
  }
  .category-option.no-categories:hover {
    background: rgba(255, 255, 255, 0.02);
  }

  .category-icon {
    width: 14px;
    height: 14px;
    margin-right: 10px;
    flex-shrink: 0;
    opacity: 0.7;
  }

  .category-color-dot {
    width: 12px;
    height: 12px;
    border-radius: 50%;
    margin-right: 10px;
    flex-shrink: 0;
  }

  /* Category panel styles removed - now using simple dropdown */
</style>