<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { activeProject, activeNotebook, projects as projectsStore } from '$lib/stores/app';
  import { invoke } from '@tauri-apps/api/core';
  import * as api from '$lib/api';
  import type { Project, CategoryWithColor } from '$lib/types';
  import { navigateTo } from '$lib/services/navigation';
  import { formatRelativeTime } from '$lib/utils/time';
  import {
    ENTITY_PROJECT,
    ENTITY_CATEGORY,
    ENTITY_NOTE,
    ENTITY_SHEET,
    ENTITY_FOLDER,
    REASON_DELETED,
    subscribeDataChanged,
    type DataChangedPayload,
  } from '$lib/refreshEvent';

  let allProjects: Project[] = $state([]);
  let categories: CategoryWithColor[] = $state([]);
  let noteCountsMap: Map<string, number> = $state(new Map());

  // Derived: root projects only (no folders), sorted alphabetically, exclude Pane System
  let projects = $derived(
    allProjects
      .filter(p => !p.parent_id && p.name !== 'Pane System')
      .sort((a, b) => a.name.localeCompare(b.name))
  );
  
  let loading = $state(true);

  onMount(async () => {
    await loadData();
  });

  // #3018 (§34.1/§34.2/§34.5): this view was never migrated by #2972
  // stages 1-3 — it had NO `pane-data-changed` listener of any kind before
  // that fix, backend or app channel, for ANY entity type. It shows
  // categories (the per-project `category-indicator` dot, colored via
  // `getCategoryColor()` below, which looks up `project.category` against
  // `categories`), so #3018's "every screen that shows categories must
  // update" ruling applies here. Scoped narrowly to what #3018 required —
  // category events re-read the category list (§34.5: "tiny, no per-item
  // read, deliberate"), and project events patch just the one changed
  // project so a rename/delete's category reassignment repaints this
  // project's dot. #3018 deliberately did NOT attempt the broader note/
  // sheet/folder migration this view still lacked for its "Notes" count
  // column — flagged there as a gap for "a future issue," and #2972 stage
  // 4 (this fix, below) is that issue: there is deliberately no separate
  // tracking issue for it, per Matt's ruling — it belongs in this stage's
  // cross-screen verification pass.
  const unlistenDataChanged = subscribeDataChanged((payload) => {
    handleDataChangedEvent(payload);
  });

  onDestroy(() => {
    unlistenDataChanged.then((fn) => fn());
  });

  async function handleDataChangedEvent(payload: DataChangedPayload) {
    const { entity_id, entity_type, reason } = payload;

    if (entity_type === ENTITY_CATEGORY) {
      try {
        categories = await api.listCategories();
      } catch (e) {
        console.error('Failed to reload categories for pane-data-changed event:', e);
      }
      return;
    }

    // #2972 stage 4 (item 2 — the gap #3018 explicitly left open "for a
    // future issue," closed here with no separate tracking issue, per
    // Matt's ruling): a note/sheet create/edit/delete, or a folder create/
    // edit/delete/move, can change which project a note's count belongs
    // to (or how many notes a project has) without changing the PROJECT
    // entity itself — so the `ENTITY_PROJECT` branch below never fires for
    // these. Re-reading `getProjectNoteCounts()` — one grouped-COUNT query,
    // not `listProjects()`/`listCategories()` — is the same "tiny, no
    // per-item read" shape as the category branch above: it is the
    // smallest query that is still guaranteed correct, because resolving
    // which project+folder owns `entity_id` on the frontend (to patch a
    // single map entry) would need an extra read anyway, and a `deleted`
    // note no longer carries that information once trashed. No full
    // `allProjects`/`categories` refetch happens here — only the counts
    // map this event family can possibly affect.
    if (entity_type === ENTITY_NOTE || entity_type === ENTITY_SHEET || entity_type === ENTITY_FOLDER) {
      try {
        noteCountsMap = await api.getProjectNoteCounts();
      } catch (e) {
        console.error('Failed to reload note counts for pane-data-changed event:', e);
      }
      return;
    }

    if (entity_type !== ENTITY_PROJECT) return;

    if (reason === REASON_DELETED) {
      allProjects = allProjects.filter((p) => p.id !== entity_id);
      return;
    }

    try {
      const entity = await api.getProjectOrFolder(entity_id);
      const idx = allProjects.findIndex((p) => p.id === entity_id);
      if (idx === -1) {
        allProjects = [...allProjects, entity];
      } else {
        allProjects = allProjects.map((p) => (p.id === entity_id ? { ...p, ...entity } : p));
      }
    } catch (e) {
      console.error('Failed to patch project for pane-data-changed event:', entity_id, e);
    }
  }

  async function loadData() {
    loading = true;
    try {
      const [projectsData, categoriesData, noteCounts] = await Promise.all([
        api.listProjects(),
        api.listCategories(),
        api.getProjectNoteCounts(),
      ]);
      
      allProjects = projectsData;
      categories = categoriesData;
      noteCountsMap = noteCounts;

      // is_favorite is a DB column — already loaded with projects, no localStorage needed
    } catch (e) {
      console.error('Failed to load projects:', e);
    }
    loading = false;
  }

  function getCategoryColor(project: Project): string {
    if (!project.category) return '#666666';
    const category = categories.find(c => c.category === project.category);
    return category?.color || '#666666';
  }

  function getNoteCount(projectId: string): number {
    return noteCountsMap.get(projectId) || 0;
  }

  async function togglePin(event: MouseEvent, project: Project) {
    event.stopPropagation();
    const newFavorite = !project.is_favorite;
    try {
      await invoke('update_project_settings', { projectId: project.id, isFavorite: newFavorite });
      // Refresh the projects store so sidebar reacts
      allProjects = await api.listProjects();
      $projectsStore = allProjects;
    } catch (e) {
      console.error('Failed to toggle favorite:', e);
    }
  }

  function openProjectSettings(event: MouseEvent, project: Project) {
    event.stopPropagation();
    navigateTo({
      view: 'project_config',
      projectId: project.id
    });
  }

  function openProject(project: Project) {
    $activeProject = project;
    $activeNotebook = project.name;
    navigateTo({
      view: 'project_config',
      projectId: project.id
    });
  }

  function handleNewProject() {
    navigateTo({ view: 'project_config' });
  }
