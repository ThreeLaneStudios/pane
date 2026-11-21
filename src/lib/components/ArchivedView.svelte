<script lang="ts">
  import { onMount } from 'svelte';
  import * as api from '$lib/api';
  import type { Task, Project } from '$lib/types';

  let archivedTasks: Task[] = $state([]);
  let archivedProjects: Project[] = $state([]);
  let activeTab: 'tasks' | 'projects' = $state('tasks');
  let loading = $state(true);

  onMount(async () => {
    await loadArchived();
  });

  async function loadArchived() {
    loading = true;
    try {
      const [tasks, projects] = await Promise.all([
        api.listArchivedTasks(),
        api.listArchivedProjects()
      ]);
      archivedTasks = tasks;
      archivedProjects = projects;
    } catch (e) {
      console.error('Failed to load archived items:', e);
    }
    loading = false;
  }

  async function restoreTask(taskId: string) {
    try {
      await api.unarchiveTask(taskId);
      archivedTasks = archivedTasks.filter(t => t.id !== taskId);
    } catch (e) { console.error('Failed to restore task:', e); }
  }

  async function restoreProject(projectId: string) {
    try {
      await api.unarchiveProject(projectId);
      archivedProjects = archivedProjects.filter(p => p.id !== projectId);
    } catch (e) { console.error('Failed to restore project:', e); }
  }

  async function permanentDeleteTask(taskId: string) {
    try {
      await api.deleteTask(taskId);
      archivedTasks = archivedTasks.filter(t => t.id !== taskId);
    } catch (e) { console.error('Failed to delete task:', e); }
  }
</script>

<div class="archived-view">
  <div class="archived-header">
    <h2>Archived</h2>
    <p class="subtitle">Items you've archived. Restore or permanently delete.</p>
  </div>

  <div class="tab-bar">
    <button class="tab" class:active={activeTab === 'tasks'} onclick={() => activeTab = 'tasks'}>
      Tasks ({archivedTasks.length})
    </button>
    <button class="tab" class:active={activeTab === 'projects'} onclick={() => activeTab = 'projects'}>
      Projects ({archivedProjects.length})
    </button>
  </div>

  {#if loading}
    <div class="loading">Loading...</div>
  {:else if activeTab === 'tasks'}
    <div class="item-list">
      {#each archivedTasks as task (task.id)}
        <div class="archived-item">
          <div class="item-info">
            <span class="item-title">{task.title}</span>
            <div class="item-meta">
              {#if task.project}<span>Project: {task.project}</span>{/if}
              <span>Status: {task.status.replace('_', ' ')}</span>
            </div>
          </div>
          <div class="item-actions">
            <button class="restore-btn" onclick={() => restoreTask(task.id)}>Restore</button>
            <button class="delete-btn" onclick={() => permanentDeleteTask(task.id)}>Delete</button>
          </div>
        </div>
      {/each}
      {#if archivedTasks.length === 0}
        <div class="empty">No archived tasks</div>
      {/if}
    </div>
  {:else}
    <div class="item-list">
      {#each archivedProjects as project (project.id)}
        <div class="archived-item">
          <div class="item-info">
            <span class="project-dot" style:background={project.color || 'var(--accent)'}></span>
            <span class="item-title">{project.name}</span>
          </div>
          <div class="item-actions">
            <button class="restore-btn" onclick={() => restoreProject(project.id)}>Restore</button>
          </div>
        </div>
      {/each}
      {#if archivedProjects.length === 0}
        <div class="empty">No archived projects</div>
      {/if}
    </div>
  {/if}
</div>

<style>
  .archived-view { flex: 1; display: flex; flex-direction: column; background: var(--bg-primary); }
  .archived-header { padding: 24px; border-bottom: 1px solid var(--border-color); }
  .archived-header h2 { font-size: 22px; font-weight: 600; color: var(--text-primary); margin: 0 0 4px; }
  .subtitle { font-size: 13px; color: var(--text-tertiary); margin: 0; }

  .tab-bar { display: flex; border-bottom: 1px solid var(--border-color); padding: 0 24px; }
  .tab {
    padding: 8px 16px; border: none; background: transparent; color: var(--text-tertiary);
    font-size: 13px; font-weight: 500; cursor: pointer; border-bottom: 2px solid transparent;
  }
  .tab:hover { color: var(--text-primary); }
  .tab.active { color: var(--text-primary); border-bottom-color: var(--accent); }

  .loading { padding: 48px; text-align: center; color: var(--text-tertiary); }

  .item-list { flex: 1; overflow-y: auto; }
  .archived-item {
    display: flex; align-items: center; justify-content: space-between;
    padding: 12px 24px; border-bottom: 1px solid var(--border-color);
    transition: background 0.15s;
  }
  .archived-item:hover { background: var(--bg-hover); }

  .item-info { display: flex; align-items: center; gap: 10px; flex: 1; min-width: 0; }
  .item-title { font-size: 14px; color: var(--text-primary); font-weight: 500; }
  .item-meta { display: flex; gap: 10px; font-size: 11px; color: var(--text-tertiary); margin-top: 2px; }
  .project-dot { width: 10px; height: 10px; border-radius: 50%; flex-shrink: 0; }

  .item-actions { display: flex; gap: 6px; flex-shrink: 0; }
  .restore-btn {
    padding: 4px 10px; border: 1px solid var(--accent); border-radius: 4px;
    background: transparent; color: var(--accent); cursor: pointer; font-size: 12px;
  }
  .restore-btn:hover { background: var(--accent); color: white; }
  .delete-btn {
    padding: 4px 10px; border: 1px solid rgba(239,68,68,0.3); border-radius: 4px;
    background: transparent; color: #ef4444; cursor: pointer; font-size: 12px;
  }
  .delete-btn:hover { background: rgba(239,68,68,0.1); }

  .empty { padding: 48px; text-align: center; color: var(--text-tertiary); font-size: 14px; }
</style>
