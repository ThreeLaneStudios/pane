<script lang="ts">
  import { untrack } from 'svelte';
  import { projects, activeProject, activeFolder, activeView, activeTag, sidebarCollapsed, activeNotebook, activeProjectTab, activeNote, activeTaskId } from '$lib/stores/app';
  import { get } from 'svelte/store';
  import { canvasState } from '$lib/stores/canvas';
  // probe: instrument all $activeNote write sites — investigating #2214 delete-close latch
  const DEBUG = typeof localStorage !== 'undefined' && localStorage.getItem('pane-debug') === '1';
  function logAnSet(caller: string, newVal: unknown): void {
    if (!DEBUG) return;
    const oldId = get(activeNote)?.meta?.id ?? 'null';
    const newId = (newVal as any)?.meta?.id ?? 'null';
    console.log(`[PANE-DEBUG] $activeNote SET in ${caller} | old="${oldId}" new="${newId}"`);
    console.log('[PANE-DEBUG] stack:', new Error().stack?.split('\n').slice(1, 5).join('\n'));
  }
  import * as api from '$lib/api';
  import { formatFolderDeleteMessage } from '$lib/folderDeletePreviewMessage';
  import type { Project, Folder, TagInfo, CategoryWithColor } from '$lib/types';
  import ProjectContextMenu from './ProjectContextMenu.svelte';
  import FolderContextMenu from './FolderContextMenu.svelte';
  import ConfirmDialog from './ConfirmDialog.svelte';
  import NavPlayer from './NavPlayer.svelte';
  import { playerTrack } from '$lib/stores/player';
  import { invoke } from '@tauri-apps/api/core';
  import { listen } from '@tauri-apps/api/event';
  import { onMount } from 'svelte';
  import { navigateTo } from '$lib/services/navigation';
  import NotificationPanel from './NotificationPanel.svelte';
  import {
    ENTITY_FOLDER,
    ENTITY_PROJECT,
    ENTITY_CATEGORY,
    REASON_DELETED,
    subscribeDataChanged,
    type DataChangedPayload,
  } from '$lib/refreshEvent';

  let { sidebarWidth = 220 }: { sidebarWidth?: number } = $props();

  let unreadCount = $state(0);
  let tags: TagInfo[] = $state([]);
  let categories: CategoryWithColor[] = $state([]);
  
  // Context menu state
  let showContextMenu = $state(false);
  let contextMenuProject: Project | null = $state(null);
  let contextMenuPosition = $state({ x: 0, y: 0 });
  
  // Expansion state for collapsible folders
  let expandedProjectIds = $state(new Set<string>());
  
  // Expansion state (context menu only - hover handled by CSS)
  let contextMenuOpen = $state(false);
  let panelOpen = $state(false);
  let sidebarExpanded = $derived(contextMenuOpen || panelOpen);
  
  let deleteConfirmProject: Project | null = $state(null);

  // Folder context menu state
  let showFolderContextMenu = $state(false);
  let folderContextMenuFolder: Project | null = $state(null);
  let folderContextMenuProject: Project | null = $state(null);
  let folderContextMenuPosition = $state({ x: 0, y: 0 });

  // Folder actions state
  let deleteConfirmFolder: Project | null = $state(null);
  // #2874 item 65 ruling 4: the confirmation message (counts + the default
  // folder's real, current name), fetched fresh each time the dialog opens
  // — never cached, never hardcoded. Null while loading/on error, in which
  // case a safe generic fallback message is shown instead.
  let deleteConfirmFolderMessage: string | null = $state(null);
  // #2874 item 65 ruling 2: a failed folder delete must surface its error
  // to the user, not just `console.error`. Kept separate from the preview
  // message above — set only on a failed confirm, cleared on open/cancel/
  // success, and the dialog stays OPEN (not silently closed) while it's set.
  let deleteConfirmFolderError: string | null = $state(null);
  let renamingFolder: Project | null = $state(null);
  let renameInputValue = $state('');
  let renameError = $state('');
  let renameInputRef: HTMLInputElement | null = $state(null);

  // New Folder (from project context menu) state
  let newFolderProjectId: string | null = $state(null);
  let newFolderInputValue = $state('');
  let newFolderError = $state('');
  let newFolderInputRef: HTMLInputElement | null = $state(null);
  let showCategoryModal = $state(false);
  let categoryModalInput = $state('');
  let categoryModalProjectId = $state('');
  let categoryModalInputRef: HTMLInputElement | null = $state(null);

  /** Re-query unread count after panel interactions (dismiss, mark read, mark all read) */
  async function refreshBadge() {
    const count = await invoke<number>('get_unread_count');
    unreadCount = count;
  }

  onMount(() => {
    (async () => {
      await loadData();
    })();

    // Load initial unread notification count
    invoke("get_unread_count").then((count) => { unreadCount = count as number; }).catch(() => {});

    // Listen for new notifications — always re-query DB (fixes #1811 blind-increment overcount)
    const unlistenNotif = listen("pane-notification", () => { refreshBadge(); });

    // Listen for backend-initiated badge syncs (auto-dismiss, etc.) that emit no pane-notification
    const unlistenBadge = listen<{ count: number }>("pane-badge-changed", (event) => {
      unreadCount = event.payload.count;
    });
    
    // Unrelated to #2972 — a distinct, non-migrated event name
    // (`pane-projects-changed`, plural "projects"), not one of the six
    // event names §34.3 folds into the single `pane-data-changed`. Left
    // untouched.
    const handleProjectsChanged = () => loadData();
    window.addEventListener('pane-projects-changed', handleProjectsChanged);

    // #2972 stage 2 (§34.5): folds the old `pane-data-changed` DOM listener,
    // `pane-note-created` DOM listener, and the now-dead
    // `listen('pane-note-changed', ...)`/`listen('pane-project-changed',
    // ...)` Tauri listeners (dead since #2972 stage 1 migrated every
    // backend emitter of those two names onto the single `pane-data-changed`
    // event, §34.1.1) into one handler on the new event — inserting a new
    // folder into the tree in place rather than calling loadData() wholesale.
    // #2972 stage 3 (item 4): now via the one shared helper hearing BOTH
    // channels — backend `listen()` AND the app `CustomEvent` — instead of
    // Stage 2's backend-only `listen()` registration.
    const unlistenDataChanged = subscribeDataChanged((payload) => {
      handleDataChangedEvent(payload);
    });

    return () => { 
      window.removeEventListener('pane-projects-changed', handleProjectsChanged);
      unlistenDataChanged.then(fn => fn());
      unlistenNotif.then(fn => fn());
      unlistenBadge.then(fn => fn());
    };
  });

  async function loadData() {
    try {
      $projects = await api.syncProjectsWithNotebooks();
      await loadTags();
      await loadCategories();
      // is_favorite is now a DB column on each project — no localStorage needed
    } catch (e) {
      console.error('Failed to load sidebar data:', e);
    }
  }

  // #2972 stage 2 (§34.5 WARN closure, PR #3016 / Reed's 2026-10-11 audit —
  // Matt's sign-off: "fix before merge"): the sidebar's single
  // pane-data-changed handler. Only folder/project entity changes are
  // relevant to this tree — note/sheet content changes have no sidebar
  // surface to patch. Patches, inserts or removes just the ONE changed
  // project/folder in `$projects` via the new single-entity read
  // (`api.getProjectOrFolder`) — no more `syncProjectsWithNotebooks()` /
  // `get_projects()` round-trip for every event. The tree's existing keyed
  // `{#each $projects as ... (project.id)}`/`{#each folders as folder
  // (folder.id)}` blocks pick up the patched array in place.
  async function handleDataChangedEvent(payload: DataChangedPayload) {
    const { entity_id, entity_type, reason } = payload;

    // #3018 (§34.1/§34.2/§34.5): category is a fifth entity_type. Unlike
    // folder/project, a category event re-reads the WHOLE category list —
    // deliberately, unlike the single-entity reads above/below — because
    // the list is tiny and has no per-item read (there's no
    // `getCategory(name)` endpoint, only `listCategories()`). Covers add,
    // rename, recolor, and delete alike; the per-affected-project
    // `project`/`edited` events (dispatched alongside this one by
    // `renameCategory`/`deleteCategory`, api.ts) handle the regrouping —
    // this handler's job is only to keep `categories` (names + colors)
    // current.
    if (entity_type === ENTITY_CATEGORY) {
      await loadCategories();
      return;
    }

    if (entity_type !== ENTITY_FOLDER && entity_type !== ENTITY_PROJECT) return;

    if (reason === REASON_DELETED) {
      // A delete removes it locally with no read at all — the row is gone,
      // there is nothing left to fetch.
      $projects = $projects.filter(p => p.id !== entity_id);
      return;
    }

    try {
      const entity = await api.getProjectOrFolder(entity_id);
      const idx = $projects.findIndex(p => p.id === entity_id);
      if (idx === -1) {
        $projects = [...$projects, entity];
      } else {
        $projects = $projects.map(p => (p.id === entity_id ? { ...p, ...entity } : p));
      }
    } catch (e) {
      console.error('Failed to patch sidebar project/folder for pane-data-changed event:', entity_id, e);
    }
  }

  async function loadTags() {
    try {
      tags = await api.listTags();
    } catch (e) {
      tags = [];
    }
  }

  async function loadCategories() {
    try {
      categories = await api.listCategories();
    } catch (e) {
      categories = [];
    }
  }

  // Re-load categories whenever projects change
  $effect(() => {
    void $projects;
    untrack(() => { loadCategories(); });
  });

  function selectProject(project: Project) {
    $activeTaskId = null;
    logAnSet('Sidebar.selectProject', null);
    $activeNote = null;
    if (!project.parent_id) {
      // Root project → kanban view
      navigateTo({
        view: 'kanban',
        projectId: project.id
      });
    } else {
      // Folder → folder view
      navigateTo({
        view: 'notes_browser',
        projectId: project.parent_id,
        folderId: project.id
      });
    }
  }

  function clearProjectContext() {
    $activeProject = null;
    $activeFolder = null;
    logAnSet('Sidebar.clearProjectContext', null);
    $activeNote = null;
    $activeTaskId = null;
  }

  function openHome() {
    clearProjectContext();
    navigateTo({ view: 'home' });
  }

  function openArchived() {
    clearProjectContext();
    navigateTo({ view: 'trash' });
  }

  function openAllProjects() {
    clearProjectContext();
    navigateTo({ view: 'project_list' });
  }

  function openAllTasks() {
    clearProjectContext();
    navigateTo({ view: 'all_tasks' });
  }

  function openCalendar() {
    clearProjectContext();
    navigateTo({ view: 'calendar' });
  }

  function openSettings() {
    navigateTo({ view: 'settings' });
  }

  // ── Category group collapse state (#2141) ─────────────────────────────
  // Persisted in localStorage: key = sidebar-cat-collapsed-{category}
  // '' = uncategorized group key.
  function getCatCollapseKey(name: string) {
    return `sidebar-cat-collapsed:${name}`;
  }

  function isCatCollapsed(name: string): boolean {
    try { return localStorage.getItem(getCatCollapseKey(name)) === '1'; } catch { return false; }
  }

  function toggleCatCollapsed(name: string) {
    try {
      if (isCatCollapsed(name)) {
        localStorage.removeItem(getCatCollapseKey(name));
      } else {
        localStorage.setItem(getCatCollapseKey(name), '1');
      }
      // Force reactivity: bump a counter that derived values depend on
      collapseRevision += 1;
    } catch { /* ignore */ }
  }

  let collapseRevision = $state(0);

  // ── Pinned project rows ─────────────────────────────────────────────────
  let pinnedProjects = $derived.by(() => {
    const rootProjects = $projects.filter(p => !p.parent_id && p.is_favorite && p.name !== 'Pane System');

    // Create category color map
    const categoryColors = new Map<string, string>();
    for (const cat of categories) {
      categoryColors.set(cat.category, cat.color);
    }
    
    return rootProjects.map(p => {
      const folders = $projects.filter(f => f.parent_id === p.id && !f.is_system).sort((a, b) => a.name.localeCompare(b.name));
      return {
        project: p,
        // C3: dot takes category color; null when uncategorized (no dot in flat mode)
        color: p.category ? (categoryColors.get(p.category) || '#00E4E4') : null,
        folders
      };
    }).sort((a, b) => a.project.name.localeCompare(b.project.name));
  });

  // ── Grouped view (#2141, C1) ────────────────────────────────────────────
  // Activation: at least one favorited project has a category assigned.
  // When inactive: fall back to the existing flat pinnedProjects list.

  type GroupedProject = { project: typeof pinnedProjects[0]['project']; color: string | null; folders: typeof pinnedProjects[0]['folders'] };
  type CategoryGroup = {
    name: string;           // '' = uncategorized
    color: string | null;   // null = uncategorized (grey dot)
    projects: GroupedProject[];
    isUncategorized: boolean;
  };

  let categoryGroups = $derived.by((): CategoryGroup[] => {
    void collapseRevision; // subscribe for collapse reactivity

    const categoryColors = new Map<string, string>();
    for (const cat of categories) {
      categoryColors.set(cat.category, cat.color);
    }

    // Build per-project entries
    const rootProjects = $projects.filter(p => !p.parent_id && p.is_favorite && p.name !== 'Pane System');
    const entries = rootProjects.map(p => {
      const folders = $projects.filter(f => f.parent_id === p.id && !f.is_system).sort((a, b) => a.name.localeCompare(b.name));
      const catColor = p.category ? (categoryColors.get(p.category) || '#00E4E4') : null;
      return { project: p, color: catColor, folders, catName: p.category ?? '' };
    });

    // Group by category name
    const groupMap = new Map<string, GroupedProject[]>();
    for (const e of entries) {
      const key = e.catName;
      if (!groupMap.has(key)) groupMap.set(key, []);
      groupMap.get(key)!.push({ project: e.project, color: e.color, folders: e.folders });
    }

    // Sort projects within each group alphabetically
    for (const [, projs] of groupMap) {
      projs.sort((a, b) => a.project.name.localeCompare(b.project.name));
    }

    // Collect named groups (non-empty category), sorted alpha; uncategorized last
    const namedGroups: CategoryGroup[] = [];
    const sortedKeys = Array.from(groupMap.keys()).filter(k => k !== '').sort((a, b) => a.localeCompare(b));
    for (const key of sortedKeys) {
      namedGroups.push({
        name: key,
        color: categoryColors.get(key) ?? null,
        projects: groupMap.get(key)!,
        isUncategorized: false,
      });
    }

    const uncatProjects = groupMap.get('') ?? [];
    if (uncatProjects.length > 0) {
      namedGroups.push({
        name: '',
        color: null,
        projects: uncatProjects,
        isUncategorized: true,
      });
    }

    return namedGroups;
  });

  // True when grouping should be shown (at least one favorited project has a category)
  let groupingActive = $derived(
    pinnedProjects.some(p => p.project.category != null && p.project.category !== '')
  );

  
  function toggleProjectExpanded(projectId: string, e: MouseEvent) {
    e.stopPropagation();
    if (expandedProjectIds.has(projectId)) {
      expandedProjectIds.delete(projectId);
    } else {
      expandedProjectIds.add(projectId);
    }
    expandedProjectIds = new Set(expandedProjectIds);
  }
  
  function selectFolder(projectId: string, folderId: string, e: MouseEvent) {
    e.stopPropagation();
    $activeTaskId = null;
    logAnSet('Sidebar.selectFolder', null);
    $activeNote = null;
    navigateTo({
      view: 'notes_browser',
      projectId,
      folderId
    });
  }

  function handleProjectRightClick(e: MouseEvent, project: Project) {
    e.preventDefault();
    e.stopPropagation();
    contextMenuProject = project;
    contextMenuPosition = { x: e.clientX, y: e.clientY };
    showContextMenu = true;
    contextMenuOpen = true;
  }

  function handleCloseContextMenu() {
    showContextMenu = false;
    contextMenuProject = null;
    contextMenuOpen = false;
  }

  async function handleDeleteProject() {
    if (!contextMenuProject) return;
    const projectName = contextMenuProject.name;
    if (projectName === 'Pane System') return;
    
    deleteConfirmProject = contextMenuProject;
    showContextMenu = false;
    contextMenuProject = null;
    contextMenuOpen = false;
  }

  // ─── Folder Context Menu ──────────────────────────────────────────────

  function handleFolderRightClick(e: MouseEvent, folder: Project, project: Project) {
    e.preventDefault();
    e.stopPropagation();
    folderContextMenuFolder = folder;
    folderContextMenuProject = project;
    folderContextMenuPosition = { x: e.clientX, y: e.clientY };
    showFolderContextMenu = true;
    contextMenuOpen = true;
  }

  function handleCloseFolderContextMenu() {
    showFolderContextMenu = false;
    folderContextMenuFolder = null;
    folderContextMenuProject = null;
    contextMenuOpen = false;
  }

  function handleFolderRenameRequest(folder: Project) {
    // Expand the parent project so the inline rename input is visible
    if (folder.parent_id) {
      expandedProjectIds.add(folder.parent_id);
      expandedProjectIds = new Set(expandedProjectIds);
    }
    renamingFolder = folder;
    renameInputValue = folder.name;
    renameError = '';
    setTimeout(() => renameInputRef?.focus(), 50);
  }

  async function handleFolderDeleteRequest(folder: Project) {
    deleteConfirmFolder = folder;
    // #2874 item 65 ruling 4: fetched fresh at display time — read by ID
    // (default_folder_id) on the backend, so a rename of the default folder
    // is always reflected, never a stale/hardcoded "General".
    deleteConfirmFolderMessage = null;
    deleteConfirmFolderError = null;
    try {
      const preview = await api.getFolderDeletePreview(folder.id);
      deleteConfirmFolderMessage = formatFolderDeleteMessage(preview);
    } catch (e) {
      console.error('Failed to load folder delete preview:', e);
      deleteConfirmFolderMessage = 'This will delete the folder. Its contents will move to the default folder. This cannot be undone.';
    }
  }

  async function confirmDeleteFolder() {
    if (!deleteConfirmFolder) return;
    deleteConfirmFolderError = null;
    try {
      await api.deleteFolder(deleteConfirmFolder.id);
      await loadData();
      // Only close/reset on SUCCESS — a failure leaves the dialog open with
      // the error visible (ruling 2), instead of silently closing as if
      // nothing happened.
      deleteConfirmFolder = null;
      deleteConfirmFolderMessage = null;
    } catch (e) {
      console.error('Failed to delete folder:', e);
      deleteConfirmFolderError = typeof e === 'string' ? e : 'Failed to delete folder.';
    }
  }

  async function submitFolderRename() {
    if (!renamingFolder) return;
    const newName = renameInputValue.trim();
    if (!newName) { renameError = 'Name cannot be empty.'; return; }
    try {
      await api.updateFolder(renamingFolder.id, newName);
      $projects = await api.listProjects();
      renamingFolder = null;
      renameError = '';
    } catch (e) {
      console.error('Failed to rename folder:', e);
      renameError = 'Failed to rename folder.';
    }
  }

  function handleRenameKeydown(e: KeyboardEvent) {
    if (e.key === 'Enter') submitFolderRename();
    if (e.key === 'Escape') { renamingFolder = null; renameError = ''; }
  }

  // ─── New Folder (from Project Context Menu) ───────────────────────────

  function handleNewFolderRequest(projectId: string) {
    newFolderProjectId = projectId;
    newFolderInputValue = '';
    newFolderError = '';
    setTimeout(() => newFolderInputRef?.focus(), 50);
  }

  async function submitNewFolder() {
    if (!newFolderProjectId) return;
    const name = newFolderInputValue.trim();
    if (!name) { newFolderError = 'Name cannot be empty.'; return; }
    // Duplicate name guard — frontend pre-check
    const existing = $projects.filter(p => p.parent_id === newFolderProjectId);
    if (existing.some(f => f.name.toLowerCase() === name.toLowerCase())) {
      newFolderError = `A folder named '${name}' already exists in this project.`;
      return;
    }
    try {
      await api.createFolder(newFolderProjectId, name);
      $projects = await api.listProjects();
      // Auto-expand the project folders
      expandedProjectIds.add(newFolderProjectId);
      expandedProjectIds = new Set(expandedProjectIds);
      newFolderProjectId = null;
      newFolderError = '';
    } catch (e) {
      console.error('Failed to create folder:', e);
      newFolderError = 'Failed to create folder.';
    }
  }

  function handleNewFolderKeydown(e: KeyboardEvent) {
    if (e.key === 'Enter') submitNewFolder();
    if (e.key === 'Escape') { newFolderProjectId = null; newFolderError = ''; }
  }
  
  async function confirmDeleteProject() {
    if (!deleteConfirmProject) return;
    try {
      await invoke('delete_project', { projectId: deleteConfirmProject.id });
      $projects = await api.listProjects();
      if ($activeProject?.id === deleteConfirmProject.id) {
        $activeProject = null;
      }
    } catch (e) {
      console.error('Failed to delete project:', e);
    }
    deleteConfirmProject = null;
  }

  function openCategoryModal(projectId: string) {
    categoryModalProjectId = projectId;
    categoryModalInput = '';
    showContextMenu = false;
    contextMenuProject = null;
    contextMenuOpen = false;
    setTimeout(() => {
      showCategoryModal = true;
      setTimeout(() => {
        if (categoryModalInputRef) categoryModalInputRef.focus();
      }, 50);
    }, 100);
  }

  async function createCategoryFromModal() {
    if (!categoryModalInput.trim()) return;
    try {
      await api.setProjectCategory(categoryModalProjectId, categoryModalInput.trim());
      $projects = await api.listProjects();
    } catch (e) {
      console.error('Failed to set category:', e);
    }
    showCategoryModal = false;
    categoryModalInput = '';
  }

  function handleCategoryModalKeydown(e: KeyboardEvent) {
    if (e.key === 'Enter') createCategoryFromModal();
    if (e.key === 'Escape') { showCategoryModal = false; categoryModalInput = ''; }
  }
