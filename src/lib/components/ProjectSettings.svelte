<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { activeProject, projects } from '$lib/stores/app';
  import * as api from '$lib/api';
  import { navigateTo } from '$lib/services/navigation';
  import { updateProject, getFolders, type Project as FolderRow } from '$lib/services/projectService';
  import type { Project, Agent } from '$lib/types';
  import { renderMarkdown } from '$lib/utils/markdown';
  import AgentPickerRows from '$lib/components/AgentPickerRows.svelte';
  import { filterConnectedAgents } from '$lib/utils/agentDisplay';
  import { ENTITY_CATEGORY, subscribeDataChanged, type DataChangedPayload } from '$lib/refreshEvent';

  interface Props {
    projectId?: string;
  }

  let { projectId }: Props = $props();
  
  // Derive mode from projectId presence
  const mode = $derived(projectId ? 'edit' : 'create');

  // Component state
  let loading = $state(mode === 'edit');
  let saving = $state(false);
  let hasChanges = $state(false);
  let saveSuccess = $state(false);

  // Form fields
  let projectName = $state('');
  let category = $state('');
  let projectBrief = $state('');
  let firstFolderName = $state('General');
  let isFavorite = $state(false);

  // ADR-0062 D2 amendment (2026-08-17): New Project agent row — binds the
  // project's first (born-active) session. Create-mode only; a project's
  // agent choice never applies retroactively to later sessions.
  let pickerAgents = $state<Agent[]>([]);
  let selectedAgentId = $state('');

  // Categories from database
  let categories: {category: string, color: string}[] = $state([]);

  // #2874 item 9: default-folder dropdown. Options are `get_folders(projectId)`
  // minus system folders (ruling 3) minus hidden folders (ruling 3) — the
  // same visibility bar `validate_default_folder_choice` (projects.rs)
  // enforces server-side, so a selection this dropdown offers is never one
  // the backend would then refuse.
  let folderOptions: FolderRow[] = $state([]);
  let defaultFolderId = $state('');

  // Memory viewer overlay
  let showMemoryViewer = $state(false);

  // Pinned notes
  let pinnedNotes: {id: string, title: string, content_preview: string, pinned_at: string, estimated_tokens: number}[] = $state([]);
  let budgetUsage: {system_files_tokens: number, project_brief_tokens: number, pinned_notes_tokens: number, pinned_notes_count: number, total_used: number, total_budget: number, warning: boolean} | null = $state(null);

  // Memory note
  let memoryNote: {id: string, content: string, word_count: number, token_estimate: number, exists: boolean} | null = $state(null);

  // Original values for change detection
  let originalValues = $state({
    name: '',
    category: '',
    brief: '',
    defaultFolderId: ''
  });

  onMount(async () => {
    await loadCategories();
    if (mode === 'edit' && projectId) {
      await loadSettings();
      await loadFolderOptions();
      await loadPinnedNotes();
      await loadBudgetUsage();
      await loadMemory();
    } else {
      await loadAgentPicker();
      loading = false;
    }
  });

  // #2972 stage 4 (§34 inventory): the Category dropdown here
  // (`categories`, loaded via `loadCategories()` above) goes stale if a
  // category is renamed/recolored/deleted elsewhere while this form is
  // open — not one of §34.12's nine screens, never migrated by any prior
  // #2972 stage. Deliberately scoped to ONLY the category list: this
  // component tracks `hasChanges`/`originalValues` for its own `category`/
  // `projectName`/`projectBrief` fields (dirty-form state) — silently
  // overwriting those from an incoming PROJECT event while the user is
  // mid-edit would clobber unsaved input, a worse regression than the
  // staleness bug being fixed. The dropdown's available options carry no
  // such risk (same "tiny, no per-item read, deliberate" rationale
  // Sidebar/ProjectsView/Settings already use).
  const unlistenDataChanged = subscribeDataChanged((payload: DataChangedPayload) => {
    if (payload.entity_type === ENTITY_CATEGORY) {
      loadCategories();
    }
  });

  onDestroy(() => {
    unlistenDataChanged.then((fn) => fn());
  });

  async function loadCategories() {
    try {
      categories = await invoke('list_categories');
    } catch (e) {
      console.error('Failed to load categories:', e);
      categories = [];
    }
  }

  async function loadAgentPicker() {
    try {
      const [agents, connections] = await Promise.all([
        api.getAgents(),
        api.listRuntimeConnections(),
      ]);
      pickerAgents = filterConnectedAgents(agents, connections);
      selectedAgentId = pickerAgents.find(a => a.is_default)?.id ?? pickerAgents[0]?.id ?? '';
    } catch (e) {
      console.error('Failed to load agents for New Project picker:', e);
      pickerAgents = [];
    }
  }

  async function loadPinnedNotes() {
    if (mode === 'edit' && projectId) {
      try {
        pinnedNotes = await invoke('list_context_pins', { projectId });
      } catch (e) {
        console.error('Failed to load pinned notes:', e);
        pinnedNotes = [];
      }
    }
  }

  async function loadBudgetUsage() {
    if (mode === 'edit' && projectId) {
      try {
        budgetUsage = await invoke('get_context_budget_usage', { projectId });
      } catch (e) {
        console.error('Failed to load budget usage:', e);
        budgetUsage = null;
      }
    }
  }

  async function loadMemory() {
    if (mode === 'edit' && projectId) {
      try {
        memoryNote = await invoke('get_project_memory', { projectId });
      } catch (e) {
        console.error('Failed to load Memory note:', e);
        memoryNote = { exists: false, content: '', word_count: 0, token_estimate: 0, id: '' };
      }
    }
  }

  async function loadSettings() {
    if (!projectId) return;
    try {
      loading = true;
      const settings = await invoke<{
        id: string;
        name: string;
        color: string | null;
        category: string | null;
        is_favorite: boolean;
        description: string | null;
      }>('get_project_settings', { projectId });

      projectName = settings.name;
      category = settings.category || '';
      // Project brief is stored in projects.description — read it directly from the response
      projectBrief = settings.description ?? '';
      isFavorite = settings.is_favorite ?? false;

      // #2874 item 9: `get_project_settings` doesn't carry `default_folder_id`
      // (it's not a "settings form" field, it's a folder pointer) — read it
      // fresh via `get_project_or_folder` instead of trusting a possibly-stale
      // `$projects` store entry, same "fetch fresh at display time" rule
      // `getFolderDeletePreview` already follows (Sidebar.svelte).
      try {
        const current = await api.getProjectOrFolder(projectId);
        defaultFolderId = current.default_folder_id ?? '';
      } catch (e) {
        console.error('Failed to load default folder:', e);
        defaultFolderId = '';
      }

      // Store original values
      originalValues = {
        name: projectName,
        category,
        brief: projectBrief,
        defaultFolderId
      };

      hasChanges = false;
    } catch (e) {
      console.error('Failed to load project settings:', e);
    } finally {
      loading = false;
    }
  }

  async function loadFolderOptions() {
    if (!projectId) return;
    try {
      const [folders, hiddenIds] = await Promise.all([
        getFolders(projectId),
        api.getHiddenFolderIds()
      ]);
      const hiddenSet = new Set(hiddenIds);
      // Ruling 3: exclude system AND hidden folders from the dropdown's options.
      folderOptions = folders.filter(f => !f.is_system && !hiddenSet.has(f.id));
    } catch (e) {
      console.error('Failed to load folder options:', e);
      folderOptions = [];
    }
  }

  // Detect changes (only in edit mode)
  $effect(() => {
    // Only detect changes in edit mode
    if (mode === 'edit') {
      hasChanges = 
        projectName !== originalValues.name ||
        category !== originalValues.category ||
        projectBrief !== originalValues.brief ||
        defaultFolderId !== originalValues.defaultFolderId;
    }
  });

  async function handleSave() {
    if (mode === 'create') {
      await handleCreate();
      return;
    }
    
    if (!hasChanges) return;
    
    try {
      saving = true;
      saveSuccess = false;

      // Route name changes through update_project so fs::rename + notes.file_path repair fires.
      // #2874 item 9: default-folder changes route through the same command
      // (validated server-side by validate_default_folder_choice) — combined
      // into one call when both changed, to avoid two separate update_project
      // round-trips for one save.
      const nameChanged = projectName !== originalValues.name;
      const defaultFolderChanged = defaultFolderId !== originalValues.defaultFolderId;
      if (nameChanged || defaultFolderChanged) {
        await updateProject({
          project_id: projectId!,
          ...(nameChanged ? { name: projectName || undefined } : {}),
          ...(defaultFolderChanged ? { default_folder_id: defaultFolderId } : {})
        });
      }

      // AI settings are owned by update_project_settings; name is no longer passed here
      // description = projectBrief: pass empty string to clear, string value to set
      await invoke('update_project_settings', {
        projectId,
        name: null,
        // A1 fix (#2141): send category as-is; do NOT coerce empty string to null.
        // The Rust handler uses Some("") = clear, Some(value) = set, None = don't touch.
        // The previous `|| null` made clearing impossible (null = None = skip in Rust).
        category: category,
        isFavorite: isFavorite,
        description: projectBrief
      });

      // Update original values
      originalValues = {
        name: projectName,
        category,
        brief: projectBrief,
        defaultFolderId
      };

      hasChanges = false;
      saveSuccess = true;

      // #2972 stage 3 (item 5): duplicate Tauri event-plugin dispatch
      // deleted, not converted. The backend `update_project_settings`
      // command (ai/commands.rs) invoked above already emits the single
      // data-changed event itself unconditionally (#2972 stage 1,
      // §34.1.1) — this frontend side used to ALSO fire a
      // `pane-project-changed` event for the exact same mutation; that was
      // a second, redundant notification, so it is gone, not rewritten.

      // Clear success message after 3s
      setTimeout(() => {
        saveSuccess = false;
      }, 3000);
    } catch (e) {
      console.error('Failed to save settings:', e);
      alert('Failed to save settings: ' + e);
    } finally {
      saving = false;
    }
  }

  async function handleCreate() {
    if (!projectName.trim()) {
      alert('Please enter a project name');
      return;
    }
    if (!selectedAgentId) {
      alert('Please choose an agent for the first session');
      return;
    }
    
    try {
      saving = true;
      
      const project = await invoke<Project>('create_project', {
        request: {
          name: projectName.trim(),
          description: projectBrief.trim() || null,
          color: null,
          category: category || null,
          first_folder_name: firstFolderName.trim() || 'General',
          is_favorite: isFavorite,
          agent_id: selectedAgentId
        }
      });
      
      // Navigate to the new project
      navigateTo({ view: 'project_config', projectId: project.id });
    } catch (e) {
      console.error('Failed to create project:', e);
      alert('Failed to create project: ' + e);
    } finally {
      saving = false;
    }
  }

  function handleDiscard() {
    projectName = originalValues.name;
    category = originalValues.category;
    projectBrief = originalValues.brief;
  }

  function goBack() {
    navigateTo({ view: 'project_list' });
  }

  async function toggleFavorite() {
    if (mode === 'create') {
      isFavorite = !isFavorite;
      return;
    }
    
    if (!projectId) return;
    
    const newFavorite = !isFavorite;
    isFavorite = newFavorite; // optimistic update
    try {
      await invoke('update_project_settings', { projectId, isFavorite: newFavorite });
      // Refresh $projects so the sidebar reacts immediately (#2009)
      $projects = await api.listProjects();
    } catch (e) {
      console.error('Failed to toggle favorite:', e);
      isFavorite = !newFavorite; // revert on error
    }
  }

  // Reference file handlers
  async function handleUnpinNote(noteId: string) {
    try {
      await invoke('unpin_note_from_context', { noteId });
      await loadPinnedNotes();
      await loadBudgetUsage();
    } catch (e) {
      console.error('Failed to unpin note:', e);
      alert('Failed to unpin note: ' + e);
    }
  }

  function handleNavigateToNote(noteId: string) {
    navigateTo({ view: 'note_editor', noteId });
  }

  function openMemoryViewer() {
    showMemoryViewer = true;
  }
