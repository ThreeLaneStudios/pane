<script lang="ts">
  import type { Task, TaskStatus, TaskPriority, Project } from '$lib/types';
  import * as api from '$lib/api';
  import { onMount } from 'svelte';

  interface Props {
    onTaskCreated?: (task: Task) => void;
    onCancel?: () => void;
    defaultStatus?: TaskStatus;
    defaultProject?: string;
    defaultFolderId?: string;
    defaultProjectId?: string;
    defaultDueDate?: string;
    requireNotebook?: boolean;
  }

  let { onTaskCreated, onCancel, defaultStatus = 'cabinet', defaultProject = '', defaultFolderId = '', defaultProjectId = '', defaultDueDate = '', requireNotebook = false }: Props = $props();

  let title = $state('');
  let status: TaskStatus = $state(defaultStatus);
  let priority: TaskPriority = $state('medium');
  let dueDate = $state(defaultDueDate);
  let submitting = $state(false);
  let selectedFolderId = $state(defaultFolderId);
  let selectedProjectId = $state(defaultProjectId);
  
  let titleInput: HTMLInputElement = $state(null!) as unknown as HTMLInputElement;
  let allProjects: Project[] = $state([]);

  onMount(async () => {
    try {
      allProjects = await api.listProjects();
    } catch (e) {
      console.error('Failed to load projects:', e);
    }
  });

  $effect(() => {
    if (titleInput) {
      titleInput.focus();
      titleInput.select();
    }
  });

  async function handleSubmit() {
    if (!title.trim() || submitting) return;
    if (requireNotebook && !selectedFolderId) return;

    submitting = true;
    try {
      const folder = allProjects.find(p => p.id === selectedFolderId);
      const projectId = selectedProjectId || folder?.parent_id || folder?.id;
      const folderName = folder?.name || defaultProject;

      const noteMeta = await api.createNoteInProject(
        projectId || '',
        selectedFolderId || undefined,
        title.trim(),
        status,
        { priority, dueDate: dueDate || undefined }
      );
      const newTask = api.noteMetaToTask(noteMeta, { project: folderName || undefined });

      onTaskCreated?.(newTask);
      title = '';
      status = defaultStatus;
      priority = 'medium';
      dueDate = '';
    } catch (e) {
      console.error('Failed to create task:', e);
    } finally {
      submitting = false;
    }
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      handleSubmit();
    } else if (e.key === 'Escape') {
      e.preventDefault();
      onCancel?.();
    }
  }

  function getProjectStructure() {
    const roots = allProjects.filter(p => !p.parent_id);
    return roots.map(root => ({
      project: root,
      folders: allProjects.filter(f => f.parent_id === root.id),
    }));
  }
</script>

<div class="quick-add">
  <div class="quick-add-row">
    <input
      bind:this={titleInput}
      bind:value={title}
      onkeydown={handleKeydown}
      placeholder="Task title — press Enter to create"
      class="title-input"
    />
    <div class="inline-controls">
      <select bind:value={status} class="mini-select">
        <option value="cabinet">Cabinet</option>
        <option value="todo">To Do</option>
        <option value="in_progress">In Progress</option>
        <option value="review">Review</option>
        <option value="done">Done</option>
      </select>
      <select bind:value={priority} class="mini-select">
        <option value="low">Low</option>
        <option value="medium">Medium</option>
        <option value="high">High</option>
        <option value="urgent">Urgent</option>
      </select>
      <input type="date" bind:value={dueDate} class="mini-date" />
      {#if requireNotebook || !defaultFolderId}
        <select bind:value={selectedFolderId} class="mini-select notebook-select" class:required-missing={requireNotebook && !selectedFolderId}>
          <option value="">Notebook…</option>
          {#each getProjectStructure() as { project, folders } (project.id)}
            <optgroup label={project.name}>
              {#each folders as folder (folder.id)}
                <option value={folder.id}>{folder.name}</option>
              {/each}
            </optgroup>
          {/each}
        </select>
      {/if}
      <button class="create-btn" onclick={handleSubmit} disabled={!title.trim() || submitting || (requireNotebook && !selectedFolderId)}>
        {submitting ? '…' : '+ Create'}
      </button>
      <button class="cancel-btn" onclick={onCancel}>✕</button>
    </div>
  </div>
</div>

<style>
  .quick-add {
    padding: 12px 24px;
    border-bottom: 1px solid var(--border-color);
    background: var(--bg-secondary);
  }

  .quick-add-row {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
  }

  .title-input {
    flex: 1;
    min-width: 200px;
    padding: 8px 12px;
    border: 1px solid var(--border-color);
    border-radius: 6px;
    background: var(--bg-primary);
    color: var(--text-primary);
    font-size: 14px;
    font-family: inherit;
    outline: none;
  }

  .title-input:focus {
    border-color: var(--accent);
  }

  .inline-controls {
    display: flex;
    align-items: center;
    gap: 6px;
    flex-wrap: wrap;
  }

  .mini-select, .mini-date {
    padding: 5px 8px;
    border: 1px solid var(--border-color);
    border-radius: 4px;
    background: var(--bg-primary);
    color: var(--text-primary);
    font-size: 12px;
    font-family: inherit;
    outline: none;
    cursor: pointer;
  }

  .mini-select:focus, .mini-date:focus {
    border-color: var(--accent);
  }

  .notebook-select.required-missing {
    border-color: #f59e0b;
  }

  .mini-date {
    width: 130px;
  }

  .create-btn {
    padding: 5px 12px;
    border: none;
    border-radius: 4px;
    background: var(--accent);
    color: white;
    font-size: 12px;
    font-weight: 500;
    cursor: pointer;
    white-space: nowrap;
  }

  .create-btn:hover:not(:disabled) {
    background: var(--accent-hover);
  }

  .create-btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .cancel-btn {
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
  }

  .cancel-btn:hover {
    color: var(--text-primary);
    background: var(--bg-hover);
  }
</style>
