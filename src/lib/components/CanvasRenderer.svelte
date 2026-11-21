<script lang="ts">
  import { onMount, untrack, tick } from 'svelte';
  import { canvasState } from '$lib/stores/canvas';
  import { navigationHistory } from '$lib/stores/history';
  import { chatSidebarOpen } from '$lib/stores/app';
  import { activeProject, activeFolder, activeNotebook, activeView, activeNote, activeProjectTab, projects, notes } from '$lib/stores/app';
  import * as api from '$lib/api';
  import * as noteService from '$lib/services/noteService';
  import { navigateTo } from '$lib/services/navigation';
  import Editor from './Editor.svelte';
  import AllTasksView from './AllTasksView.svelte';
  import CalendarView from './CalendarView.svelte';
  import Settings from './Settings.svelte';
  import KanbanBoard from './KanbanBoard.svelte';
  import ProjectsView from './ProjectsView.svelte';
  import NotesBrowser from './NotesBrowser.svelte';
  import ProjectSettings from './ProjectSettings.svelte';
  
  import Trash from './Trash.svelte';
  import HomeDashboard from './HomeDashboard.svelte';

  import type { Project, NoteMeta, NoteContent, Task } from '$lib/types';
  import { get } from 'svelte/store';

  // probe: instrument all $activeNote write sites — investigating #2214 delete-close latch
  const DEBUG = typeof localStorage !== 'undefined' && localStorage.getItem('pane-debug') === '1';
  function logAnSet(caller: string, newVal: unknown): void {
    if (!DEBUG) return;
    const oldId = get(activeNote)?.meta?.id ?? 'null';
    const newId = (newVal as any)?.meta?.id ?? 'null';
    console.log(`[PANE-DEBUG] $activeNote SET in ${caller} | old="${oldId}" new="${newId}"`);
    console.log('[PANE-DEBUG] stack:', new Error().stack?.split('\n').slice(1, 5).join('\n'));
  }

  let refreshKey = $state(0);
  let loadedNotes: NoteMeta[] = $state([]);
  let tasks: Task[] = $state([]);
  let isLoading = $state(false);

  // Reactive canvas state
  let currentCanvasState = $derived($canvasState);
  let previousState: any = null;
  
  // Track navigation history
  $effect(() => {
    const current = currentCanvasState;
    if (previousState && JSON.stringify(previousState) !== JSON.stringify(current)) {
      navigationHistory.pushState(previousState);
    }
    previousState = { ...current };
  });

  // Back navigation handler
  function goBack() {
    const previousState = navigationHistory.popState();
    if (previousState) {
      $canvasState = previousState;
    }
  }
  
  // Filtered projects when a filter is active
  let filteredProjects = $derived(
    currentCanvasState.context.projectFilter 
      ? $projects.filter(p => 
          currentCanvasState.context.projectFilter!.some(f => 
            p.name.toLowerCase().includes(f.toLowerCase())
          )
        )
      : $projects
  );

  // #2335/#2370: noteService.selectNote() calls canvasState.update() on every card click
  // (noteService.ts:~95), but by the time that update() fires, selectNote() has ALREADY set
  // $activeNote to the just-clicked note one line earlier (noteService.ts:91, before the
  // canvasState.update() at line 95). So when this $effect re-fires on that reference churn
  // (view stays 'note_editor', only context.noteId/noteTitle refreshed to stay in sync per
  // FIX #619), $activeNote already equals context.noteId for ANY canvasState.update() echo
  // coming out of selectNote() — including a genuine different-note click, not just a
  // same-note re-selection. Comparing against a CanvasRenderer-local bookkeeping var (the
  // #2380 attempt's `lastLoadedNoteId`) doesn't work: it only updates once THIS component's
  // own async loadDataForView resolves, one full cycle behind $activeNote, so it still holds
  // the *previous* note's id at the moment a different-note echo is checked — comparison
  // always fails, reload always proceeds, remount bounce persists (see Reed's audit,
  // docs/audits/2380-delta-audit.md §1b, which verified this empirically against #2380).
  //
  // Corrected guard: compare context.noteId directly against $activeNote's CURRENT id. If
  // they already match, selectNote() has already fully populated $activeNote for this note —
  // this canvasState.update() is by construction just its own trailing echo, so the redundant
  // fetch/isLoading/unmount-remount cycle is always safe to skip, regardless of whether the
  // note differs from whichever one was previously displayed.
  //
  // Reset conditions (unchanged from #2380, still correct — Reed's audit §1c confirmed all
  // five were implemented correctly, the flaw was purely in what was compared):
  //   1. Genuinely different note where $activeNote does NOT yet reflect it (e.g. a
  //      programmatic canvasState.set() from navigation.ts that isn't preceded by
  //      selectNote()'s activeNote.set()) -> guard condition fails, reloads for real.
  //   2. View change away and back to note_editor -> lastLoadedProjectId cleared below
  //      whenever state.view !== 'note_editor', so project-switch detection still resets.
  //   3. Project switch (same noteId, different project) -> guard also compares
  //      context.projectId against lastLoadedProjectId.
  //   4. First load of a note from a non-note_editor view -> lastLoadedProjectId is null (or
  //      was just cleared by #2 above), so the project-tuple half of the guard can't match.
  //   5. Failed/null load -> loadDataForView's note_editor branch clears lastLoadedProjectId
  //      on failure/empty result, so a retry (re-click) isn't skipped by a stale match.
  //
  // lastLoadedNoteId is kept alongside lastLoadedProjectId — not for the skip check itself
  // ($activeNote's own id remains the authoritative reference there, per #2335 above), but
  // for the reset-condition logic below (#2443 re-land, nav-lockup diagnosis §7 fix 3).
  let lastLoadedProjectId: string | null = $state(null);
  let lastLoadedNoteId: string | null = $state(null);

  // Load data when view changes
  $effect(() => {
    const state = currentCanvasState;
    // untrack the actual loading so store writes inside don't re-trigger this effect
    untrack(() => {
      if (
        state.view === 'note_editor' &&
        state.context?.noteId &&
        state.context.noteId === get(activeNote)?.meta?.id &&
        (state.context.projectId ?? null) === lastLoadedProjectId
      ) {
        // $activeNote already reflects this exact note (selectNote() set it before firing
        // this echo) — this re-fire is redundant by construction. Skip entirely: don't set
        // isLoading, don't re-fetch, don't unmount/remount NotesBrowser.
        return;
      }
      if (state.view !== 'note_editor') {
        // #2443 re-land (nav-lockup diagnosis §7 fix 3): a naive unconditional reset here
        // (`lastLoadedProjectId = null` on ANY non-note_editor view) went stale the instant
        // any transient view change en route to note_editor for the SAME note/project
        // occurred — e.g. the old wiki-link double-write (navigateFn → 'notes_browser',
        // then selectNote() → 'note_editor' a beat later) always hit this branch first,
        // clearing the guard right before the very note_editor transition that would
        // otherwise have matched it, forcing a redundant loadDataForView call on top of
        // the load selectNote() itself already did. This is a real, separately-triggerable
        // defect — ANY notes_browser→note_editor transition to the same note re-triggers a
        // redundant load, not just via wiki-links.
        //
        // Fix: only invalidate the guard when this transition genuinely leaves the
        // currently-loaded note/project behind — i.e. neither the outgoing noteId nor the
        // outgoing projectId still points at what's already loaded. A transient pass-through
        // that still targets the same note or project is not a real departure.
        const departingContext = state.context ?? {};
        const stillSameNote = lastLoadedNoteId !== null && departingContext.noteId === lastLoadedNoteId;
        const stillSameProject = lastLoadedProjectId !== null && (departingContext.projectId ?? null) === lastLoadedProjectId;
        if (!stillSameNote && !stillSameProject) {
          lastLoadedProjectId = null;
          lastLoadedNoteId = null;
        }
      }
      loadDataForView(state.view, state.context);
    });
  });

  // Watch for activeProject changes — only navigate to project_detail if user explicitly selected a project
  // (not when sidebar navigates to today/trash/calendar/etc.)
  // Track lastProjectId to avoid redundant navigations
  let lastProjectId: string | null = $state(null);
  $effect(() => {
    if ($activeProject && $activeProject.id !== lastProjectId) {
      lastProjectId = $activeProject.id;
      // Don't override if we're already in a project-related or global view
      // (sidebar selectProject already sets canvasState directly)
    } else if (!$activeProject) {
      lastProjectId = null;
    }
  });

  async function loadDataForView(view: string, context: any) {
    isLoading = true;
    try {
      switch (view) {
        case 'project_list':
          $activeProject = null;
          $activeFolder = null;
          $activeView = 'projects';
          $projects = await api.syncProjectsWithNotebooks();
          break;
        case 'notes_browser':
        case 'project_detail':
          if (context.projectId) {
            const allProjects = await api.listProjects();
            const project = allProjects.find(p => p.id === context.projectId);
            if (project) {
              $activeProject = project;
              
              // Check if we're navigating to a folder within the project
              if (context.folderId) {
                const folder = allProjects.find(p => p.id === context.folderId && p.parent_id === context.projectId);
                if (folder) {
                  $activeFolder = folder;
                  $activeNotebook = `${project.name}/${folder.name}`;
                  $activeView = 'folder';
                  $activeProjectTab = 'notes';
                } else {
                  // Folder not found, fall back to project view
                  $activeNotebook = project.name;
                  $activeView = 'project';
                }
              } else {
                // No folder, just project view
                $activeNotebook = project.name;
                $activeView = 'project';
              }
            }
            const projectTasks = await api.listTasks();
            tasks = projectTasks.filter(t => t.project_id === context.projectId);
          }
          break;
        // REMOVED: folder_detail - now handled by notes_browser
        case 'folder_detail':
          if (context.folderId && context.projectId) {
            const allProjects2 = await api.listProjects();
            const parentProject = allProjects2.find(p => p.id === context.projectId);
            const folder = allProjects2.find(p => p.id === context.folderId);
            if (parentProject && folder) {
              $activeProject = parentProject;
              $activeFolder = folder;
              $activeNotebook = `${parentProject.name}/${folder.name}`;
              $activeView = 'folder';
              $activeProjectTab = 'notes';
            }
          }
          break;
        case 'note_editor':
          if (context.noteId && context.projectId) {
            const noteSlug = context.noteId;
            const projectName = context.projectName || 'Notes';
            // #3044 item 2: every other case in this switch (notes_browser/project_detail,
            // folder_detail, task_list, project_config) resolves context.projectId
            // into $activeProject (the store NotesBrowser.svelte — the SAME component this
            // view renders, CanvasRenderer.svelte:484 — actually reads for its folder header,
            // folder list, and card filtering). This case never did. A note_editor navigation
            // whose target note lives in a DIFFERENT project from whatever $activeProject
            // already held (e.g. a wiki-link click, or any other cross-project note-editor
            // navigation) therefore loaded the right note's CONTENT via the id-keyed get_note
            // call below, while every surrounding NotesBrowser chrome kept rendering the OLD
            // project — confirmed cause of Matt's report that two wiki-link clicks (one to an
            // existing note, one to a newly created one) both "opened under Pane Development"
            // instead of the Symbiotic project both notes actually belonged to. Resolved and
            // set here, exactly like the notes_browser case does, before the content load.
            const allProjectsForNote = await api.listProjects();
            const noteProject = allProjectsForNote.find(p => p.id === context.projectId);
            if (noteProject) {
              $activeProject = noteProject;
              const noteFolder = context.folderId
                ? allProjectsForNote.find(p => p.id === context.folderId && p.parent_id === context.projectId)
                : null;
              if (noteFolder) {
                $activeFolder = noteFolder;
                $activeNotebook = `${noteProject.name}/${noteFolder.name}`;
              } else {
                $activeFolder = null;
                $activeNotebook = noteProject.name;
              }
            }
            try {
              const noteWithJson = await noteService.loadNoteWithJson(projectName, noteSlug);
              if (noteWithJson) {
                logAnSet('CanvasRenderer.handleCanvasAction[note_editor]', noteWithJson);
                $activeNote = noteWithJson;
                // Update context with note title so AI knows what's open
                if (noteWithJson.meta?.title) {
                  currentCanvasState.context.noteTitle = noteWithJson.meta.title;
                }
                // #2335: record this successful load's project so the $effect guard above can
                // still detect a project switch even though it now trusts $activeNote for the
                // noteId half of the comparison.
                lastLoadedProjectId = context.projectId ?? null;
                // #2443 re-land §7 fix 3: also record the noteId, used by the reset-condition
                // check above to distinguish a genuine departure from a transient pass-through.
                lastLoadedNoteId = context.noteId ?? null;
              } else {
                // Load returned nothing usable — don't mark this project as "loaded", so a
                // future retry (e.g. re-clicking the same card) isn't skipped by the guard.
                lastLoadedProjectId = null;
                lastLoadedNoteId = null;
              }
            } catch (error) {
              console.error('Failed to load note:', error);
              // Don't mark a failed load as "loaded" — allow retry on the next reference churn.
              lastLoadedProjectId = null;
              lastLoadedNoteId = null;
            }
          }
          break;
        // 'note_list' case REMOVED #3044 item 5 — zero real call sites set
        // this view anywhere (user or agent); see canvas.ts's CanvasViewType
        // comment for the full citation history.
        // 'task_list' case REMOVED #3044 item 5 — proven unreachable:
        // zero literal `view: 'task_list'` call sites anywhere in src/
        // (user or agent — the backend's show_view/navigate tools actively
        // REJECT it, per #3022/#3043), confirmed by the derived
        // user-reachable-views scan. TaskList.svelte (the component this
        // case rendered) is deleted in the same commit.
        case 'search_results':
          if (context.searchQuery) {
            const [searchNotes, searchTasks] = await Promise.all([
              api.searchNotes(context.searchQuery),
              api.listTasks()
            ]);
            $notes = searchNotes;
            tasks = searchTasks.filter(t => 
              t.title.toLowerCase().includes(context.searchQuery.toLowerCase()) ||
              (t.description && t.description.toLowerCase().includes(context.searchQuery.toLowerCase()))
            );
          }
          break;
        // 'dashboard' case REMOVED #3044 item 5 — proven unreachable, same
        // basis as 'task_list' above (#3043 Matt's ruling + derived scan);
        // its render branch below (inline markup, not a separate
        // component) is deleted in the same commit.
        case 'project_config':
          if (context.projectId) {
            const allProjects = await api.listProjects();
            const project = allProjects.find(p => p.id === context.projectId);
            if (project) {
              $activeProject = project;
              $activeNotebook = project.name;
              $activeView = 'project';
            }
          }
          break;
      }
    } catch (error) {
      console.error('Failed to load data for view:', view, error);
    } finally {
      isLoading = false;
    }
  }

  function sendChatMessage(message: string) {
    // Send context update to chat so AI knows about manual navigation
    try {
      // Dispatch custom event that ChatSidebar will listen to
      const contextEvent = new CustomEvent('canvas-navigation', {
        detail: {
          message,
          canvasState: $canvasState,
          timestamp: new Date().toISOString()
        }
      });
      window.dispatchEvent(contextEvent);
      
      // Open chat if closed
      if (!$chatSidebarOpen) {
        $chatSidebarOpen = true;
      }
    } catch (error) {
      console.error('Failed to send chat message:', error);
    }
  }

  function handleProjectClick(project: Project) {
    navigateTo({
      view: 'notes_browser',
      projectId: project.id,
      folderId: project.parent_id || undefined
    });
    sendChatMessage(`Show me details for project "${project.name}"`);
  }

  function handleNoteClick(note: NoteMeta) {
    navigateTo({
      view: 'note_editor',
      noteId: note.slug,
      projectId: currentCanvasState.context.projectId || undefined
    });
    sendChatMessage(`Open note "${note.title}"`);
  }

  async function handleTaskClick(task: Task) {
    // Navigate to the task's project board with its card open.
    // #2874 item F (scope doc item 6): previously landed on 'notes_browser'
    // (the full per-project note browser, "every note") — Matt's ruling is
    // the project's *board* (view: 'kanban', same view Sidebar.svelte's
    // selectProject uses for a project root) with that task's card open.
    if (task.project_id) {
      // Same await-navigateTo()-then-tick()-then-dispatch sequencing #2874
      // item 18 fix 1 established for notes_browser: let Svelte flush the
      // {#if currentCanvasState.view === 'kanban'} mount and run
      // KanbanBoard's $effects before dispatching, no fixed-delay timer
      // guess. KanbanBoard listens for this same 'pane-select-note' event
      // (already used by NotesBrowser and ChatSidebar for the identical
      // purpose) and queues it the same way NotesBrowser does if its own
      // task list hasn't finished loading yet — no new dispatch path added.
      await navigateTo({
        view: 'kanban',
        projectId: task.project_id,
        noteId: task.id
      });
      await tick();

      window.dispatchEvent(new CustomEvent('pane-select-note', { detail: { noteId: task.id } }));
    }
  }

  function getBreadcrumb(): string {
    const { view, context } = currentCanvasState;
    const parts = [];
    
    if (context.projectName && view !== 'project_list') {
      parts.push(context.projectName);
    }
    
    if (context.folderName) {
      parts.push(context.folderName);
    }
    
    switch (view) {
      case 'project_list': return 'Projects';
      case 'notes_browser': return parts.length ? parts.join(' > ') : 'Project';
      // case 'project_detail' - REMOVED v10.13.0: return parts.length ? parts.join(' > ') : 'Project';
      // case 'folder_detail' - now notes_browser: return [...parts, context.folderName || 'Folder'].join(' > ');
      case 'note_editor': return [...parts, 'Note'].join(' > ');
      case 'kanban': return [...parts, 'Kanban'].join(' > ');
      case 'all_tasks': return 'All Tasks';
      case 'calendar': return 'Calendar';
      case 'trash': return 'Trash';
      case 'search_results': return `Search: "${context.searchQuery}"`;
      case 'settings': return 'Settings';
      case 'project_config': return [...parts, 'Settings'].join(' > ');
      case 'welcome':
      default: return '';
    }
  }

  // Group projects by category for project_list view
  function getGroupedProjects(): Record<string, Project[]> {
    const grouped: Record<string, Project[]> = {};
    
    for (const project of $projects) {
      // If project has a parent, it's a subfolder
      if (project.parent_id) {
        continue; // Skip subfolders for now, show them under parent
      }
      
      const category = project.category || 'Uncategorized';
      if (!grouped[category]) {
        grouped[category] = [];
      }
      grouped[category].push(project);
      
      // Add subfolders
      const subfolders = $projects.filter(p => p.parent_id === project.id);
      grouped[category].push(...subfolders);
    }
    
    return grouped;
  }
