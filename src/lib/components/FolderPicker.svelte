<script lang="ts">
  import { onMount } from 'svelte';
  import * as api from '$lib/api';
  import type { Project, Folder } from '$lib/types';

  interface Props {
    selectedFolderId?: string;
    onFolderSelected?: (folderId: string, folderName: string, projectId: string) => void;
    placeholder?: string;
    required?: boolean;
  }

  let { selectedFolderId = '', onFolderSelected, placeholder = 'Select a folder...', required = false }: Props = $props();

  let projects: Project[] = $state([]);
  let folders: { [projectId: string]: Folder[] } = $state({});
  let isOpen = $state(false);
  let selectedFolder: { name: string; projectName: string } | null = $state(null);
  let searchQuery = $state('');
  let filteredOptions = $state<{ projectName: string; folderName: string; folderId: string; projectId: string }[]>([]);

  onMount(async () => {
    await loadProjectsAndFolders();
  });

  async function loadProjectsAndFolders() {
    try {
      projects = await api.listProjects();
      
      // Load folders for each project
      const folderPromises = projects.filter(p => !p.parent_id).map(async project => {
        const projectFolders = await api.listFolders(project.id);
        return { projectId: project.id, folders: projectFolders };
      });
      
      const folderResults = await Promise.all(folderPromises);
      const newFolders: { [projectId: string]: Folder[] } = {};
      
      for (const result of folderResults) {
        newFolders[result.projectId] = result.folders;
      }
      
      folders = newFolders;
      
      // Build filtered options
      updateFilteredOptions();
      
      // Set initial selection if selectedFolderId is provided
      if (selectedFolderId) {
        setSelectedFolder(selectedFolderId);
      }
    } catch (e) {
      console.error('Failed to load projects and folders:', e);
    }
  }

  function updateFilteredOptions() {
    const options: { projectName: string; folderName: string; folderId: string; projectId: string }[] = [];
    
    for (const project of projects.filter(p => !p.parent_id)) {
      const projectFolders = folders[project.id] || [];
      
      for (const folder of projectFolders) {
        const optionText = `${project.name} > ${folder.name}`.toLowerCase();
        if (!searchQuery || optionText.includes(searchQuery.toLowerCase())) {
          options.push({
            projectName: project.name,
            folderName: folder.name,
            folderId: folder.id,
            projectId: project.id
          });
        }
      }
    }
    
    filteredOptions = options;
  }

  function setSelectedFolder(folderId: string) {
    for (const project of projects.filter(p => !p.parent_id)) {
      const projectFolders = folders[project.id] || [];
      const folder = projectFolders.find(f => f.id === folderId);
      
      if (folder) {
        selectedFolder = {
          name: folder.name,
          projectName: project.name
        };
        break;
      }
    }
  }

  function handleSelect(option: { projectName: string; folderName: string; folderId: string; projectId: string }) {
    selectedFolder = {
      name: option.folderName,
      projectName: option.projectName
    };
    isOpen = false;
    searchQuery = '';
    
    if (onFolderSelected) {
      onFolderSelected(option.folderId, option.folderName, option.projectId);
    }
  }

  function handleInputChange() {
    updateFilteredOptions();
  }

  $effect(() => {
    updateFilteredOptions();
  });
</script>

<div class="folder-picker">
  <button 
    type="button"
    class="picker-button"
    class:open={isOpen}
    class:selected={selectedFolder}
    onclick={() => { isOpen = !isOpen; }}
  >
    {#if selectedFolder}
      <span class="selected-text">{selectedFolder.projectName} > {selectedFolder.name}</span>
    {:else}
      <span class="placeholder">{placeholder}</span>
    {/if}
    <span class="arrow">{isOpen ? '▲' : '▼'}</span>
  </button>

  {#if isOpen}
    <div class="picker-dropdown">
      <div class="search-box">
        <input
          type="text"
          bind:value={searchQuery}
          oninput={handleInputChange}
          placeholder="Search folders..."
          autofocus
        />
      </div>
      
      <div class="options-list">
        {#each filteredOptions as option (option.folderId)}
          <button
            type="button"
            class="option-item"
            onclick={() => handleSelect(option)}
          >
            <span class="project-name">{option.projectName}</span>
            <span class="separator">></span>
            <span class="folder-name">{option.folderName}</span>
          </button>
        {:else}
          <div class="no-options">No folders found</div>
        {/each}
      </div>
    </div>
  {/if}
</div>

{#if isOpen}
  <div class="picker-overlay" onclick={() => { isOpen = false; }}></div>
{/if}

<style>
  .folder-picker {
    position: relative;
    width: 100%;
  }

  .picker-button {
    display: flex;
    align-items: center;
    justify-content: space-between;
    width: 100%;
    padding: 8px 12px;
    border: 1px solid var(--border-color);
    border-radius: 6px;
    background: var(--bg-primary);
    color: var(--text-primary);
    font-size: 14px;
    font-family: inherit;
    cursor: pointer;
    transition: all 0.15s ease;
  }

  .picker-button:hover {
    border-color: var(--accent-light);
  }

  .picker-button.open {
    border-color: var(--accent);
    box-shadow: 0 0 0 3px rgba(59, 130, 246, 0.1);
  }

  .placeholder {
    color: var(--text-tertiary);
  }

  .selected-text {
    color: var(--text-primary);
    text-overflow: ellipsis;
    overflow: hidden;
    white-space: nowrap;
  }

  .arrow {
    margin-left: 8px;
    font-size: 10px;
    color: var(--text-tertiary);
    flex-shrink: 0;
  }

  .picker-dropdown {
    position: absolute;
    top: calc(100% + 4px);
    left: 0;
    right: 0;
    background: var(--bg-secondary);
    border: 1px solid var(--border-color);
    border-radius: 8px;
    box-shadow: 0 4px 12px rgba(0, 0, 0, 0.15);
    z-index: 100;
    overflow: hidden;
    max-height: 300px;
  }

  .search-box {
    padding: 12px;
    border-bottom: 1px solid var(--border-color);
  }

  .search-box input {
    width: 100%;
    padding: 6px 10px;
    border: 1px solid var(--border-color);
    border-radius: 4px;
    background: var(--bg-primary);
    color: var(--text-primary);
    font-size: 13px;
    outline: none;
  }

  .search-box input:focus {
    border-color: var(--accent);
  }

  .options-list {
    max-height: 200px;
    overflow-y: auto;
  }

  .option-item {
    display: flex;
    align-items: center;
    gap: 6px;
    width: 100%;
    padding: 10px 12px;
    border: none;
    background: transparent;
    color: var(--text-primary);
    font-size: 13px;
    text-align: left;
    cursor: pointer;
    transition: background 0.15s ease;
  }

  .option-item:hover {
    background: var(--bg-hover);
  }

  .project-name {
    color: var(--text-secondary);
    font-weight: 500;
  }

  .separator {
    color: var(--text-tertiary);
    font-size: 11px;
  }

  .folder-name {
    color: var(--text-primary);
  }

  .no-options {
    padding: 20px 12px;
    text-align: center;
    color: var(--text-tertiary);
    font-size: 13px;
  }

  .picker-overlay {
    position: fixed;
    top: 0;
    left: 0;
    width: 100vw;
    height: 100vh;
    z-index: 50;
  }
</style>