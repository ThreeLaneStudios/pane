<script lang="ts">
  import { onMount } from 'svelte';
  import QuickSwitcher from '$lib/components/QuickSwitcher.svelte';
  import Templates from '$lib/components/Templates.svelte';
  import Sidebar from '$lib/components/Sidebar.svelte';
  import ChatSidebar from '$lib/components/ChatSidebar.svelte';
  import SessionsDrawer from '$lib/components/SessionsDrawer.svelte';
  import ScheduledPanel from '$lib/components/ScheduledPanel.svelte';
  import CanvasRenderer from '$lib/components/CanvasRenderer.svelte';
  import Onboarding from '$lib/components/Onboarding.svelte';
  import * as noteService from '$lib/services/noteService';
  import {
    notebooks,
    activeNotebook,
    notes,
    activeNote,
    darkMode,
    sidebarCollapsed,
    activeView,
    activeProject,
    activeFolder,
    activeProjectTab,
    activeTag,
    navigationHistory,
    activeTaskId,
    projects,
    chatSidebarOpen,
    cardListCollapsed,
  } from '$lib/stores/app';
  import { canvasState } from '$lib/stores/canvas';
  import { navigateTo } from '$lib/services/navigation';
  import { goToJobSessionBackTarget } from '$lib/services/jobSessionBack';
  import { deriveResponsiveState } from '$lib/utils/responsiveLayout';
  import * as api from '$lib/api';
  import type { NoteMeta, Session } from '$lib/types';
  import { get } from 'svelte/store';
  import { invoke } from '@tauri-apps/api/core';
  import { listen } from '@tauri-apps/api/event';
  import { getCurrentWebview } from '@tauri-apps/api/webview';
  import { editorPreEditText } from '$lib/stores/editor';
  import {
    ENTITY_NOTE,
    ENTITY_PROJECT,
    REASON_CREATED,
    REASON_DELETED,
    REASON_EDITED,
    subscribeDataChanged,
    dispatchDataChanged,
    makeOriginTag,
    registerOwnOriginTag,
    isOwnOriginTag,
    type DataChangedPayload,
  } from '$lib/refreshEvent';

  let showOnboarding = $state(false);
  let authReady = $state(false);
  let windowWidth = $state(window.innerWidth);
  import { autoSync } from '$lib/sync';
  import { initAuth } from '$lib/stores/authStore';

  // probe: instrument all $activeNote write sites — investigating #2214 delete-close latch
  const DEBUG = typeof localStorage !== 'undefined' && localStorage.getItem('pane-debug') === '1';
  function logAnSet(caller: string, newVal: unknown): void {
    if (!DEBUG) return;
    const oldId = get(activeNote)?.meta?.id ?? 'null';
    const newId = (newVal as any)?.meta?.id ?? 'null';
    console.log(`[PANE-DEBUG] $activeNote SET in ${caller} | old="${oldId}" new="${newId}"`);
    console.log('[PANE-DEBUG] stack:', new Error().stack?.split('\n').slice(1, 5).join('\n'));
  }

  // Ref to ChatSidebar so we can forward its handleSessionSwitch to SessionsDrawer.
  // (#395 fix: home drawer needs the same reload callback the project dropdown uses)
  let chatSidebarRef: { handleSessionSwitch: (s: Session) => Promise<void> } | undefined;

  let saveTimeout: ReturnType<typeof setTimeout>;
  let showQuickSwitcher = $state(false);
  let showTemplates = $state(false);
  let autoCollapsed = false;  // Track if sidebar was auto-collapsed by responsive breakpoint
  // #3182 C64: chat's own `autoCollapsed` — same mechanism, second
  // instance, per the sidebar's existing pattern above. Chat and
  // sidebar are independently manually-overridable, so they need
  // independent tracking: `autoChatCollapsed` records ONLY "the narrow
  // breakpoint forced this closed," never "the user chose this,"
  // mirroring exactly what `autoCollapsed` already means for the
  // sidebar. Without this, `$chatSidebarOpen` conflated user intent
  // and layout-forced state (the bug) — there was no way to tell, on
  // widen, whether a closed chat was the user's own choice or the
  // breakpoint's doing.
  let autoChatCollapsed = false;

  // Resizable chat panel
  let chatWidth = $state(parseInt(localStorage.getItem('pane-chat-width') || '420'));
  let isResizing = $state(false);

  function startResize(e: MouseEvent) {
    e.preventDefault();
    isResizing = true;
    const startX = e.clientX;
    const startWidth = chatWidth;
    
    function onMouseMove(ev: MouseEvent) {
      const delta = startX - ev.clientX;
      const containerWidth = document.querySelector('.app')?.clientWidth || window.innerWidth;
      const maxChat = Math.min(900, containerWidth - 220 - 400 - 8);
      chatWidth = Math.max(280, Math.min(maxChat, startWidth + delta));
    }
    function onMouseUp() {
      isResizing = false;
      localStorage.setItem('pane-chat-width', String(chatWidth));
      document.removeEventListener('mousemove', onMouseMove);
      document.removeEventListener('mouseup', onMouseUp);
      document.body.style.cursor = '';
      document.body.style.userSelect = '';
    }
    document.addEventListener('mousemove', onMouseMove);
    document.addEventListener('mouseup', onMouseUp);
    document.body.style.cursor = 'col-resize';
    document.body.style.userSelect = 'none';
  }

  function resetChatWidth() {
    chatWidth = 380;
    localStorage.setItem('pane-chat-width', '380');
  }


  async function checkOnboarding() {
    console.log('[ONBOARD] Checking onboarding status...');
    try {
      const shouldShow: boolean = await invoke('should_show_onboarding');
      if (shouldShow) {
        console.log('[ONBOARD] should_show_onboarding returned true — showing onboarding');
        showOnboarding = true;
      } else {
        console.log('[ONBOARD] should_show_onboarding returned false — skipping');
        showOnboarding = false;
      }
    } catch (err) {
      console.error('[ONBOARD] Error checking onboarding:', err);
      showOnboarding = false; // Database errors should NOT trigger onboarding
    }
  }

  let watcherToast = $state('');
  let watcherToastTimeout: any = $state(null);

  // #3044 (revisited): noteService.createNewNote() dispatches this when
  // Cmd+N or the New Note button is pressed outside a project ($activeProject
  // is null — Home, All Tasks, Calendar, Trash, the project list). This is
  // the ONLY place that message is shown, for either call site — the shared
  // function itself doesn't know about any toast UI, it just signals "blocked"
  // on window, same shape as the existing pane-show-templates event below.
  let newNoteBlockedToast = $state('');
  let newNoteBlockedToastTimeout: any = $state(null);

  // App-wide zoom (Cmd+/- or Settings)
  let zoomLevel = $state(parseFloat(localStorage.getItem('pane_zoom_level') || '1'));

  $effect(() => {
    getCurrentWebview().setZoom(zoomLevel).catch(err => {
      console.error('[ZOOM] setZoom failed, rolling back:', err);
      zoomLevel = 1.0;
      localStorage.setItem('pane_zoom_level', '1');
    });
  });

  function handleZoom(e: KeyboardEvent) {
    if (e.metaKey || e.ctrlKey) {
      if (e.key === '=' || e.key === '+') {
        e.preventDefault();
        zoomLevel = Math.min(2, +(zoomLevel + 0.1).toFixed(1));
        localStorage.setItem('pane_zoom_level', String(zoomLevel));
      } else if (e.key === '-') {
        e.preventDefault();
        zoomLevel = Math.max(0.5, +(zoomLevel - 0.1).toFixed(1));
        localStorage.setItem('pane_zoom_level', String(zoomLevel));
      } else if (e.key === '0') {
        e.preventDefault();
        zoomLevel = 1;
        localStorage.setItem('pane_zoom_level', '1');
      }
    }
  }

  $effect(() => {
    window.addEventListener('keydown', handleZoom);
    const handleZoomChanged = () => {
      zoomLevel = parseFloat(localStorage.getItem('pane_zoom_level') || '1');
    };
    window.addEventListener('pane-zoom-changed', handleZoomChanged);
    return () => {
      window.removeEventListener('keydown', handleZoom);
      window.removeEventListener('pane-zoom-changed', handleZoomChanged);
    };
  });

  // Due-date check debounce (60s — §19.5.1). The entitlement-check cadence
  // this effect used to also drive (ADR-0025, auth_check_entitlement, the
  // grace-period toast) is removed per ADR-0070 Decision 1 (#3195 PR B) —
  // no entitlement check at launch or on focus. The visibilitychange
  // listener that existed solely to trigger that check is removed with it;
  // the tauri://focus listener stays because the due-date watcher below
  // still needs it.
  let lastDueDateCheck = 0;
  const DUE_DATE_CHECK_DEBOUNCE_MS = 60_000;

  let unlistenTauriFocus: (() => void) | null = null;

  // Set up the due-date watcher once auth init has settled.
  $effect(() => {
    if (!authReady) return;

    listen('tauri://focus', () => {
      // PR3b: due-date watcher — independent 60s debounce (§19.5.1)
      const now = Date.now();
      if (now - lastDueDateCheck >= DUE_DATE_CHECK_DEBOUNCE_MS) {
        lastDueDateCheck = now;
        invoke('check_due_dates_cmd').catch(console.error);
      }
    }).then(fn => { unlistenTauriFocus = fn; });

    return () => {
      if (unlistenTauriFocus) unlistenTauriFocus();
    };
  });

  onMount(() => {
    checkOnboarding();

    // Listen for template shortcut from Editor (TipTap intercepts Cmd+Shift+T)
    const templateHandler = () => { showTemplates = true; };
    window.addEventListener('pane-show-templates', templateHandler);

    // ⌘⇧T fix (#2130): capture-phase listener fires before ProseMirror, ensuring the
    // picker always opens regardless of TipTap extension state or editor focus.
    const captureTemplateShortcut = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.shiftKey && e.key === 'T') {
        e.preventDefault();
        showTemplates = true;
      }
    };
    window.addEventListener('keydown', captureTemplateShortcut, { capture: true });

    // #3044 (revisited): Cmd+N calls noteService.createNewNote() directly
    // (see handleKeydown below) — the exact same shared function the New
    // Note button in NotesBrowser.svelte calls, with no window global in
    // between. That function shows this toast itself (via the
    // 'pane-new-note-blocked' event) when pressed outside a project.
    const newNoteBlockedHandler = () => {
      newNoteBlockedToast = 'Open a project to make a note.';
      if (newNoteBlockedToastTimeout) clearTimeout(newNoteBlockedToastTimeout);
      newNoteBlockedToastTimeout = setTimeout(() => { newNoteBlockedToast = ''; }, 3000);
    };
    window.addEventListener('pane-new-note-blocked', newNoteBlockedHandler);

    // Async initialization — runs after sync setup so cleanup return is not async
    (async () => {
      // File watcher toast
      await listen('pane-file-imported', (event: any) => {
        const { title, project } = event.payload;
        watcherToast = `New file detected: ${title} -- added to ${project}`;
        if (watcherToastTimeout) clearTimeout(watcherToastTimeout);
        watcherToastTimeout = setTimeout(() => { watcherToast = ''; }, 5000);
      });

      // Repair orphaned note mappings (fixes project_id/folder_id)
      api.repairNoteMappings().then(n => {
        if (n > 0) console.log(`[REPAIR] Fixed ${n} orphaned notes`);
      }).catch(() => {});

      // Load dark mode preference
      const savedDarkMode = localStorage.getItem('pane-dark-mode');
      if (savedDarkMode !== null) {
        $darkMode = savedDarkMode === 'true';
      } else {
        // Pane is dark-only for now; don't follow OS pref into an unbuilt light theme.
        $darkMode = true;
      }

      // Initialize auth session (ADR-0070: flag check only when
      // account_enabled is off — no network call in the launch path)
      await initAuth();
      authReady = true;

      // Ensure chat sidebar is always open in the new layout
      $chatSidebarOpen = true;

      try {
        // Initialize canvas to home state immediately (no filesystem reads)
        navigateTo({ view: 'home' });
      } catch (e) {
        console.error('Failed to initialize:', e);
      }

      // All filesystem reads happen in background after app is visible
      setTimeout(async () => {
        try {
          $notebooks = await api.listNotebooks();
          await api.syncProjectsWithNotebooks();
        } catch (e) {
          console.warn('Background sync:', e);
        }
      }, 500);
    })();

    return () => {
      window.removeEventListener('pane-show-templates', templateHandler);
      window.removeEventListener('keydown', captureTemplateShortcut, { capture: true });
      window.removeEventListener('pane-new-note-blocked', newNoteBlockedHandler);
    };
  });

  // Apply dark mode
  $effect(() => {
    document.documentElement.setAttribute('data-theme', $darkMode ? 'dark' : 'light');
    localStorage.setItem('pane-dark-mode', String($darkMode));
  });

  // Responsive breakpoints
  $effect(() => {
    function handleResize() {
      windowWidth = window.innerWidth;
      // #3182 C64: branching extracted to a pure function
      // (`deriveResponsiveState`) specifically so it's jsdom-testable
      // without a DOM/stores/window — see that file's own tests for
      // the narrow→medium→wide restore sequence and the
      // "manual close while wide is never force-reopened" guarantee.
      const next = deriveResponsiveState(windowWidth, {
        sidebarCollapsed: $sidebarCollapsed,
        autoCollapsed,
        chatSidebarOpen: $chatSidebarOpen,
        autoChatCollapsed,
      });
      $sidebarCollapsed = next.sidebarCollapsed;
      autoCollapsed = next.autoCollapsed;
      $chatSidebarOpen = next.chatSidebarOpen;
      autoChatCollapsed = next.autoChatCollapsed;
    }
    window.addEventListener('resize', handleResize);
    handleResize();
    return () => window.removeEventListener('resize', handleResize);
  });

  // AI navigation tool listener
  $effect(() => {
    let unlisten: (() => void) | null = null;
    listen('pane-navigate', (event: any) => { console.log('[NAV] pane-navigate event received:', event.payload);
      const { view, tab } = event.payload;
      switch(view) {
        case 'home':
          activeProject.set(null);
          navigateTo({ view: 'home' });
          break;
        case 'project_list':
          activeProject.set(null);
          navigateTo({ view: 'project_list' });
          break;
        case 'all_tasks':
          activeProject.set(null);
          navigateTo({ view: 'all_tasks' });
          break;
        case 'trash':
          activeProject.set(null);
          navigateTo({ view: 'trash' });
          break;
        case 'calendar':
          activeProject.set(null);
          navigateTo({ view: 'calendar' });
          break;
        case 'kanban':
          window.dispatchEvent(new CustomEvent('pane-toggle-kanban'));
          break;
        case 'set_filter':
          window.dispatchEvent(new CustomEvent('pane-set-filter', { detail: { tab } }));
          break;
      }
    }).then(fn => { unlisten = fn; });
    return () => { if (unlisten) unlisten(); };
  });

  async function loadNotes() {
    try {
      // If no specific project/folder context, show all notes
      if (!$activeProject && !$activeFolder) {
        await noteService.loadNotes('');
      } else {
        await noteService.loadNotes($activeNotebook);
      }
    } catch (e) {
      console.error('Failed to load notes:', e);
    }
  }

  // #3044 item 5: handleNewTask — along with TaskEditModal.svelte,
  // showTaskEditModal/selectedTask, and handleTaskSaved/handleTaskDeleted
  // below — deleted outright. Proven unreachable: handleNewTask had ZERO
  // callers anywhere in src/ (confirmed by grep across the whole tree,
  // not just this file) — no button, no keyboard shortcut, no dynamically
  // built navigation ever called it, so showTaskEditModal could never
  // become true and TaskEditModal could never render.

  async function handleQuickSwitcherSelect(note: NoteMeta) {
    try {
      // Navigate to the project (try id first, then name from search results)
      const projectId = note.project_id || (note as any).projectId || '';
      const projectName = note.notebook || (note as any).project_name || '';
      const project = (projectId && $projects.find(p => p.id === projectId)) || $projects.find(p => p.name === projectName);
      if (project) {
        $activeProject = project;
        $activeView = 'notes';
        navigateTo({
          view: 'notes_browser',
          projectId: project.id,
          noteId: note.id || note.slug
        });
        
        // After NotesBrowser loads, select the specific note
        setTimeout(() => {
          window.dispatchEvent(new CustomEvent('pane-select-note', { detail: { noteId: note.id || note.slug } }));
        }, 500);
      }
    } catch (e) {
      console.error('Failed to navigate to note:', e);
    }
  }

  async function handleTemplateSelect(templateId: string) {
    const currentNoteId = $activeNote?.meta?.id;
    if (!currentNoteId) {
      console.error('handleTemplateSelect: no active note to apply template to');
      return;
    }
    try {
      await api.applyTemplate(currentNoteId, templateId);
      // #2972 stage 3 (§34.1.3): migrated from the bare, payload-less
      // `pane-data-changed` CustomEvent to the single entity-identified
      // event, `reason: "edited"`. Registered as this page's own tag
      // (§34.4) so the new bridge listener below (which also reacts to
      // "edited" events for the open note) recognizes this as its OWN
      // content change and does not ALSO fire a second, redundant
      // `pane-reload-editor` dispatch — the explicit one immediately below
      // already covers it.
      const originTag = makeOriginTag();
      registerOwnOriginTag(originTag);
      dispatchDataChanged(currentNoteId, ENTITY_NOTE, REASON_EDITED, originTag);
      setTimeout(() => {
        window.dispatchEvent(new CustomEvent('pane-reload-editor', { detail: { oldText: '' } }));
      }, 200);
    } catch (e) {
      console.error('Failed to apply template:', e);
    }
  }

  // handleNoteNavigation removed — logic lives in NotesBrowser.svelte (handleWikiLinkNavigation)
  // and KanbanBoard.svelte (handleKanbanWikiLinkNavigation). Fix: #2419.

  // handleEditorUpdate deleted per A4 spec - Editor self-saves

  // handleTitleUpdate deleted per A4 spec - Editor handles title updates

  // Global keyboard shortcuts
  // #2972 stage 3 (§34.2's "Replaced" verdict + §34 item 3): the old
  // `listen('pane-note-changed', ...)` bridge above (this comment's
  // predecessor) listened for a Tauri event whose every backend emitter
  // was migrated off that name back in #2972 stage 1 — it had been
  // literal dead code ever since, confirmed at this PR's base by the
  // absence of ANY `pane-note-changed` backend emitter anywhere in
  // `src-tauri/`. Its three jobs (close the editor on an agent delete of
  // the open note, reload the open note's content on an agent edit to
  // it, auto-favorite agent-created projects) had consequently all been
  // silently broken since Stage 1 landed. Replaced with one handler on
  // the real, live event, each of its three branches on `entity_type`/
  // `reason` directly instead of tool-name-array sniffing:
  $effect(() => {
    let unlisten: (() => void) | null = null;
    subscribeDataChanged(async (payload: DataChangedPayload) => {
      const { entity_id: entityId, entity_type: entityType, reason, origin_tag: originTag } = payload;

      // Behavior 1 of 3: an agent (or any foreign-origin) delete of the
      // currently open note closes it — direct `entity_id ===
      // $activeNote.meta.id && reason === "deleted"` check (§34.2),
      // replacing the old `payload.tools.includes('delete_note')` sniff.
      if (entityType === ENTITY_NOTE && reason === REASON_DELETED) {
        if ($activeNote?.meta?.id === entityId) {
          logAnSet('+page.dataChanged[note-deleted]', null);
          $activeNote = null;
        }
        return;
      }

      // Behavior 3 of 3: auto-favorite an agent-created project (#241).
      // Untangled onto its own `entity_type: "project", reason: "created"`
      // branch (§34.2) instead of riding the old note-changed handler's
      // `payload.tools.includes('create_project')` sniff — and, since this
      // event already names the exact project that changed, no more
      // before/after `$projects` ID-set diffing is needed to figure out
      // which one is new.
      if (entityType === ENTITY_PROJECT && reason === REASON_CREATED) {
        try {
          const created = await api.getProjectOrFolder(entityId);
          if (!created.parent_id && created.name !== 'Pane System' && !created.is_favorite) {
            await invoke('update_project_settings', { projectId: entityId, isFavorite: true });
            $projects = await api.syncProjectsWithNotebooks();
            console.log('[AUTO-FAVORITE] DB-favorited new project:', entityId);
          }
        } catch (err) {
          console.error('[AUTO-FAVORITE] Failed to auto-favorite project:', entityId, err);
        }
        return;
      }

      // Behavior 2 of 3: an agent (or any foreign-origin) edit to the
      // CONTENT of the currently open note reloads it. §34.4's echo
      // suppression applies here too — `isOwnOriginTag` skips this
      // page's OWN `handleTemplateSelect` dispatch (which already fires
      // its own explicit `pane-reload-editor` immediately below it) and
      // skips the open Editor's own saves (Editor.svelte registers its
      // own tags for its own saves) — only a GENUINELY foreign edit to
      // the open note reaches this branch.
      if (entityType === ENTITY_NOTE && reason === REASON_EDITED && entityId === $activeNote?.meta?.id) {
        if (isOwnOriginTag(originTag)) return;
        // Capture editor text BEFORE the reload for diff highlighting.
        const oldText = $editorPreEditText || '';
        setTimeout(() => {
          window.dispatchEvent(new CustomEvent('pane-reload-editor', { detail: { oldText } }));
        }, 200);
      }
    }).then(fn => { unlisten = fn; });
    return () => { if (unlisten) unlisten(); };
  });

  async function handleKeydown(e: KeyboardEvent) {
    const meta = e.metaKey || e.ctrlKey;

    if (meta && e.key === 'n' && !e.shiftKey) {
      // Let KanbanBoard handle Cmd+N when on kanban view (creates a task
      // card, a different entity/flow entirely — out of scope here, and
      // unaffected by #3044 — KanbanBoard.svelte's own window keydown
      // listener, pre-existing since #424, continues to own this case).
      if ($canvasState.view === 'kanban') return;
      e.preventDefault();
      // #3044 (revisited): Cmd+N calls noteService.createNewNote() directly
      // — the exact same shared function the New Note button
      // (NotesBrowser.svelte) calls. No window global, no "is the right
      // component mounted" precondition: this works from every screen,
      // including ones NotesBrowser never renders for (e.g. project_config).
      noteService.createNewNote();
    }

    // #3044 ruling: Cmd+T removed — only Cmd+N creates a note.

    if (meta && e.shiftKey && e.key === 'f') {
      e.preventDefault();
      showQuickSwitcher = true;
    }

    if (meta && e.shiftKey && e.key === 'T') {
      e.preventDefault();
      showTemplates = true;
    }

    if (meta && e.key === '\\') {
      e.preventDefault();
      $chatSidebarOpen = !$chatSidebarOpen;
      // #3182 C64: user manually toggled — reset auto-collapse
      // tracking, same reasoning as the sidebar's own manual-override
      // sites resetting `autoCollapsed`. Without this, a manual
      // re-open while still narrow could get silently re-closed by
      // the NEXT resize event's narrow branch (which only acts when
      // `$chatSidebarOpen` is true — see handleResize above), and a
      // manual close is never mistaken for a forced one that a later
      // widen would try to undo.
      autoChatCollapsed = false;
    }

    // #3151 PR1 Commit F: Cmd+[ — the one-deep back target (job card/bell
    // entry points). A safe no-op when there's nothing to go back to
    // (`goToJobSessionBackTarget` itself checks the store; this handler
    // doesn't need its own duplicate guard) — exactly what a second
    // Cmd+[ press does: nothing, same as the back button's own `{#if}`
    // guard disappearing.
    if (meta && e.key === '[') {
      e.preventDefault();
      void goToJobSessionBackTarget();
    }

    if (meta && e.shiftKey && e.key === 'B') {
      e.preventDefault();
      $cardListCollapsed = !$cardListCollapsed;
    }

    if (e.key === 'Escape') {
      showQuickSwitcher = false;
      showTemplates = false;
    }

    // Cmd+1: Home
    if (meta && e.key === '1') {
      e.preventDefault();
      navigateTo({ view: 'home' });
    }

    // Cmd+2: All Projects
    if (meta && e.key === '2') {
      e.preventDefault();
      navigateTo({ view: 'project_list' });
    }

    // Cmd+3: All Tasks
    if (meta && e.key === '3') {
      e.preventDefault();
      navigateTo({ view: 'all_tasks' });
    }

    // Cmd+/: Focus chat input
    if (meta && e.key === '/') {
      e.preventDefault();
      const chatInput = document.querySelector('.chat-input-box textarea, .chat-input textarea, .chat-input-area textarea') as HTMLTextAreaElement;
      if (chatInput) chatInput.focus();
    }

  }