</script>

<!--
  #3191 correction (C65): a persistent flex-row placeholder, always
  exactly the rail's resting width, regardless of whether `.sidebar`
  itself is hovered/expanded. `.sidebar` below is now `position:
  absolute` unconditionally (not just during hover) specifically so it
  never participates in `.app`'s flex sizing at all — this element is
  what takes its place in that flex row instead, so `.main-content`/
  `.chat-panel` (both untouched by this fix) keep computing their own
  width exactly as if `.sidebar` were still the in-flow item, at every
  width band. See `.sidebar-rail-spacer` below for the full rationale.
-->
<div class="sidebar-rail-spacer" aria-hidden="true"></div>
<aside 
  class="sidebar" 
  class:collapsed={$sidebarCollapsed}
  class:expanded={sidebarExpanded}
>
  <!-- Top Navigation -->
  <div class="nav-section">
    <button 
      class="nav-item" 
      class:active={$canvasState.view === 'home'} 
      onclick={openHome}
      title="Home"
    >
      <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
        <path d="M3 9l9-7 9 7v11a2 2 0 01-2 2H5a2 2 0 01-2-2z"/>
        <polyline points="9 22 9 12 15 12 15 22"/>
      </svg>
      <span class="nav-label">Home</span>
    </button>

    <button 
      class="nav-item" 
      class:active={$canvasState.view === 'all_tasks'} 
      onclick={openAllTasks}
      title="All Tasks"
    >
      <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
        <polyline points="9 11 12 14 22 4"/>
        <path d="M21 12v7a2 2 0 01-2 2H5a2 2 0 01-2-2V5a2 2 0 012-2h11"/>
      </svg>
      <span class="nav-label">All Tasks</span>
    </button>

    <button 
      class="nav-item" 
      class:active={$canvasState.view === 'calendar'} 
      onclick={openCalendar}
      title="Calendar"
    >
      <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
        <rect x="3" y="4" width="18" height="18" rx="2" ry="2"/>
        <line x1="16" y1="2" x2="16" y2="6"/>
        <line x1="8" y1="2" x2="8" y2="6"/>
        <line x1="3" y1="10" x2="21" y2="10"/>
      </svg>
      <span class="nav-label">Calendar</span>
    </button>

    <button 
      class="nav-item" 
      class:active={$canvasState.view === 'trash'} 
      onclick={openArchived}
      title="Trash"
    >
      <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
        <polyline points="3 6 5 6 21 6"/>
        <path d="M19 6v14a2 2 0 01-2 2H7a2 2 0 01-2-2V6m3 0V4a2 2 0 012-2h4a2 2 0 012 2v2"/>
      </svg>
      <span class="nav-label">Trash</span>
    </button>

    <button 
      class="nav-item" 
      class:active={$canvasState.view === 'project_list'} 
      onclick={openAllProjects}
      title="All Projects"
    >
      <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
        <rect x="3" y="3" width="7" height="7"/>
        <rect x="14" y="3" width="7" height="7"/>
        <rect x="14" y="14" width="7" height="7"/>
        <rect x="3" y="14" width="7" height="7"/>
      </svg>
      <span class="nav-label">All Projects</span>
    </button>
  </div>

  <!-- C2 (#2141): Collapsed rail — one dot per category group, carets for expanded groups.
       #3190 (C65 final): dropped the `$sidebarCollapsed` clause from this
       guard. CSS cannot mount/unmount a template block, so hiding `.ps-rail`
       on hover/expanded (below, alongside revealing `.projects-section`) has
       to happen with a CSS rule instead of here — this `{#if}` now mounts
       the rail whenever it's relevant at all (grouped + has groups), exactly
       parallel to `.projects-section` below, which has always been
       unconditionally mounted and left the hiding entirely to CSS. -->
  {#if groupingActive && categoryGroups.length > 0}
    <div class="ps-rail">
      {#each categoryGroups as group}
        {@const catCollapsed = isCatCollapsed(group.name)}
        <button
          class="ps-rail__dotbtn"
          onclick={() => { $sidebarCollapsed = false; }}
          title={group.isUncategorized ? 'Uncategorized' : group.name}
          aria-label={group.isUncategorized ? 'Uncategorized projects' : group.name}
        >
          <span
            class="ps-rail__dot"
            style:background={group.color ?? 'var(--fg-4, #555)'}
          ></span>
        </button>
        {#if !catCollapsed}
          {#each group.projects as { project, color } (project.id)}
            <button
              class="ps-rail__dotbtn"
              onclick={() => { $sidebarCollapsed = false; }}
              title={project.name}
              aria-label={project.name}
            >
              <svg viewBox="0 0 8 8" width="8" height="8" fill="currentColor"
                shape-rendering="crispEdges"
                style:color={color ?? 'var(--fg-4, #555)'}
                aria-hidden="true"
              ><path d="M2 1L6 4L2 7Z"/></svg>
            </button>
          {/each}
        {/if}
      {/each}
    </div>
  {/if}

  <!-- Favorited Projects -->
  <!-- C1 (#2141): When groupingActive, render grouped headers + indented project rows.
       When !groupingActive (no category assigned to any favorite), render flat list unchanged. -->
  <div class="projects-section">

    {#if groupingActive}

      <!-- Grouped view -->
      {#each categoryGroups as group}
        {@const collapsed = isCatCollapsed(group.name)}
        <!-- Category header -->
        <button
          class="ps-cat"
          class:ps-cat--uncat={group.isUncategorized}
          onclick={() => toggleCatCollapsed(group.name)}
          aria-expanded={!collapsed}
          aria-label={group.isUncategorized ? 'Uncategorized' : group.name}
        >
          <span
            class="ps-cat__dot"
            style:background={group.color ?? 'var(--fg-4, #555)'}
          ></span>
          <span class="ps-cat__label">
            {group.isUncategorized ? 'Uncategorized' : group.name}
          </span>
        </button>

        {#if !collapsed}
          <!-- Projects within this group, indented -->
          <div class="ps-cat__projects">
            {#each group.projects as { project, color, folders } (project.id)}
              <div class="project-container">
                <div
                  class="project-item"
                  class:active={$canvasState.context.projectId === project.id && !$canvasState.context.folderId}
                  oncontextmenu={(e) => handleProjectRightClick(e, project)}
                >
                  {#if folders.length > 0}
                    <button
                      class="project-chevron-button"
                      onclick={(e) => toggleProjectExpanded(project.id, e)}
                      aria-label="Toggle folders"
                      style:color={color ?? 'var(--fg-4, #555)'}
                    >
                      <svg
                        class="chevron-icon"
                        class:expanded={expandedProjectIds.has(project.id)}
                        width="14" height="14" viewBox="0 0 24 24"
                        fill="none" stroke="currentColor" stroke-width="2"
                        stroke-linecap="round" stroke-linejoin="round"
                      >
                        <polyline points="9 18 15 12 9 6"/>
                      </svg>
                    </button>
                  {:else}
                    <span class="project-chevron-spacer"></span>
                  {/if}
                  <button class="project-name-link" onclick={() => selectProject(project)}>
                    <span class="project-label">{project.name}</span>
                  </button>
                </div>
                {#if folders.length > 0 || newFolderProjectId === project.id}
                  <div
                    class="folders-container"
                    class:expanded={expandedProjectIds.has(project.id) || newFolderProjectId === project.id}
                  >
                    {#each folders as folder (folder.id)}
                      {#if renamingFolder?.id === folder.id}
                        <div class="folder-item folder-rename-row">
                          <svg class="folder-icon" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                            <path d="M22 19a2 2 0 01-2 2H4a2 2 0 01-2-2V5a2 2 0 012-2h5l2 3h9a2 2 0 012 2z"/>
                          </svg>
                          <input bind:this={renameInputRef} bind:value={renameInputValue}
                            class="folder-rename-input" onkeydown={handleRenameKeydown}
                            onblur={submitFolderRename} placeholder="Folder name" />
                        </div>
                        {#if renameError}<div class="folder-error">{renameError}</div>{/if}
                      {:else}
                        <button
                          class="folder-item"
                          class:active={$canvasState.context.folderId === folder.id}
                          onclick={(e) => selectFolder(project.id, folder.id, e)}
                          oncontextmenu={(e) => handleFolderRightClick(e, folder, project)}
                        >
                          <svg class="folder-icon" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                            <path d="M22 19a2 2 0 01-2 2H4a2 2 0 01-2-2V5a2 2 0 012-2h5l2 3h9a2 2 0 012 2z"/>
                          </svg>
                          <span class="folder-label">{folder.name}</span>
                        </button>
                      {/if}
                    {/each}
                    {#if newFolderProjectId === project.id}
                      <div class="folder-item folder-rename-row">
                        <svg class="folder-icon" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                          <path d="M22 19a2 2 0 01-2 2H4a2 2 0 01-2-2V5a2 2 0 012-2h5l2 3h9a2 2 0 012 2z"/>
                        </svg>
                        <input bind:this={newFolderInputRef} bind:value={newFolderInputValue}
                          class="folder-rename-input" onkeydown={handleNewFolderKeydown}
                          onblur={() => { if (!newFolderInputValue.trim()) { newFolderProjectId = null; newFolderError = ''; } }}
                          placeholder="New folder name" />
                      </div>
                      {#if newFolderError}<div class="folder-error">{newFolderError}</div>{/if}
                    {/if}
                  </div>
                {/if}
              </div>
            {/each}
          </div>
        {/if}
      {/each}

    {:else}

      <!-- Flat list (no categories assigned to any favorite) -->
      {#each pinnedProjects as { project, color, folders } (project.id)}
        <div class="project-container">
          <div
            class="project-item"
            class:active={$canvasState.context.projectId === project.id && !$canvasState.context.folderId}
            oncontextmenu={(e) => handleProjectRightClick(e, project)}
          >
            {#if folders.length > 0}
              <button
                class="project-chevron-button"
                onclick={(e) => toggleProjectExpanded(project.id, e)}
                aria-label="Toggle folders"
              >
                <svg
                  class="chevron-icon"
                  class:expanded={expandedProjectIds.has(project.id)}
                  width="14" height="14" viewBox="0 0 24 24"
                  fill="none" stroke="currentColor" stroke-width="2"
                  stroke-linecap="round" stroke-linejoin="round"
                >
                  <polyline points="9 18 15 12 9 6"/>
                </svg>
              </button>
            {:else}
              <span class="project-chevron-spacer"></span>
            {/if}
            <button class="project-name-link" onclick={() => selectProject(project)}>
              {#if color}
                <span class="category-dot" style:background={color}></span>
              {/if}
              <span class="project-label">{project.name}</span>
            </button>
          </div>
          {#if folders.length > 0 || newFolderProjectId === project.id}
            <div
              class="folders-container"
              class:expanded={expandedProjectIds.has(project.id) || newFolderProjectId === project.id}
            >
              {#each folders as folder (folder.id)}
                {#if renamingFolder?.id === folder.id}
                  <div class="folder-item folder-rename-row">
                    <svg class="folder-icon" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                      <path d="M22 19a2 2 0 01-2 2H4a2 2 0 01-2-2V5a2 2 0 012-2h5l2 3h9a2 2 0 012 2z"/>
                    </svg>
                    <input bind:this={renameInputRef} bind:value={renameInputValue}
                      class="folder-rename-input" onkeydown={handleRenameKeydown}
                      onblur={submitFolderRename} placeholder="Folder name" />
                  </div>
                  {#if renameError}<div class="folder-error">{renameError}</div>{/if}
                {:else}
                  <button
                    class="folder-item"
                    class:active={$canvasState.context.folderId === folder.id}
                    onclick={(e) => selectFolder(project.id, folder.id, e)}
                    oncontextmenu={(e) => handleFolderRightClick(e, folder, project)}
                  >
                    <svg class="folder-icon" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                      <path d="M22 19a2 2 0 01-2 2H4a2 2 0 01-2-2V5a2 2 0 012-2h5l2 3h9a2 2 0 012 2z"/>
                    </svg>
                    <span class="folder-label">{folder.name}</span>
                  </button>
                {/if}
              {/each}
              {#if newFolderProjectId === project.id}
                <div class="folder-item folder-rename-row">
                  <svg class="folder-icon" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                    <path d="M22 19a2 2 0 01-2 2H4a2 2 0 01-2-2V5a2 2 0 012-2h5l2 3h9a2 2 0 012 2z"/>
                  </svg>
                  <input bind:this={newFolderInputRef} bind:value={newFolderInputValue}
                    class="folder-rename-input" onkeydown={handleNewFolderKeydown}
                    onblur={() => { if (!newFolderInputValue.trim()) { newFolderProjectId = null; newFolderError = ''; } }}
                    placeholder="New folder name" />
                </div>
                {#if newFolderError}<div class="folder-error">{newFolderError}</div>{/if}
              {/if}
            </div>
          {/if}
        </div>
      {/each}

    {/if}
  </div>

  <div class="player-dock">
    <!-- Global Audio Player (PR5a, #401 / #1801) -->
    <!-- Player + divider shown only when a track is loaded (empty state = hidden) -->
    {#if $playerTrack !== null}
      <div class="np-divider"></div>
      <NavPlayer />
    {/if}

    <!-- Bottom Section -->
    <div class="bottom-section">
    <button 
      class="nav-item" 
      onclick={() => { panelOpen = !panelOpen; }}
      title="Notifications"
    >
      <div style="position: relative;">
        <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
          <path d="M18 8A6 6 0 006 8c0 7-3 9-3 9h18s-3-2-3-9"/>
          <path d="M13.73 21a2 2 0 01-3.46 0"/>
        </svg>
        {#if unreadCount > 0}
          <span class="notification-badge">{unreadCount}</span>
        {/if}
      </div>
      <span class="nav-label">Notifications</span>
    </button>

    {#if panelOpen}
      <div
        class="panel-overlay"
        role="button"
        tabindex="-1"
        onclick={() => { panelOpen = false; }}
        onkeydown={(e) => { if (e.key === 'Escape') panelOpen = false; }}
      ></div>
      <NotificationPanel
        bind:panelOpen
        onRefreshBadge={refreshBadge}
      />
    {/if}

    <button 
      class="nav-item" 
      class:active={$canvasState.view === 'settings'}
      onclick={openSettings}
      title="Settings"
    >
      <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
        <circle cx="12" cy="12" r="3"/>
        <path d="M19.4 15a1.65 1.65 0 00.33 1.82l.06.06a2 2 0 010 2.83 2 2 0 01-2.83 0l-.06-.06a1.65 1.65 0 00-1.82-.33 1.65 1.65 0 00-1 1.51V21a2 2 0 01-2 2 2 2 0 01-2-2v-.09A1.65 1.65 0 009 19.4a1.65 1.65 0 00-1.82.33l-.06.06a2 2 0 01-2.83 0 2 2 0 010-2.83l.06-.06A1.65 1.65 0 004.68 15a1.65 1.65 0 00-1.51-1H3a2 2 0 01-2-2 2 2 0 012-2h.09A1.65 1.65 0 004.6 9a1.65 1.65 0 00-.33-1.82l-.06-.06a2 2 0 010-2.83 2 2 0 012.83 0l.06.06A1.65 1.65 0 009 4.68a1.65 1.65 0 001-1.51V3a2 2 0 012-2 2 2 0 012 2v.09a1.65 1.65 0 001 1.51 1.65 1.65 0 001.82-.33l.06-.06a2 2 0 012.83 0 2 2 0 010 2.83l-.06.06A1.65 1.65 0 0019.4 9a1.65 1.65 0 001.51 1H21a2 2 0 012 2 2 2 0 01-2 2h-.09a1.65 1.65 0 00-1.51 1z"/>
      </svg>
      <span class="nav-label">Settings</span>
    </button>
  </div>
  </div>
</aside>

{#if showCategoryModal}
  <div class="category-modal-overlay" onclick={() => { showCategoryModal = false; }} role="dialog" tabindex="-1" onkeydown={(e) => { if (e.key === 'Escape') showCategoryModal = false; }}>
    <div class="category-modal" onclick={(e) => e.stopPropagation()}>
      <h4 style="margin:0 0 12px;font-size:11px;font-weight:300;letter-spacing:2px;color:rgba(0,229,204,0.7)">SET CATEGORY</h4>
      
      {#if categories.length > 0}
        <div style="display:flex;flex-direction:column;gap:6px;margin-bottom:12px">
          {#each categories as cat}
            <button class="category-option" onclick={async () => { 
              await api.setProjectCategory(categoryModalProjectId, cat.category);
              $projects = await api.listProjects();
              showCategoryModal = false;
            }}>
              <span class="cat-dot" style:background={cat.color || '#00E4E4'}></span>
              {cat.category}
            </button>
          {/each}
        </div>
        
        <button class="category-option remove" onclick={async () => {
          await api.setProjectCategory(categoryModalProjectId, null);
          $projects = await api.listProjects();
          showCategoryModal = false;
        }}>
          No Category
        </button>
        
        <div style="height:1px;background:rgba(255,255,255,0.1);margin:16px 0"></div>
      {/if}
      
      <div style="font-size:11px;font-weight:300;letter-spacing:1px;color:rgba(0,229,204,0.6);margin-bottom:8px">OR CREATE NEW:</div>
      <input
        bind:this={categoryModalInputRef}
        bind:value={categoryModalInput}
        onkeydown={handleCategoryModalKeydown}
        placeholder="e.g. Revenue, Voice, Music..."
        style="width:100%;padding:10px 14px;border:1px solid rgba(0,229,204,0.3);border-radius:6px;background:rgba(0,229,204,0.05);color:#fff;font-size:15px;font-family:inherit;outline:none;box-sizing:border-box"
      />
      <div style="display:flex;justify-content:flex-end;gap:8px;margin-top:12px">
        <button onclick={() => { showCategoryModal = false; }} style="padding:8px 16px;border-radius:4px;font-size:11px;font-weight:300;letter-spacing:1px;cursor:pointer;font-family:inherit;background:transparent;border:1px solid rgba(224,224,224,0.2);color:#E0E0E0">CANCEL</button>
        <button onclick={createCategoryFromModal} style="padding:8px 16px;border-radius:4px;font-size:11px;font-weight:300;letter-spacing:1px;cursor:pointer;font-family:inherit;background:transparent;border:1px solid rgba(0,229,204,0.4);color:#00E4E4">CREATE</button>
      </div>
    </div>
  </div>
{/if}

{#if deleteConfirmProject}
  <ConfirmDialog
    title={`Delete "${deleteConfirmProject.name}"?`}
    message="This will delete the project and all its notes. This cannot be undone."
    onConfirm={confirmDeleteProject}
    onCancel={() => { deleteConfirmProject = null; }}
  />
{/if}

{#if deleteConfirmFolder}
  <ConfirmDialog
    title={`Delete "${deleteConfirmFolder.name}"?`}
    message={deleteConfirmFolderMessage ?? 'Loading…'}
    error={deleteConfirmFolderError}
    onConfirm={confirmDeleteFolder}
    onCancel={() => { deleteConfirmFolder = null; deleteConfirmFolderMessage = null; deleteConfirmFolderError = null; }}
  />
{/if}

{#if showContextMenu && contextMenuProject}
  <div class="context-overlay" role="button" tabindex="-1" onclick={handleCloseContextMenu} onkeydown={(e) => { if (e.key === 'Escape') handleCloseContextMenu(); }}></div>
  <ProjectContextMenu
    project={contextMenuProject}
    position={contextMenuPosition}
    onClose={handleCloseContextMenu}
    onNewCategory={openCategoryModal}
    onNewFolder={handleNewFolderRequest}
    onDelete={handleDeleteProject}
  />
{/if}

{#if showFolderContextMenu && folderContextMenuFolder}
  <div class="context-overlay" role="button" tabindex="-1" onclick={handleCloseFolderContextMenu} onkeydown={(e) => { if (e.key === 'Escape') handleCloseFolderContextMenu(); }}></div>
  <FolderContextMenu
    folder={folderContextMenuFolder}
    position={folderContextMenuPosition}
    onClose={handleCloseFolderContextMenu}
    onRename={handleFolderRenameRequest}
    onDelete={handleFolderDeleteRequest}
    isDefaultFolder={folderContextMenuProject?.default_folder_id === folderContextMenuFolder.id}
  />
{/if}

<style>
  /* #3191 correction (C65): fixed-width flex item standing in for
     `.sidebar` in `.app`'s flex row. `.sidebar` itself is now
     `position: absolute` unconditionally (see below) so it is no
     longer a flex item at all — removing it from flow would otherwise
     hand its reserved 40px straight to `.chat-panel`'s `flex: 1`
     (or `.main-content`'s, on non-Home routes) the instant hover
     started, which is itself a one-time reflow of exactly the kind
     this correction exists to eliminate. This element is a dumb
     40px-wide box with no visual content — its only job is occupying
     that space in the flex track permanently, so removing `.sidebar`
     from flow is layout-neutral at every width band, not just a
     coincidence of one. Nothing on `.main-content`/`.chat-panel`
     changes to make this true. */
  .sidebar-rail-spacer {
    width: var(--sidebar-rail-width, 40px);
    height: 100vh;
    flex-shrink: 0;
  }

  .sidebar {
    /* #3188: was a literal `40px` duplicated here and in +page.svelte's
       `.chat-panel.home-view { left: 40px }` — both bound to the same
       rail width without anything tying them together. `--sidebar-rail-
       width` is declared once on `.app` (+page.svelte), inherited down
       through the DOM to this element (CSS custom properties inherit
       regardless of Svelte's per-component style scoping) — no behavior
       change, no positioning math, same resolved value. */
    width: var(--sidebar-rail-width, 40px);
    height: 100vh;
    background: #0a0a0f;
    display: flex;
    flex-direction: column;
    transition: width 0.15s ease;
    /* #3191 correction (C65): was `position: relative` — a flex item,
       reflowing `.chat-panel`/`.main-content` at every width band
       whenever this box grew on hover (confirmed: `width:
       var(--sidebar-rail-width, 40px)` here was never conditioned on
       `.collapsed` or width band, so this box was always a plain flex
       item everywhere, not just at narrow/medium). Narrow *looked*
       overlay-correct only because #3187's unrelated `@media
       (max-width: 899px) { .chat-panel { position: fixed } }` rule
       happens to take `.chat-panel` out of flow below 900px too — a
       side effect, not a mechanism this component owns or can rely on
       at medium/wide, where `.chat-panel` stays a normal flex sibling.
       `position: absolute` here, unconditionally (not just during
       hover), makes this element genuinely positioned at all three
       bands, so growing to 220px on hover is a real stacking
       comparison everywhere, not inert at two of three bands the way
       a hover-only position toggle reading from a removed flex item
       would be mid-transition. `.sidebar-rail-spacer` above is what
       keeps this layout-neutral: it, not this element, is what
       `.chat-panel`/`.main-content` size against now, so taking
       `.sidebar` fully out of flow changes nothing about where either
       sibling starts, at rest, at any width — including wide, where
       the rest-state baseline is the regression tripwire. */
    position: absolute;
    top: 0;
    left: 0;
    border-right: 1px solid rgba(255,255,255,0.06);
    overflow: hidden;
  }

  /* Expand on hover (CSS-based, not affected by stacking contexts) */
  /* #3191 correction (C65): z-index moved here from the old
     `.sidebar.collapsed:hover`/`.sidebar.collapsed.expanded`-scoped
     rule below (now removed) — that gate meant this never applied at
     wide, where `.collapsed` is never set by `responsiveLayout.ts`, so
     the stacking fix was inert for exactly the band Matt's ruling says
     it must also cover. `.sidebar` is positioned unconditionally now
     (see above), so the same z-index applies on hover/expanded at
     every band without needing `.collapsed` as a gate — "overlaid, not
     pushed, at every width," not "at one of three." */
  .sidebar:hover {
    width: 220px;
    z-index: 1500;
  }

  /* Expand when context menu is open (JavaScript-based) */
  .sidebar.expanded {
    width: 220px;
    z-index: 1500;
  }

  /* #3186 C65 — removed: `.sidebar.collapsed:hover,
     .sidebar.collapsed.expanded { width: 40px; }` unconditionally
     defeated hover-expand any time `collapsed` was set, which is both
     of the sidebar's two collapsed states (narrow, medium) — exactly
     when hover-expand matters most. It dated to 8598e470, when
     `collapsed` meant only "the user hit Cmd+B" (a real, deliberate
     case worth guarding against a stray mouse pass at the time). The
     very next commit (aa7df7c9) reused the same `$sidebarCollapsed`
     flag for breakpoint-driven auto-collapse without updating this
     selector, and #2916 later removed Cmd+B — the only path that
     could ever set it manually — leaving this rule permanently
     guarding a state nothing can reach (confirmed: zero
     `sidebarCollapsed = true` writers outside
     `responsiveLayout.ts`'s own auto-collapse branches). Matt's own
     ruling on this scope: two collapsed states only, no third
     "user-collapsed" state to restore a manual control for — so
     hover-expand (`.sidebar:hover`/`.sidebar.expanded` above) now
     applies unconditionally in both. */

  .sidebar.collapsed .projects-section {
    display: none;
  }

  /* #3190 (C65 final): the rule above hid .projects-section whenever
     .collapsed was set, which includes the entire hover-expanded window
     (hover never clears .collapsed - see the note above). That is the
     medium-width bug: the box widened to 220px but this rule still fired,
     so only .ps-rail's dots/chevrons were ever visible, never labels.
     Same dual-selector escape every other reveal rule in this file already
     uses (.nav-label, .ps-cat__label, etc.) - on hover or .expanded, show
     the real label list again, same as the >=1200 state. */
  .sidebar.collapsed:hover .projects-section,
  .sidebar.collapsed.expanded .projects-section {
    display: block;
  }

  /* #3190 (C65 final): inverse of the rule above, and the other half of
     keeping #2875/#2879's invariant intact - .ps-rail and .projects-section
     must never both be visible at once (61cb6d30 added the base
     .projects-section hide rule for exactly that reason). .ps-rail's own
     {#if} guard (see the template above) no longer depends on
     $sidebarCollapsed, so it stays mounted through hover/expanded - this
     CSS rule is what hides it there, the mirror image of revealing
     .projects-section in the same two states. */
  .sidebar.collapsed:hover .ps-rail,
  .sidebar.collapsed.expanded .ps-rail {
    display: none;
  }

  /* #3190 (C65 final) / #3191 correction: hover-expand paints OVER the
     rest of the app instead of pushing it, at every width band, not
     just narrow. The z-index value itself (1500) is unchanged from
     #3190 — it still only needs to beat .chat-panel's highest explicit
     value (1001 at medium/wide, 100 at narrow, both under .app's single
     stacking context; see +page.svelte's own C10/#3120 comment on
     .chat-panel for why .main-content is not the relevant context
     here) while staying well below the app's top-level overlay tier
     (9999/10000) - see the .sidebar:hover / .sidebar.expanded rules
     above, where this now lives unconditionally instead of gated behind
     `.collapsed` here. That gate was the bug this correction fixes:
     `.collapsed` is never true at wide (`responsiveLayout.ts` only sets
     it below 1200px), so a z-index scoped to `.sidebar.collapsed:hover`
     could never apply at wide at all - not "inert because nothing
     overlaps there," genuinely unreachable. Nothing changes on
     .main-content - only .sidebar's own comparison against its two
     siblings, and only during the hover/expanded window. */

  .nav-section {
    display: flex;
    flex-direction: column;
  }

  .nav-item {
    display: flex;
    align-items: center;
    height: 32px;
    padding: 0 12px;
    cursor: pointer;
    position: relative;
    white-space: nowrap;
    transition: background 0.15s ease;
    background: transparent;
    border: none;
    color: inherit;
    width: 100%;
    text-align: left;
  }

  .nav-item:hover {
    background: rgba(255,255,255,0.05);
  }

  .nav-item.active {
    background: rgba(0,228,228,0.1);
  }

  .nav-item.active::before {
    content: '';
    position: absolute;
    left: 0;
    top: 8px;
    bottom: 8px;
    width: 3px;
    background: #00E4E4;
    border-radius: 0 2px 2px 0;
  }

  .nav-item svg {
    min-width: 18px;
    width: 18px;
    height: 18px;
    stroke: rgba(255,255,255,0.5);
    transition: stroke 0.15s ease;
    flex-shrink: 0;
  }

  .nav-item:hover svg {
    stroke: #e0e0e0;
  }

  .nav-item.active svg {
    stroke: #00E4E4;
  }

  .nav-label {
    margin-left: 12px;
    font-size: 14px;
    color: #e0e0e0;
    opacity: 0;
    transition: opacity 0.15s ease;
    pointer-events: none;
  }

  .sidebar:hover .nav-label,
  .sidebar.expanded .nav-label {
    opacity: 1;
  }

  .nav-item.active .nav-label {
    color: #00E4E4;
  }

  /* ── C2 (#2141): Collapsed rail dots ──────────────────────────────── */
  /* #3190 (C65 final): hidden by default now that the mount `{#if}` no
     longer gates on `$sidebarCollapsed` (it mounts at every width once
     grouped). Shown only inside `.sidebar.collapsed` below, and the
     hover/expanded hide rules above (higher specificity: 4 class-level
     selectors vs. this rule's 1) win back out of that during hover-expand
     regardless of source order. */
  .ps-rail {
    display: none;
    flex-direction: column;
    align-items: center;
    padding: 4px 0;
    padding-left: 2px; /* Fix 2141-svg: shift dot center from 20.5px to 21px (content box = 38px, center = 2 + 19 = 21px) */
  }

  .sidebar.collapsed .ps-rail {
    display: flex;
  }

  .ps-rail__dotbtn {
    width: 100%;
    height: 30px;
    border-radius: 8px;
    display: flex;
    align-items: center;
    justify-content: center;
    background: transparent;
    border: none;
    cursor: pointer;
    transition: background 0.15s ease;
  }

  .ps-rail__dotbtn:hover {
    background: rgba(255,255,255,0.07);
  }

  .ps-rail__dot {
    width: 9px;
    height: 9px;
    border-radius: 50%;
    display: block;
    flex-shrink: 0;
  }

  /* Rail caret rows reuse .ps-rail__dotbtn for aligned flex centering (#2141) */
  /* .ps-rail__caretrow removed — carets now use .ps-rail__dotbtn (<button>) for flex-centering on the 21px axis */

  /* ── C1 (#2141): Category group headers ─────────────────────────── */
  .ps-cat {
    display: flex;
    align-items: center;
    gap: 9px;
    width: 100%;
    padding: 6px 12px 5px;
    margin-top: 6px;
    background: transparent;
    border: none;
    cursor: pointer;
    text-align: left;
    overflow: hidden;
    white-space: nowrap;
  }

  /* Uncategorized group gets extra top margin */
  .ps-cat--uncat {
    margin-top: 12px;
  }

  .ps-cat__dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    flex-shrink: 0;
    display: block;
    margin-left: 5px; /* Fix 1: shift center from 16px to 21px to match nav icon axis */
  }

  .ps-cat__label {
    font-size: 12px;
    font-weight: 600;
    letter-spacing: 0.06em;
    text-transform: uppercase;
    color: var(--fg-3, #666666);
    transition: color 0.15s ease;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    opacity: 0;
    transition: opacity 0.15s ease, color 0.15s ease;
  }

  .ps-cat--uncat .ps-cat__label {
    color: var(--fg-4, #555555);
  }

  /* Show label when sidebar is expanded (hover or JS-expanded) */
  .sidebar:hover .ps-cat__label,
  .sidebar.expanded .ps-cat__label {
    opacity: 1;
  }

  .ps-cat:hover .ps-cat__label {
    color: var(--fg-2, #A0A0A0);
  }

  /* Grouped project rows: no extra indent — project-item inherits normal left margin */
  .ps-cat__projects {
    margin-left: 0;
  }

  .projects-section {
    flex: 1;
    overflow-y: auto;
    overflow-x: hidden;
    margin-top: 8px;
  }

  .projects-section::-webkit-scrollbar {
    width: 4px;
  }

  .projects-section::-webkit-scrollbar-track {
    background: transparent;
  }

  .projects-section::-webkit-scrollbar-thumb {
    background: rgba(255,255,255,0.1);
    border-radius: 2px;
  }

  .project-container {
    position: relative;
  }

  .project-item {
    display: flex;
    align-items: center;
    height: 36px;
    padding: 0;
    white-space: nowrap;
    background: transparent;
    border: none;
    width: 100%;
    text-align: left;
    position: relative;
  }

  .project-item:hover {
    background: rgba(255,255,255,0.06);
  }

  .project-item.active {
    background: rgba(13,115,119,0.15);
  }

  .project-item.active::before {
    content: '';
    position: absolute;
    left: 0;
    top: 0;
    bottom: 0;
    width: 3px;
    background: #00E4E4;
    border-radius: 0 2px 2px 0;
  }
  
  .project-chevron-button {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 20px;
    height: 20px;
    margin-left: 8px;
    background: transparent;
    border: none;
    cursor: pointer;
    flex-shrink: 0;
    padding: 0;
    color: #A0A0A0;
    transition: color 0.15s ease;
  }
  
  .project-chevron-button:hover {
    color: #F0EDE8;
  }
  
  .project-chevron-spacer {
    width: 20px;
    height: 20px;
    margin-left: 8px;
    flex-shrink: 0;
  }
  
  .chevron-icon {
    transition: transform 0.3s ease;
  }
  
  .chevron-icon.expanded {
    transform: rotate(90deg);
  }
  
  .project-name-link {
    display: flex;
    align-items: center;
    flex: 1;
    height: 100%;
    padding: 0 12px 0 4px;
    background: transparent;
    border: none;
    cursor: pointer;
    color: inherit;
    text-align: left;
    min-width: 0;
  }

  .category-dot {
    min-width: 6px;
    width: 6px;
    height: 6px;
    border-radius: 50%;
    margin-right: 8px;
    opacity: 0;
    transition: opacity 0.15s ease;
    flex-shrink: 0;
  }

  .project-item:hover .category-dot,
  .sidebar:hover .category-dot,
  .sidebar.expanded .category-dot {
    opacity: 1;
  }

  /* C3 (#2141): In grouped rows, the folder chevron IS the caret — tinted with category color.
     margin-left positions chevron center on 21px nav-icon axis (11 + 10 = 21). */
  .ps-cat__projects .project-chevron-button,
  .ps-cat__projects .project-chevron-spacer {
    margin-left: 11px;
  }

  .project-label {
    font-size: 13px;
    color: #A0A0A0;
    opacity: 0;
    transition: opacity 0.15s ease, color 0.15s ease;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    min-width: 0;
  }

  .sidebar:hover .project-label,
  .sidebar.expanded .project-label {
    opacity: 1;
  }

  .project-item:hover .project-label {
    color: #F0EDE8;
  }

  .project-item.active .project-label {
    color: #F0EDE8;
  }
  
  .folders-container {
    max-height: 0;
    overflow: hidden;
    transition: max-height 0.3s ease;
    position: relative;
  }
  
  .folders-container::before {
    content: '';
    position: absolute;
    left: 17px;
    top: 0;
    bottom: 0;
    width: 1px;
    background: rgba(0,228,228,0.2);
    opacity: 0;
    transition: opacity 0.3s ease;
  }
  
  .sidebar:hover .folders-container::before,
  .sidebar.expanded .folders-container::before {
    opacity: 1;
  }
  
  .folders-container.expanded {
    max-height: 300px;
  }
  
  .folder-item {
    display: flex;
    align-items: center;
    height: 32px;
    padding-left: 48px;
    padding-right: 12px;
    background: transparent;
    border: none;
    cursor: pointer;
    width: 100%;
    text-align: left;
    transition: background 0.15s ease;
    position: relative;
  }
  
  .folder-item:hover {
    background: rgba(255,255,255,0.06);
  }
  
  .folder-item.active {
    background: rgba(13,115,119,0.15);
  }
  
  .folder-item.active::before {
    content: '';
    position: absolute;
    left: 0;
    top: 0;
    bottom: 0;
    width: 3px;
    background: #00E4E4;
    border-radius: 0 2px 2px 0;
  }
  
  .folder-icon {
    width: 16px;
    height: 16px;
    stroke: #A0A0A0;
    margin-right: 8px;
    flex-shrink: 0;
    transition: stroke 0.15s ease;
  }
  
  .folder-item:hover .folder-icon {
    stroke: #F0EDE8;
  }
  
  .folder-item.active .folder-icon {
    stroke: #00E4E4;
  }
  
  .folder-label {
    font-size: 12px;
    color: #A0A0A0;
    opacity: 0;
    transition: opacity 0.15s ease, color 0.15s ease;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  
  .sidebar:hover .folder-label,
  .sidebar.expanded .folder-label {
    opacity: 1;
  }
  
  .folder-item:hover .folder-label {
    color: #F0EDE8;
  }
  
  .folder-item.active .folder-label {
    color: #F0EDE8;
  }

  /* Folder rename / new-folder inline input */
  .folder-rename-row {
    cursor: default;
  }

  .folder-rename-row:hover {
    background: transparent;
  }

  .folder-rename-input {
    flex: 1;
    background: rgba(255, 255, 255, 0.06);
    border: 1px solid rgba(0, 228, 228, 0.3);
    border-radius: 4px;
    color: #e0e0e0;
    font-size: 12px;
    font-family: inherit;
    padding: 2px 6px;
    outline: none;
    min-width: 0;
  }

  .folder-rename-input:focus {
    border-color: rgba(0, 228, 228, 0.6);
  }

  .folder-error {
    padding: 2px 12px 4px 48px;
    font-size: 11px;
    color: #EF4444;
  }

  .np-divider {
    height: 1px;
    background: rgba(255,255,255,0.06);
    margin: 0;
  }

  .player-dock {
    margin-top: auto;
  }

  .bottom-section {
    border-top: 1px solid rgba(255,255,255,0.06);
    padding: 8px 0;
  }

  .notification-badge {
    position: absolute;
    top: -4px;
    right: -4px;
    background: var(--accent);
    color: var(--fg-on-accent);
    font-size: 10px;
    font-weight: 600;
    min-width: 16px;
    height: 16px;
    border-radius: 8px;
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 0 4px;
  }

  /* C25 (#3128): same stacking-context mismatch as NotificationPanel's
     own .nt-pop — raised above .chat-panel's 1001 (C10, #3125) so this
     overlay (and the panel it gates) paints over the chat panel again. */
  .panel-overlay {
    position: fixed;
    inset: 0;
    z-index: 1002;
    background: transparent;
  }

  .context-overlay {
    position: fixed;
    top: 0;
    left: 0;
    width: 100vw;
    height: 100vh;
    background: rgba(0, 0, 0, 0.3);
    z-index: 999;
  }

  .category-modal-overlay {
    position: fixed;
    top: 0;
    left: 0;
    width: 100vw;
    height: 100vh;
    background: rgba(0, 0, 0, 0.6);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 10000;
  }

  .category-modal {
    background: #1F1F23;
    border: 1px solid rgba(255,255,255,0.10);
    border-radius: 6px;
    padding: 24px;
    width: 320px;
    box-shadow: 0 8px 32px rgba(0, 0, 0, 0.5);
  }

  .category-option {
    display: flex;
    align-items: center;
    gap: 10px;
    width: 100%;
    padding: 10px 14px;
    border: 1px solid rgba(255, 255, 255, 0.1);
    border-radius: 6px;
    background: rgba(255, 255, 255, 0.03);
    color: #e0e0e0;
    font-size: 14px;
    font-family: inherit;
    font-weight: 400;
    cursor: pointer;
    transition: all 0.15s ease;
    text-align: left;
  }

  .category-option:hover {
    background: rgba(255, 255, 255, 0.06);
    border-color: rgba(0, 228, 228, 0.3);
  }

  .category-option.remove {
    color: #666666;
    font-style: italic;
  }

  .category-option.remove:hover {
    color: #888888;
    border-color: rgba(255, 255, 255, 0.2);
  }

  .cat-dot {
    width: 10px;
    height: 10px;
    border-radius: 50%;
    flex-shrink: 0;
  }
</style>