</script>

<div class="all-projects-view">
  <div class="header">
    <h1>All Projects</h1>
    <button class="new-project-btn" onclick={handleNewProject}>
      <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
        <path d="M5 12h14"></path>
        <path d="M12 5v14"></path>
      </svg>
      New Project
    </button>
  </div>

  {#if loading}
    <div class="loading">Loading projects...</div>
  {:else if projects.length === 0}
    <div class="empty">
      <p>No projects yet</p>
      <button onclick={handleNewProject}>Create your first project</button>
    </div>
  {:else}
    <div class="table-container">
      <table>
        <thead>
          <tr>
            <th class="col-indicator"></th>
            <th class="col-name">Project Name</th>
            <th class="col-notes">Notes</th>
            <th class="col-updated">Last Updated</th>
            <th class="col-actions"></th>
          </tr>
        </thead>
        <tbody>
          {#each projects as project (project.id)}
            {@const isPinned = project.is_favorite}
            <tr onclick={() => openProject(project)}>
              <td class="cell-indicator">
                <div class="category-indicator" style:background={getCategoryColor(project)}></div>
              </td>
              <td class="cell-name">
                <div class="project-name">{project.name}</div>
              </td>
              <td class="cell-notes">
                {getNoteCount(project.id)}
              </td>
              <td class="cell-updated">
                {formatRelativeTime(project.updated_at)}
              </td>
              <td class="cell-actions">
                <div class="actions">
                  <button 
                    class="icon-btn" 
                    class:starred={isPinned}
                    onclick={(e) => togglePin(e, project)}
                    title={isPinned ? 'Unpin from sidebar' : 'Pin to sidebar'}
                  >
                    {#if isPinned}
                      <svg width="16" height="16" viewBox="0 0 24 24" fill="currentColor" stroke="none">
                        <path d="M11.525 2.295a.53.53 0 0 1 .95 0l2.31 4.679a.53.53 0 0 0 .399.29l5.16.753a.53.53 0 0 1 .294.904l-3.733 3.638a.53.53 0 0 0-.152.469l.882 5.14a.53.53 0 0 1-.77.56l-4.614-2.426a.53.53 0 0 0-.494 0l-4.614 2.426a.53.53 0 0 1-.77-.56l.882-5.14a.53.53 0 0 0-.152-.469L3.36 8.921a.53.53 0 0 1 .294-.904l5.16-.753a.53.53 0 0 0 .399-.29z"/>
                      </svg>
                    {:else}
                      <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                        <path d="M11.525 2.295a.53.53 0 0 1 .95 0l2.31 4.679a.53.53 0 0 0 .399.29l5.16.753a.53.53 0 0 1 .294.904l-3.733 3.638a.53.53 0 0 0-.152.469l.882 5.14a.53.53 0 0 1-.77.56l-4.614-2.426a.53.53 0 0 0-.494 0l-4.614 2.426a.53.53 0 0 1-.77-.56l.882-5.14a.53.53 0 0 0-.152-.469L3.36 8.921a.53.53 0 0 1 .294-.904l5.16-.753a.53.53 0 0 0 .399-.29z"/>
                      </svg>
                    {/if}
                  </button>
                  <button 
                    class="icon-btn"
                    onclick={(e) => openProjectSettings(e, project)}
                    title="Project settings"
                  >
                    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                      <path d="M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.39a2 2 0 0 0-.73-2.73l-.15-.08a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2z"/>
                      <circle cx="12" cy="12" r="3"/>
                    </svg>
                  </button>
                </div>
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  {/if}
</div>

<style>
  .all-projects-view {
    height: 100%;
    display: flex;
    flex-direction: column;
    background: #0a0a0f;
    color: #F0EDE8;
  }

  .header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 32px 40px;
  }

  .header h1 {
    font-size: 28px;
    font-weight: 600;
    color: #F0EDE8;
    margin: 0;
  }

  .new-project-btn {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 10px 20px;
    background: #00E4E4;
    color: #0a0a0f;
    border: none;
    border-radius: 8px;
    font-size: 14px;
    font-weight: 500;
    cursor: pointer;
    transition: background 0.2s;
    font-family: inherit;
  }

  .new-project-btn:hover {
    background: #33EEDD;
  }

  .loading,
  .empty {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    flex: 1;
    color: #666666;
    font-size: 14px;
    gap: 12px;
  }

  .empty button {
    padding: 8px 16px;
    border: 1px solid #00E4E4;
    border-radius: 8px;
    background: transparent;
    color: #00E4E4;
    cursor: pointer;
    font-size: 13px;
    font-family: inherit;
  }

  .table-container {
    margin: 0 40px 40px;
    background: rgba(255, 255, 255, 0.05);
    border-radius: 10px;
    border: 1px solid rgba(255, 255, 255, 0.08);
    overflow: hidden;
  }

  table {
    width: 100%;
    border-collapse: collapse;
  }

  thead {
    background: rgba(255, 255, 255, 0.03);
    border-bottom: 1px solid rgba(255, 255, 255, 0.08);
  }

  th {
    text-align: left;
    padding: 16px 20px;
    font-size: 12px;
    font-weight: 600;
    color: #A0A0A0;
    text-transform: uppercase;
    letter-spacing: 0.5px;
  }

  .col-indicator {
    width: 40px;
  }

  .col-notes {
    width: 120px;
  }

  .col-updated {
    width: 180px;
  }

  .col-actions {
    width: 100px;
  }

  tbody tr {
    border-bottom: 1px solid rgba(255, 255, 255, 0.05);
    cursor: pointer;
    transition: background 0.2s;
  }

  tbody tr:last-child {
    border-bottom: none;
  }

  tbody tr:hover {
    background: rgba(255, 255, 255, 0.03);
  }

  tbody tr:hover .actions {
    opacity: 1;
  }

  td {
    padding: 20px;
    font-size: 14px;
  }

  .cell-indicator {
    padding: 20px;
  }

  .category-indicator {
    width: 12px;
    height: 12px;
    border-radius: 2px;
    display: inline-block;
  }

  .cell-name {
    padding: 20px;
  }

  .project-name {
    font-weight: 500;
    color: #F0EDE8;
  }

  .cell-notes,
  .cell-updated {
    color: #A0A0A0;
  }

  .cell-actions {
    padding: 20px;
  }

  .actions {
    display: flex;
    gap: 8px;
    justify-content: flex-end;
    opacity: 0;
    transition: opacity 0.2s;
  }

  .icon-btn {
    background: none;
    border: none;
    color: #A0A0A0;
    cursor: pointer;
    padding: 4px;
    display: flex;
    align-items: center;
    transition: color 0.2s;
  }

  .icon-btn:hover {
    color: #F0EDE8;
  }

  .icon-btn.starred {
    color: #f59e0b;
  }
</style>