</script>

{#if loading}
  <div class="loading-container">
    <div class="loading-spinner"></div>
    <p>Loading settings...</p>
  </div>
{:else}
  <div class="config-flow">
    <div class="page-header">
      <button class="back-link" onclick={goBack}>
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
          <path d="m15 18-6-6 6-6"/>
        </svg>
        All Projects
      </button>
      <div class="title-row">
        <h1 class="page-title">{mode === 'create' ? 'New Project' : projectName}</h1>
        {#if mode === 'edit'}
          <button class="favorite-btn" class:favorited={isFavorite} onclick={toggleFavorite} title={isFavorite ? 'Remove from favorites' : 'Add to favorites'}>
            {isFavorite ? '★' : '☆'}
          </button>
        {/if}
      </div>
      <p class="page-subtitle">{mode === 'create' ? 'Create a new project' : 'Project configuration and AI context'}</p>
    </div>

    <!-- Card 1: Project Settings (MOVED TO FIRST) -->
    <div class="card">
      <div class="card-header">
        <div class="card-icon">
          <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
            <path d="M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.39a2 2 0 0 0-.73-2.73l-.15-.08a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2z"/><circle cx="12" cy="12" r="3"/>
          </svg>
        </div>
        <h2 class="card-title">Project Settings</h2>
      </div>
      <div class="card-body">
        <div class="input-group">
          <label class="label">Project Name</label>
          <input type="text" class="input" bind:value={projectName} placeholder={mode === 'create' ? 'Enter project name...' : ''}>
        </div>
        {#if mode === 'create'}
          <div class="input-group">
            <label class="label">First Folder Name</label>
            <input type="text" class="input" bind:value={firstFolderName} placeholder="General">
            <div class="hint">This folder will be created automatically in your new project</div>
          </div>
          <!-- ADR-0062 D2 amendment: New Project agent row (design pc-d.jsx PcNewProject) -->
          <div class="input-group">
            <label class="label">Agent for the first session</label>
            {#if pickerAgents.length > 0}
              <div class="new-project-agent-picker">
                <AgentPickerRows
                  agents={pickerAgents}
                  selectedId={selectedAgentId}
                  onSelect={(id) => { selectedAgentId = id; }}
                />
              </div>
              <div class="hint">The project's first session stays with this agent. Later sessions in the project choose their own.</div>
            {:else}
              <div class="hint">No agents available yet — set one up in Settings first.</div>
            {/if}
          </div>
        {/if}
        <div class="input-group">
          <label class="label">Category</label>
          {#if categories.length === 0}
            <div class="empty-category-hint">
              <p>No categories —
                <!-- C4 (#2141): actionable deep-link to Settings → Categories -->
                <button
                  class="settings-deeplink"
                  onclick={() => navigateTo({ view: 'settings' })}
                >
                  Create them in Settings → Categories
                  <svg class="deeplink-arrow" width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">
                    <polyline points="9 18 15 12 9 6"/>
                  </svg>
                </button>
              </p>
            </div>
          {:else}
            <div class="category-select-wrapper">
              <select class="input" bind:value={category}>
                <option value="">No category</option>
                {#each categories as cat}
                  <option value={cat.category}>{cat.category}</option>
                {/each}
              </select>
              {#if category}
                {@const selectedCat = categories.find(c => c.category === category)}
                {#if selectedCat}
                  <div class="category-swatch" style:background={selectedCat.color}></div>
                {/if}
              {/if}
            </div>
            <div class="hint">Category determines the project's color in the sidebar</div>
          {/if}
        </div>
        {#if mode === 'edit'}
          <div class="input-group">
            <label class="label">Default folder</label>
            {#if folderOptions.length === 0}
              <div class="hint">No folders available.</div>
            {:else}
              <select class="input" bind:value={defaultFolderId}>
                {#each folderOptions as f}
                  <option value={f.id}>{f.name}</option>
                {/each}
              </select>
              <div class="hint">New notes and sheets land here by default. The default folder can't be deleted or hidden.</div>
            {/if}
          </div>
        {/if}
        {#if mode === 'create'}
          <div class="input-group">
            <label class="toggle-label">
              <input type="checkbox" class="toggle-checkbox" bind:checked={isFavorite}>
              <span class="toggle-text">Add to favorites (show in sidebar)</span>
            </label>
          </div>
        {/if}
      </div>
    </div>

    <!-- Card 2: AI Model Settings (REMOVED v0.32.3 #458) -->
    <!-- Per-project model selection hidden from UI. Smart routing is now global only. -->
    <!-- Backend support available in projects table columns for future re-enablement. -->

    <!-- Card 3: Project Brief -->
    <div class="card">
      <div class="card-header">
        <div class="card-icon">
          <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
            <path d="M15 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7Z"/><path d="M14 2v4a2 2 0 0 0 2 2h4"/>
          </svg>
        </div>
        <h2 class="card-title">Project Brief</h2>
      </div>
      <div class="card-body">
        <div class="input-group">
          <label class="label">Brief Content</label>
          <textarea 
            class="input textarea" 
            placeholder="Enter AI instructions and project overview..."
            bind:value={projectBrief}
          ></textarea>
          <div class="hint">This brief guides the AI agent working on this project</div>
        </div>
      </div>
    </div>

    <!-- Card 4: Pinned to Context — EDIT MODE ONLY -->
    {#if mode === 'edit'}
    <div class="card">
      <div class="card-header">
        <div class="card-icon">
          <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
            <path d="M12 17v5"/><path d="M9 10.76a2 2 0 0 1-1.11 1.79l-1.78.9A2 2 0 0 0 5 15.24V16a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1v-.76a2 2 0 0 0-1.11-1.79l-1.78-.9A2 2 0 0 1 15 10.76V7a1 1 0 0 1 1-1 2 2 0 0 0 0-4H8a2 2 0 0 0 0 4 1 1 0 0 1 1 1z"/>
          </svg>
        </div>
        <h2 class="card-title">Pinned to Context</h2>
      </div>
      <div class="card-body">
        {#if pinnedNotes.length === 0}
          <p class="hint">Pin notes to inject their content into every AI conversation in this project.</p>
        {:else}
          <div class="pins-list">
            {#each pinnedNotes as pin}
              <div class="pin-item">
                <div class="pin-item-info" onclick={() => handleNavigateToNote(pin.id)}>
                  <div class="pin-item-name">{pin.title}</div>
                  <div class="pin-item-preview">{pin.content_preview}</div>
                </div>
                <div class="pin-item-meta">
                  <span class="token-badge">{pin.estimated_tokens.toLocaleString()} tokens</span>
                  <button class="unpin-btn" title="Unpin" onclick={() => handleUnpinNote(pin.id)}>×</button>
                </div>
              </div>
            {/each}
          </div>
          
          {#if budgetUsage && budgetUsage.pinned_notes_tokens > 0}
            <div class="budget-display">
              <span class="budget-total">{budgetUsage.pinned_notes_tokens.toLocaleString()} tokens in context</span>
            </div>
          {/if}
        {/if}
      </div>
    </div>
    {/if}

    <!-- Card 5: Memory — EDIT MODE ONLY -->
    {#if mode === 'edit'}
    <div class="card">
      <div class="card-header">
        <div class="card-icon">
          <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
            <path d="M12 2a7 7 0 0 0-7 7c0 2.38 1.19 4.47 3 5.74V17a1 1 0 0 0 1 1h6a1 1 0 0 0 1-1v-2.26c1.81-1.27 3-3.36 3-5.74a7 7 0 0 0-7-7z"/>
          </svg>
        </div>
        <h2 class="card-title">Memory</h2>
      </div>
      <div class="card-body">
        <p class="hint">AI-synthesized memory from conversations about this project</p>
        
        {#if memoryNote}
          {#if memoryNote.exists}
            <div class="memory-viewer">
              <div class="memory-header">
                <span class="token-badge">{memoryNote.token_estimate.toLocaleString()} tokens</span>
                {#if memoryNote.word_count > 3000}
                  <span class="memory-health-badge warning">⚠️ Long ({memoryNote.word_count} words)</span>
                {:else if memoryNote.word_count > 2500}
                  <span class="memory-health-badge notice">📊 Growing ({memoryNote.word_count} words)</span>
                {/if}
              </div>
              <div class="memory-content">
                {@html renderMarkdown(memoryNote.content)}
              </div>
              <button class="btn btn-secondary" onclick={openMemoryViewer}>
                Open in Viewer
              </button>
              <div class="memory-info-note">
                ℹ️ Memory is auto-regenerated from facts extracted during chat. To add new facts, use "Save to Memory" in chat. To edit or remove existing facts, use the Memory tab in Settings.
              </div>
            </div>
          {:else}
            <div class="memory-empty">
              <p>No memory yet — start a conversation about this project to build memory automatically</p>
            </div>
          {/if}
        {:else}
          <div class="memory-loading">Loading Memory...</div>
        {/if}
      </div>
    </div>
    {/if}

    <!-- Sticky save bar -->
    <div class="sticky-save">
      <div class="actions">
        {#if mode === 'create'}
          <button 
            class="btn btn-primary" 
            onclick={handleSave}
            disabled={!projectName.trim() || saving}
          >
            {#if saving}
              Creating...
            {:else}
              Create Project
            {/if}
          </button>
          <button 
            class="btn btn-ghost" 
            onclick={goBack}
            disabled={saving}
          >
            Cancel
          </button>
        {:else}
          <button 
            class="btn btn-primary" 
            onclick={handleSave}
            disabled={!hasChanges || saving}
          >
            {#if saving}
              Saving...
            {:else if saveSuccess}
              ✓ Saved
            {:else}
              Save All Changes
            {/if}
          </button>
          <button 
            class="btn btn-ghost" 
            onclick={handleDiscard}
            disabled={!hasChanges || saving}
          >
            Discard Changes
          </button>
        {/if}
      </div>
    </div>
  </div>
{/if}

{#if showMemoryViewer}
<div
  class="mem-overlay"
  role="dialog"
  aria-modal="true"
  aria-label="Project Memory (read-only)"
  onkeydown={(e) => { if (e.key === 'Escape') showMemoryViewer = false; }}
  tabindex="-1"
>
  <div class="mem-panel">
    <div class="mem-header">
      <h3 class="mem-title">Project Memory <span class="mem-readonly-badge">read-only</span></h3>
      {#if memoryNote?.token_estimate}
        <span class="mem-token-badge">{memoryNote.token_estimate.toLocaleString()} tokens</span>
      {/if}
      <button class="mem-close" onclick={() => showMemoryViewer = false} aria-label="Close memory viewer">×</button>
    </div>
    <div class="mem-body">
      {#if memoryNote?.content}
        {@html renderMarkdown(memoryNote.content)}
      {:else}
        <p class="mem-empty">No content yet.</p>
      {/if}
    </div>
  </div>
</div>
{/if}

<style>
  .loading-container {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    min-height: 400px;
    color: #A0A0A0;
  }

  .loading-spinner {
    width: 40px;
    height: 40px;
    border: 3px solid rgba(0, 228, 228, 0.2);
    border-top-color: #00E4E4;
    border-radius: 50%;
    animation: spin 1s linear infinite;
    margin-bottom: 16px;
  }

  @keyframes spin {
    to { transform: rotate(360deg); }
  }

  .config-flow {
    max-width: 720px;
    margin: 0 auto;
    padding: 48px 24px;
  }

  .page-header {
    margin-bottom: 40px;
  }

  .title-row {
    display: flex;
    align-items: center;
    gap: 12px;
  }

  .back-link {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    color: #A0A0A0;
    font-size: 13px;
    background: none;
    border: none;
    padding: 0;
    cursor: pointer;
    margin-bottom: 16px;
    transition: color 0.15s;
    font-family: inherit;
  }

  .back-link:hover {
    color: #F0EDE8;
  }

  .page-title {
    font-size: 32px;
    font-weight: 700;
    color: #F0EDE8;
    margin-bottom: 0;
  }

  .favorite-btn {
    background: none;
    border: none;
    font-size: 28px;
    color: #666666;
    cursor: pointer;
    padding: 0;
    transition: all 0.15s;
    line-height: 1;
  }

  .favorite-btn:hover {
    color: #f59e0b;
    transform: scale(1.1);
  }

  .favorite-btn.favorited {
    color: #f59e0b;
  }

  .page-subtitle {
    font-size: 15px;
    color: #666666;
    margin-top: 8px;
  }

  .card {
    background: rgba(255,255,255,0.05);
    border: 1px solid rgba(255,255,255,0.08);
    border-radius: 12px;
    padding: 28px;
    margin-bottom: 20px;
    transition: all 0.2s;
  }

  .card:hover {
    background: rgba(255,255,255,0.06);
    border-color: rgba(255,255,255,0.12);
  }

  .card-header {
    display: flex;
    align-items: center;
    gap: 12px;
    margin-bottom: 20px;
    padding-bottom: 16px;
    border-bottom: 1px solid rgba(255,255,255,0.08);
  }

  .card-icon {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 36px;
    height: 36px;
    background: rgba(0,228,228,0.1);
    border-radius: 8px;
    color: #00E4E4;
  }

  .card-title {
    font-size: 18px;
    font-weight: 600;
    color: #F0EDE8;
  }

  .input-group {
    margin-bottom: 20px;
  }

  .input-group:last-child {
    margin-bottom: 0;
  }

  .label {
    display: block;
    font-size: 13px;
    font-weight: 600;
    color: #F0EDE8;
    margin-bottom: 8px;
  }

  .input {
    width: 100%;
    padding: 11px 14px;
    background: rgba(255,255,255,0.05);
    border: 1px solid rgba(255,255,255,0.08);
    border-radius: 8px;
    color: #F0EDE8;
    font-size: 14px;
    font-family: inherit;
    transition: all 0.15s;
  }

  .input:focus {
    outline: none;
    background: rgba(255,255,255,0.08);
    border-color: #00E4E4;
    box-shadow: 0 0 0 3px rgba(0,228,228,0.08);
  }

  select.input {
    cursor: pointer;
  }

  .new-project-agent-picker {
    background: rgba(255,255,255,0.03);
    border: 1px solid rgba(255,255,255,0.08);
    border-radius: 8px;
    padding: 4px;
    max-height: 260px;
    overflow-y: auto;
  }

  .textarea {
    min-height: 130px;
    resize: vertical;
    font-family: inherit;
    line-height: 1.6;
  }

  .hint {
    font-size: 12px;
    color: #666666;
    margin-top: 6px;
  }

  .empty-category-hint {
    background: rgba(255,255,255,0.02);
    border: 1px solid rgba(255,255,255,0.06);
    border-radius: 8px;
    padding: 14px;
    color: #666666;
    font-size: 13px;
    text-align: center;
  }

  /* C4 (#2141): deep-link to Settings → Categories */
  .settings-deeplink {
    display: inline-flex;
    align-items: center;
    gap: 3px;
    background: none;
    border: none;
    padding: 0;
    cursor: pointer;
    color: var(--accent, #00E4E4);
    font-size: inherit;
    font-family: inherit;
    text-decoration: none;
    outline: none;
    border-radius: 3px;
    transition: color 0.15s ease;
  }

  .settings-deeplink:hover {
    color: var(--accent-hover, #00c8c8);
    text-decoration: underline;
  }

  .settings-deeplink:focus-visible {
    outline: 2px solid var(--accent, #00E4E4);
    outline-offset: 2px;
  }

  .settings-deeplink .deeplink-arrow {
    transition: transform 0.15s ease;
  }

  .settings-deeplink:hover .deeplink-arrow {
    transform: translateX(2px);
  }

  .memory-content {
    background: rgba(255,255,255,0.02);
    border: 1px solid rgba(255,255,255,0.06);
    border-radius: 8px;
    padding: 18px;
    font-size: 13px;
    color: #A0A0A0;
    line-height: 1.7;
  }

  .memory-content p {
    margin-bottom: 12px;
  }

  .memory-content p:last-child {
    margin-bottom: 0;
  }

  .memory-content strong {
    color: #F0EDE8;
  }

  .memory-placeholder {
    font-style: italic;
    color: #666666;
  }

  .category-select-wrapper {
    position: relative;
    display: flex;
    align-items: center;
    gap: 12px;
  }

  .category-select-wrapper select {
    flex: 1;
  }

  .category-swatch {
    width: 32px;
    height: 32px;
    border-radius: 6px;
    flex-shrink: 0;
    box-shadow: 0 2px 4px rgba(0,0,0,0.3);
  }

  .toggle-label {
    display: flex;
    align-items: center;
    gap: 10px;
    cursor: pointer;
    margin-bottom: 0;
  }

  .toggle-checkbox {
    width: 16px;
    height: 16px;
    cursor: pointer;
  }

  .toggle-checkbox:disabled {
    opacity: 0.3;
    cursor: not-allowed;
  }

  .toggle-text {
    font-weight: 400;
  }

  .sticky-save {
    position: sticky;
    bottom: 0;
    left: 0;
    right: 0;
    padding: 20px 0;
    background: linear-gradient(to top, #0a0a0f 80%, transparent);
    margin-top: 32px;
  }

  .actions {
    display: flex;
    gap: 10px;
  }

  .btn {
    padding: 11px 22px;
    border-radius: 8px;
    font-size: 14px;
    font-weight: 600;
    cursor: pointer;
    transition: all 0.15s;
    border: none;
    font-family: inherit;
  }

  .btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .btn-primary {
    background: #00E4E4;
    color: #0a0a0f;
  }

  .btn-primary:not(:disabled):hover {
    background: #33EEDD;
    transform: translateY(-1px);
    box-shadow: 0 4px 12px rgba(0,228,228,0.3);
  }

  .btn-ghost {
    background: transparent;
    color: #A0A0A0;
    border: 1px solid rgba(255,255,255,0.08);
  }

  .btn-ghost:not(:disabled):hover {
    background: rgba(255,255,255,0.05);
    color: #F0EDE8;
  }

  .btn-secondary {
    background: transparent;
    color: #A0A0A0;
    border: 1px solid rgba(255,255,255,0.08);
  }

  .btn-secondary:hover {
    background: rgba(255,255,255,0.05);
    color: #F0EDE8;
  }

  /* Pinned Notes */
  .pins-list {
    display: flex;
    flex-direction: column;
    gap: 10px;
    margin-bottom: 20px;
  }

  .pin-item {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 12px 16px;
    background: rgba(255,255,255,0.03);
    border: 1px solid rgba(255,255,255,0.06);
    border-radius: 8px;
    transition: all 0.15s;
  }

  .pin-item:hover {
    background: rgba(255,255,255,0.06);
    border-color: rgba(255,255,255,0.12);
  }

  .pin-item-info {
    flex: 1;
    cursor: pointer;
    min-width: 0;
  }

  .pin-item-name {
    font-size: 14px;
    font-weight: 500;
    color: #F0EDE8;
    margin-bottom: 4px;
  }

  .pin-item-preview {
    font-size: 12px;
    color: #666666;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .pin-item-meta {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-shrink: 0;
  }

  .token-badge {
    font-size: 11px;
    font-weight: 600;
    color: #A0A0A0;
    background: rgba(255,255,255,0.05);
    padding: 4px 8px;
    border-radius: 4px;
  }

  .unpin-btn {
    background: rgba(255,255,255,0.05);
    border: none;
    border-radius: 4px;
    width: 24px;
    height: 24px;
    cursor: pointer;
    color: #A0A0A0;
    font-size: 18px;
    line-height: 1;
    transition: all 0.15s;
    display: flex;
    align-items: center;
    justify-content: center;
  }

  .unpin-btn:hover {
    background: rgba(239,68,68,0.2);
    color: #ef4444;
  }

  .budget-display {
    margin-top: 12px;
    text-align: right;
  }

  .budget-total {
    color: #A0A0A0;
    font-size: 12px;
  }

  /* Memory */
  .memory-viewer {
    display: flex;
    flex-direction: column;
    gap: 16px;
    margin-top: 16px;
  }

  .memory-header {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
  }

  .memory-health-badge {
    font-size: 11px;
    font-weight: 600;
    padding: 4px 8px;
    border-radius: 4px;
  }

  .memory-health-badge.notice {
    background: rgba(59,130,246,0.15);
    color: #60a5fa;
  }

  .memory-health-badge.warning {
    background: rgba(239,68,68,0.15);
    color: #ef4444;
  }

  .memory-content {
    background: rgba(255,255,255,0.03);
    border: 1px solid rgba(255,255,255,0.06);
    border-radius: 8px;
    padding: 16px;
    max-height: 300px;
    overflow-y: auto;
    font-size: 14px;
    line-height: 1.6;
    color: #D0D0D0;
  }

  .memory-content :global(h1),
  .memory-content :global(h2),
  .memory-content :global(h3) {
    color: #F0EDE8;
    margin-top: 1em;
    margin-bottom: 0.5em;
  }

  .memory-content :global(h1) { font-size: 1.4em; }
  .memory-content :global(h2) { font-size: 1.2em; }
  .memory-content :global(h3) { font-size: 1.1em; }

  .memory-content :global(p) {
    margin-bottom: 0.8em;
  }

  .memory-content :global(ul),
  .memory-content :global(ol) {
    margin-left: 1.5em;
    margin-bottom: 0.8em;
  }

  .memory-content :global(code) {
    background: rgba(255,255,255,0.05);
    padding: 2px 6px;
    border-radius: 3px;
    font-family: 'SF Mono', Consolas, monospace;
    font-size: 0.9em;
  }

  .memory-content :global(pre) {
    background: rgba(255,255,255,0.05);
    padding: 12px;
    border-radius: 6px;
    overflow-x: auto;
    margin-bottom: 0.8em;
  }

  .memory-content :global(pre code) {
    background: none;
    padding: 0;
  }

  .memory-content :global(a) {
    color: #60a5fa;
    text-decoration: none;
  }

  .memory-content :global(a:hover) {
    text-decoration: underline;
  }

  /* Custom scrollbar for memory content */
  .memory-content::-webkit-scrollbar {
    width: 8px;
  }

  .memory-content::-webkit-scrollbar-track {
    background: rgba(255,255,255,0.03);
    border-radius: 4px;
  }

  .memory-content::-webkit-scrollbar-thumb {
    background: rgba(255,255,255,0.1);
    border-radius: 4px;
  }

  .memory-content::-webkit-scrollbar-thumb:hover {
    background: rgba(255,255,255,0.15);
  }

  .memory-empty {
    padding: 20px;
    text-align: center;
    color: #666666;
    font-size: 14px;
    background: rgba(255,255,255,0.02);
    border: 1px dashed rgba(255,255,255,0.08);
    border-radius: 8px;
    margin-top: 16px;
  }

  .memory-loading {
    padding: 20px;
    text-align: center;
    color: #666666;
    font-size: 14px;
  }

  .memory-info-note {
    font-size: 12px;
    color: rgba(255, 255, 255, 0.5);
    padding: 8px 12px;
    background: rgba(255, 255, 255, 0.03);
    border: 1px solid rgba(255, 255, 255, 0.06);
    border-radius: 6px;
    margin-top: 8px;
    line-height: 1.5;
  }

  /* Memory Viewer Overlay */
  .mem-overlay {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.7);
    z-index: 9999;
    display: flex;
    align-items: center;
    justify-content: center;
  }

  .mem-panel {
    background: #16161e;
    border: 1px solid rgba(255, 255, 255, 0.1);
    border-radius: 12px;
    width: min(720px, 90vw);
    max-height: 80vh;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  .mem-header {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 16px 20px;
    border-bottom: 1px solid rgba(255, 255, 255, 0.07);
    flex-shrink: 0;
  }

  .mem-title {
    font-size: 15px;
    font-weight: 600;
    color: #F0EDE8;
    margin: 0;
    flex: 1;
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .mem-readonly-badge {
    font-size: 11px;
    font-weight: 500;
    color: #A0A0A0;
    background: rgba(255, 255, 255, 0.06);
    border: 1px solid rgba(255, 255, 255, 0.08);
    border-radius: 4px;
    padding: 2px 6px;
  }

  .mem-token-badge {
    font-size: 11px;
    color: #A0A0A0;
    background: rgba(255, 255, 255, 0.04);
    border: 1px solid rgba(255, 255, 255, 0.08);
    border-radius: 4px;
    padding: 2px 6px;
    flex-shrink: 0;
  }

  .mem-close {
    background: none;
    border: none;
    color: #A0A0A0;
    font-size: 20px;
    line-height: 1;
    cursor: pointer;
    padding: 4px 8px;
    border-radius: 4px;
    transition: color 0.15s, background 0.15s;
    flex-shrink: 0;
  }

  .mem-close:hover {
    color: #F0EDE8;
    background: rgba(255, 255, 255, 0.06);
  }

  .mem-body {
    flex: 1;
    overflow-y: auto;
    padding: 20px;
    color: #C0BDB8;
    font-size: 14px;
    line-height: 1.7;
  }

  .mem-body :global(h1),
  .mem-body :global(h2),
  .mem-body :global(h3) {
    color: #F0EDE8;
    margin-top: 1.4em;
    margin-bottom: 0.5em;
  }

  .mem-body :global(ul),
  .mem-body :global(ol) {
    padding-left: 1.5em;
  }

  .mem-body :global(li) {
    margin-bottom: 4px;
  }

  .mem-body :global(strong) {
    color: #F0EDE8;
  }

  .mem-body :global(code) {
    background: rgba(255, 255, 255, 0.06);
    border-radius: 3px;
    padding: 1px 4px;
    font-size: 13px;
  }

  .mem-empty {
    color: #666;
    font-style: italic;
    text-align: center;
    padding: 40px 20px;
  }
</style>