</script>

<svelte:window onkeydown={handleKeydown} />

{#if !authReady}
  <div class="auth-loading"><div class="auth-loading-mark">Pane</div></div>
{:else}

{#if showOnboarding}
  <Onboarding onComplete={async () => {
    showOnboarding = false;
    // Force sidebar to reload projects after onboarding
    // is_favorite is a DB column — reloading $projects is sufficient
    try {
      const refreshed = await api.syncProjectsWithNotebooks();
      $projects = refreshed;
    } catch {}
  }} />
{/if}
<div class="app" data-theme={$darkMode ? 'dark' : 'light'} data-testid="app-loaded">
  <!-- Navigation sidebar -->
  <Sidebar />
  
  <!-- Main content area -->
  <div class="main-content" class:hidden={$canvasState.view === 'home'}>
    <CanvasRenderer />
  </div>
  
  <!--
    Chat/session hosting is intentionally absent on the Settings view (#2536, ADR-0063 D8).
    Settings is a configuration surface, not a conversation surface — no ChatSidebar, no
    resize handle, no collapsed-chat expand affordance render while $canvasState.view is
    'settings'. This also removes the trigger surface for the 8/14 stale-projectId leak,
    where navigating to Settings from within a project context carried a stale projectId
    into a Settings-hosted chat session.
  -->
  {#if $canvasState.view !== 'settings'}
    <!-- Resizable divider (hidden on home view) -->
    {#if $chatSidebarOpen && $canvasState.view !== 'home'}
    <div class="resize-handle chat-resize-handle"
      class:dragging={isResizing}
      onmousedown={startResize}
      ondblclick={resetChatWidth}
      role="separator"
      aria-orientation="vertical"
      tabindex="-1"
    ></div>
    {/if}

    <!-- Chat sidebar (always mounted except on Settings, layout changes via CSS) -->
    <div class="chat-panel" class:home-view={$canvasState.view === 'home'} class:collapsed={!$chatSidebarOpen && $canvasState.view !== 'home'} style:width="{$canvasState.view === 'home' ? '' : chatWidth + 'px'}">
      {#if $canvasState.view === 'home'}
        <!--
          §36.3.8 (#3050 PR 8): the Scheduled panel docks on the LEFT,
          always — a fixed 352px flex-row sibling, mounted BEFORE
          ChatSidebar so it sits to its left. ChatSidebar (flex:1,
          layoutMode="centered") absorbs every width change — the panel
          never moves or resizes, same mechanism SessionsDrawer already
          uses on the right (that file's own header, lines 5-8), mirrored
          onto this side. Design sheet README "Layout rules" + Notes #1/#2.
        -->
        <ScheduledPanel />
      {/if}
      <ChatSidebar
        bind:this={chatSidebarRef}
        layoutMode={$canvasState.view === 'home' ? 'centered' : 'sidebar'}
        alwaysOpen={$canvasState.view === 'home'}
      />
      {#if $canvasState.view === 'home'}
        <!--
          onSessionSwitch is REQUIRED — wires home drawer to the same reload handler
          the project dropdown uses (ChatSidebar.handleSessionSwitch).
          BUG-1 fix: this prop was missing, so switching sessions never reloaded messages.
          (#395 fix)
        -->
        <SessionsDrawer onSessionSwitch={(s) => chatSidebarRef!.handleSessionSwitch(s)} />
      {/if}
    </div>
    {#if !$chatSidebarOpen && $canvasState.view !== 'home'}
      <!-- #3182 C64: matches the `.chat-panel`'s own
           `class:collapsed={!$chatSidebarOpen && $canvasState.view !== 'home'}`
           guard two lines above — Home's `alwaysOpen` ChatSidebar means
           $chatSidebarOpen can be false on Home without chat actually
           being collapsed; without this guard the sliver+expand-button
           would render ALONGSIDE the still-visible Home chat panel. -->
      <div class="chat-panel-collapsed">
        <button class="chat-expand-btn" onclick={() => { $chatSidebarOpen = true; autoChatCollapsed = false; }} title="Open Chat">
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <polyline points="15,18 9,12 15,6"></polyline>
          </svg>
        </button>
      </div>
    {/if}
  {/if}
</div>

{#if showQuickSwitcher}
  <QuickSwitcher
    onClose={() => showQuickSwitcher = false}
    onSelect={handleQuickSwitcherSelect}
  />
{/if}

{#if showTemplates}
  <Templates
    onClose={() => showTemplates = false}
    onSelectTemplate={handleTemplateSelect}
  />
{/if}

{/if}

{#if watcherToast}
  <div class="watcher-toast">{watcherToast}</div>
{/if}

{#if newNoteBlockedToast}
  <div class="watcher-toast" data-testid="new-note-blocked-toast">{newNoteBlockedToast}</div>
{/if}

<style>
  .app {
    display: flex;
    /* C27 (#3144), second attempt: the prior fix (`ad0c074a`, #3139)
       changed this from 100 vw / 100 vh to a percent-of-ancestor pair
       on the theory that viewport units diverge from percent under a
       zoomed WKWebView — that theory was never confirmed, and the
       percent value it shipped broke the app completely on 1.0.72 (no
       session scrolled at all; see #3144). A later theory (percentage
       height not passing through `app.html`'s `display: contents`
       wrapper) was tested and also NOT confirmed (6/6 structural
       variants passed in both WebKit and Chromium — see
       docs/audits/2026-11-02-scope-3144-c27-scroll.md).
       This sizing is deliberately neither percentage-of-ancestor nor
       viewport-unit, because the real mechanism that drops `.app`'s
       height is still unconfirmed: `position: fixed` resolves its
       containing block against the viewport's own geometry (the
       initial containing block, since no ancestor sets `transform` /
       `filter` / `perspective` / `contain` / `backdrop-filter` /
       `will-change` naming one of those), a third resolution path that
       depends on neither of the two already-discredited mechanisms.
       Proven empirically: forcing an ancestor box to collapse to zero
       height reproduces the exact 1.0.72 break under the percent pair,
       and recovers completely under `position: fixed; inset: 0`. Do
       not "simplify" this back to a percent pair or to any viewport
       unit (vw/vh/dvh/dvw) — see #3144. */
    position: fixed;
    inset: 0;
    overflow: hidden;
    /* #3188: single source of truth for the sidebar rail's resting width,
       previously a `40px` literal duplicated here (via `.chat-panel.
       home-view`'s `left` bound below) and in Sidebar.svelte's own
       `.sidebar` width rule. Declared on `.app` so it inherits down to
       both consumers through the DOM — no behavior change, no
       positioning math reintroduced (per #3188's own guardrail). */
    --sidebar-rail-width: 40px;
    background: #0a0a0f;
    color: #e0e0e0;
    font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Text', 'Segoe UI', sans-serif;
    -webkit-font-smoothing: antialiased;
    font-weight: 400;
  }

  .main-content {
    flex: 1;
    overflow: hidden;
    display: flex;
    flex-direction: column;
    min-width: 0;
    max-width: 100%;
  }

  .main-content.hidden {
    display: none;
  }

  .resize-handle {
    width: 4px;
    cursor: col-resize;
    background: transparent;
    transition: background 0.15s ease;
    flex-shrink: 0;
    position: relative;
  }
  .resize-handle:hover {
    background: rgba(0, 228, 228, 0.3);
  }
  .resize-handle:active,
  .resize-handle.dragging {
    background: rgba(0, 228, 228, 0.5);
  }

  .chat-panel {
    overflow: hidden;
    display: flex;
    flex-direction: column;
    border-left: 1px solid rgba(255, 255, 255, 0.06);
    position: relative;
    /* C10 (#3120 PR 3): was z-index: 1. `.main-content` (this div's own
       sibling) has no `position`/`z-index` of its own, so it never forms
       a stacking context — every z-index inside it (e.g. NotesBrowser's
       `.grid-folder-toolbar`, position: sticky, z-index: 5) is compared
       DIRECTLY against this element's z-index, not contained by it. 5 > 1,
       so that sticky project toolbar painted on top of this entire
       stacking context, including NewSessionSheet's fixed-position
       `.ns-sheet` nested deep inside it at its own z-index: 200 —
       z-index:200 only matters WITHIN this stacking context, so it could
       never out-rank an external z-index:5 sibling once this element lost
       that comparison. Raised well above every in-project toolbar/dropdown
       z-index found in `NotesBrowser.svelte`/`KanbanBoard.svelte` (max
       1000), while staying well below the app's own top-level overlays
       (QuickSwitcher/Templates/auth-loading, all siblings of `.app`
       itself at z-index 9999, unaffected by this value either way). */
    z-index: 1001;
  }

  .chat-panel:not(.home-view) {
    flex-shrink: 0;
  }

  .chat-panel.home-view {
    flex: 1;
    display: flex;
    flex-direction: row; /* ChatSidebar (flex:1 centered) + SessionsDrawer side-by-side */
    border-left: none;
  }

  .chat-panel.collapsed {
    width: 0 !important;
    overflow: hidden;
    pointer-events: none;
    opacity: 0;
    border-left: none;
  }

  @media (max-width: 899px) {
    .chat-panel {
      position: fixed;
      top: 0;
      right: 0;
      bottom: 0;
      z-index: 100;
      box-shadow: -4px 0 20px rgba(0, 0, 0, 0.5);
      background: #0a0a0f;
    }

    /* #3186 C65 — every non-Home route pins an explicit inline
       `width` on `.chat-panel` regardless of width band (see the
       `style:width` binding on the element below), so the missing
       `left` here is harmless for them: `right: 0` + a concrete width
       already resolves to a bounded drawer. Home sets `style:width=""`
       (empty) instead, so with no `left`/`width` bound at all, this
       box's `flex: 1` child (ChatSidebar, centered layout) is free to
       shrink-to-fit wider than the viewport — measured in WebKit at an
       800px viewport: `left: -10.6px; width: 810.6px`, extending past
       the left edge and painting over the 40px rail in the same
       background color (#0a0a0f both), which reads as "the sidebar
       disappeared" rather than "something is covering it." Bounding
       `left` to the rail's own 40px width stops that — scoped to
       `.home-view` only so the already-pinned non-Home drawer is
       untouched. */
    .chat-panel.home-view {
      left: var(--sidebar-rail-width, 40px);
    }
  }

  .chat-panel-collapsed {
    flex-shrink: 0;
    width: 36px;
    display: flex;
    align-items: flex-start;
    justify-content: center;
    padding-top: 16px;
    background: #0a0a0f;
    border-left: 1px solid rgba(255, 255, 255, 0.06);
  }

  .chat-expand-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 24px;
    height: 24px;
    border: none;
    background: transparent;
    color: #666666;
    cursor: pointer;
    transition: color 0.15s ease;
  }

  .chat-expand-btn:hover {
    color: #00E4E4;
  }
  .auth-loading {
    position: fixed;
    inset: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    background: #0a0a0f;
    z-index: 9999;
  }

  .auth-loading-mark {
    color: white;
    font-size: 24px;
    font-weight: 700;
    font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Text', 'Segoe UI', sans-serif;
    opacity: 0.6;
  }
  .watcher-toast { position: fixed; bottom: 24px; right: 24px; background: rgba(0,228,228,0.1); border: 1px solid rgba(0,228,228,0.3); color: #e0e0e0; padding: 10px 16px; border-radius: 6px; font-size: 13px; z-index: 9999; animation: watcherToastIn 0.3s ease; }
  @keyframes watcherToastIn { from { opacity: 0; transform: translateY(10px); } to { opacity: 1; transform: translateY(0); } }
</style>