</script>

<div class="canvas-container">
  <!-- Header bar removed per design -->
  
  <div class="canvas-content">
    {#if isLoading && currentCanvasState.view !== 'all_tasks' && currentCanvasState.view !== 'trash' && currentCanvasState.view !== 'calendar' && currentCanvasState.view !== 'notes_browser' && currentCanvasState.view !== 'note_editor'}
      <div class="loading-state">
        <div class="loading-spinner"></div>
        <span>Loading...</span>
      </div>
    {:else if currentCanvasState.view === 'project_list' && currentCanvasState.context.projectFilter}
      <!-- Filtered project list -->
      <div class="project-list-view">
        <div class="filter-label">Showing {currentCanvasState.context.projectFilter.length} projects</div>
        <div class="projects-grid">
          {#each filteredProjects as project}
            <button 
              class="project-card"
              onclick={() => handleProjectClick(project)}
            >
              <div class="project-name">{project.name}</div>
              {#if project.description}
                <div class="project-description">{project.description}</div>
              {/if}
            </button>
          {/each}
        </div>
      </div>
    
    {:else if currentCanvasState.view === 'project_list'}
      <ProjectsView />
    
    {:else if currentCanvasState.view === 'notes_browser' || currentCanvasState.view === 'note_editor'}
      <NotesBrowser />
    
    {:else if currentCanvasState.view === 'all_tasks'}
      <AllTasksView />
    
    {:else if currentCanvasState.view === 'calendar'}
      <CalendarView onTaskSelect={handleTaskClick} refreshKey={refreshKey} />
    
    {:else if currentCanvasState.view === 'trash'}
      <Trash onClose={() => {
        navigateTo({ view: 'home' });
      }} />
    
    {:else if currentCanvasState.view === 'kanban' && currentCanvasState.context.projectId}
      <KanbanBoard />
    
    {:else if currentCanvasState.view === 'search_results'}
      <div class="search-results-view">
        <div class="results-sections">
          {#if $notes.length > 0}
            <div class="section">
              <h3>Notes ({$notes.length})</h3>
              <div class="notes-list">
                {#each $notes as note}
                  <button class="note-item" onclick={() => handleNoteClick(note)}>
                    <div class="note-title">{note.title}</div>
                    <div class="note-meta">{note.notebook}</div>
                  </button>
                {/each}
              </div>
            </div>
          {/if}
          
          {#if tasks.length > 0}
            <div class="section">
              <h3>Tasks ({tasks.length})</h3>
              <div class="tasks-list">
                {#each tasks as task}
                  <button class="task-item" onclick={() => handleTaskClick(task)}>
                    <div class="task-checkbox" class:checked={task.status === 'done'}>
                      {#if task.status === 'done'}✓{/if}
                    </div>
                    <div class="task-content">
                      <div class="task-title">{task.title}</div>
                      {#if task.project}
                        <div class="task-meta">{task.project}</div>
                      {/if}
                    </div>
                  </button>
                {/each}
              </div>
            </div>
          {/if}
          
          {#if $notes.length === 0 && tasks.length === 0}
            <div class="empty-state">No results found for "{currentCanvasState.context.searchQuery}"</div>
          {/if}
        </div>
      </div>
    
    {:else if currentCanvasState.view === 'settings'}
      <Settings />
    
    {:else if currentCanvasState.view === 'project_config'}
      <ProjectSettings projectId={currentCanvasState.context.projectId} />
    
    {:else if currentCanvasState.view === 'home'}
      <HomeDashboard />

    {:else}
      <div class="error-state">
        <p>Unknown view: {currentCanvasState.view}</p>
      </div>
    {/if}
  </div>
</div>

<style>
  .canvas-container {
    flex: 1;
    background: #0a0a0f;
    color: #e0e0e0;
    font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Text', 'Segoe UI', sans-serif;
    display: flex;
    flex-direction: column;
    height: 100%;
  }

  .canvas-header {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-shrink: 0;
  }

  .header-actions {
    display: flex;
    gap: 4px;
    margin-left: auto;
  }

  .header-btn {
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 4px 10px;
    border: 1px solid rgba(255, 255, 255, 0.06);
    border-radius: 4px;
    background: transparent;
    color: #888888;
    font-size: 12px;
    font-family: inherit;
    cursor: pointer;
    transition: all 0.15s ease;
  }

  .header-btn:hover {
    background: rgba(255, 255, 255, 0.06);
    color: #e0e0e0;
  }

  .header-spacer {
    flex: 1;
  }

  .header-action-btn {
    display: flex;
    align-items: center;
    gap: 5px;
    padding: 5px 12px;
    border: 1px solid rgba(255, 255, 255, 0.08);
    border-radius: 4px;
    background: rgba(255, 255, 255, 0.04);
    color: #888888;
    font-size: 12px;
    font-family: inherit;
    cursor: pointer;
    transition: all 0.15s ease;
    flex-shrink: 0;
  }

  .header-action-btn:hover {
    background: rgba(0, 228, 228, 0.15);
    color: #00E4E4;
    border-color: rgba(0, 228, 228, 0.3);
  }

  .back-button {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 28px;
    height: 28px;
    border: none;
    background: transparent;
    color: #666666;
    cursor: pointer;
    font-size: 14px;
    font-weight: 500;
    transition: color 0.15s ease;
    margin-left: 12px;
  }

  .back-button:hover {
    color: #e0e0e0;
  }

  .breadcrumb {
    padding: 12px 24px 12px 0;
    font-size: 12px;
    color: var(--text-tertiary);
    border-bottom: 1px solid var(--border-color);
    font-weight: 500;
    background: transparent;
    flex: 1;
  }

  .canvas-content {
    flex: 1;
    overflow: auto;
    min-height: 0;
    background: #0a0a0f;
  }

  /* Loading and error states */
  .loading-state {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    height: 100%;
    gap: 16px;
    color: #666666;
    background: #0a0a0f;
  }

  .loading-spinner {
    width: 24px;
    height: 24px;
    border: 2px solid rgba(0, 228, 228, 0.2);
    border-top: 2px solid #00E4E4;
    border-radius: 50%;
    animation: spin 1s linear infinite;
  }

  @keyframes spin {
    from { transform: rotate(0deg); }
    to { transform: rotate(360deg); }
  }

  .error-state {
    display: flex;
    align-items: center;
    justify-content: center;
    height: 100%;
    color: #ef4444;
    background: #0a0a0f;
  }

  .empty-state {
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 40px;
    color: var(--text-tertiary);
    font-size: 14px;
  }

  /* Project list view */
  .filter-label {
    font-size: 12px;
    color: #888888;
    margin-bottom: 16px;
    font-weight: 500;
  }

  .project-list-view {
    padding: 24px;
  }

  .project-category {
    margin-bottom: 32px;
  }

  .category-title {
    font-size: 14px;
    font-weight: 600;
    color: var(--text-tertiary);
    text-transform: uppercase;
    letter-spacing: 0.5px;
    margin: 0 0 16px 0;
  }

  .projects-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(300px, 1fr));
    gap: 16px;
  }

  .project-card {
    padding: 20px;
    background: var(--bg-secondary);
    border: 1px solid var(--border-color);
    border-radius: 8px;
    cursor: pointer;
    transition: all 0.15s ease;
    text-align: left;
  }

  .project-card:hover {
    background: var(--bg-hover);
    border-color: var(--accent);
    box-shadow: 0 4px 12px rgba(0, 228, 228, 0.1);
  }

  .project-card.subfolder {
    margin-left: 20px;
    opacity: 0.8;
  }

  .project-name {
    font-size: 16px;
    font-weight: 500;
    color: var(--text-primary);
    margin-bottom: 8px;
  }

  .project-description {
    font-size: 14px;
    color: var(--text-secondary);
    line-height: 1.4;
  }

  /* Project detail view */
  .project-detail-view {
    padding: 24px;
  }

  .project-sections {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 32px;
  }

  .section {
    background: var(--bg-secondary);
    border-radius: 8px;
    overflow: hidden;
    border: 1px solid var(--border-color);
  }

  .section-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 16px 20px;
    border-bottom: 1px solid var(--border-color);
    background: var(--bg-tertiary);
  }

  .section-header h3 {
    font-size: 16px;
    font-weight: 600;
    color: var(--text-primary);
    margin: 0;
  }

  .count {
    font-size: 12px;
    color: var(--text-tertiary);
    background: var(--bg-hover);
    padding: 2px 8px;
    border-radius: 12px;
  }

  /* Note and task items */
  .notes-list, .tasks-list {
    max-height: 400px;
    overflow-y: auto;
  }

  .note-item, .task-item {
    display: flex;
    align-items: center;
    gap: 12px;
    width: 100%;
    padding: 12px 20px;
    border: none;
    background: transparent;
    color: var(--text-primary);
    cursor: pointer;
    border-bottom: 1px solid var(--border-color);
    text-align: left;
    transition: background-color 0.15s ease;
  }

  .note-item:hover, .task-item:hover {
    background: var(--bg-hover);
  }

  .note-title, .task-title {
    font-size: 14px;
    font-weight: 500;
    color: var(--text-primary);
  }

  .note-meta, .task-meta {
    font-size: 12px;
    color: var(--text-tertiary);
    margin-top: 4px;
  }

  .task-checkbox {
    width: 16px;
    height: 16px;
    border: 1px solid var(--text-tertiary);
    border-radius: 2px;
    display: flex;
    align-items: center;
    justify-content: center;
    font-size: 12px;
    flex-shrink: 0;
  }

  .task-checkbox.checked {
    background: var(--accent);
    border-color: var(--accent);
    color: white;
  }

  .task-content {
    flex: 1;
  }

  /* Grid views */
  .notes-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(300px, 1fr));
    gap: 16px;
    padding: 24px;
  }

  .pane-card {
    padding: 20px;
    background: rgba(255, 255, 255, 0.03);
    border: 1px solid rgba(255, 255, 255, 0.08);
    border-radius: 4px;
    cursor: pointer;
    transition: all 0.15s ease;
    text-align: left;
  }

  .pane-card:hover {
    background: rgba(255, 255, 255, 0.06);
    border-color: #00E4E4;
  }

  /* Dashboard view */
  .dashboard-view {
    padding: 24px;
  }

  .stats-grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(200px, 1fr));
    gap: 24px;
  }

  .stat-card {
    background: var(--bg-secondary);
    border: 1px solid var(--border-color);
    border-radius: 8px;
    padding: 24px;
    text-align: center;
    transition: all 0.15s ease;
  }

  .stat-card:hover {
    background: var(--bg-hover);
    border-color: var(--accent);
    box-shadow: 0 4px 12px rgba(0, 228, 228, 0.1);
  }

  .stat-number {
    font-size: 32px;
    font-weight: 600;
    color: var(--accent);
    margin-bottom: 8px;
  }

  .stat-label {
    font-size: 14px;
    color: var(--text-secondary);
  }

  /* Note editor view */
  .note-editor-view {
    height: 100%;
    display: flex;
    flex-direction: column;
  }

  /* Search results */
  .search-results-view {
    padding: 24px;
  }

  .results-sections {
    display: flex;
    flex-direction: column;
    gap: 32px;
  }

  .results-sections .section {
    background: transparent;
  }

  .results-sections .section h3 {
    font-size: 16px;
    font-weight: 600;
    color: var(--text-primary);
    margin: 0 0 16px 0;
  }

  /* Task list view */
  .task-list-view {
    padding: 24px;
  }

  .task-list-view .tasks-list {
    max-height: none;
    background: var(--bg-secondary);
    border-radius: 8px;
    border: 1px solid var(--border-color);
  }

  /* Responsive adjustments */
  @media (max-width: 768px) {
    .project-sections {
      grid-template-columns: 1fr;
    }
    
    .projects-grid {
      grid-template-columns: 1fr;
    }
    
    .stats-grid {
      grid-template-columns: repeat(2, 1fr);
    }
  }
</style>