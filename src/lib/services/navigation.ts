import { get } from 'svelte/store';
import { canvasState, type CanvasViewType } from '$lib/stores/canvas';
import { activeProject, activeChatThreadId, activeSessionOrigin, projects } from '$lib/stores/app';
import * as api from '$lib/api';
import { isVisibleLoadedMessage } from '$lib/services/messageVisibility';

let navigationGeneration = 0;

export interface NavigateDestination {
  view: CanvasViewType;
  projectId?: string;
  noteId?: string;
  folderId?: string;
  taskId?: string;
  searchQuery?: string;
  tagName?: string;
  /**
   * §36.7 (#3050 PR 6): a specific session to land on, overriding STEP 3's
   * default `getActiveSession(projectId)` resolution. The one caller today
   * is a notification's click-to-open (`NotificationPanel.svelte`,
   * `target_kind = 'session'`) — "open this exact session," not "open
   * whatever this project's active session happens to be." Safe for a
   * job-origin (hidden) session id too: `api.switchSession` no-ops the
   * promote/demote mutation for those (Ruling 2's guard), it still
   * resolves and returns the session for display.
   */
  sessionId?: string;
}

export interface NavigateOptions {
  skipMessageLoad?: boolean;     // Skip loadMessages (e.g., navigating during stream)
  clearMessages?: boolean;        // Clear allMessages before navigation (default: true)
  forceReload?: boolean;          // Force loadMessages even if threadId unchanged (default: false)
}

export interface NavigationResult {
  threadId: string;
  messages: any[];
  shouldClearMessages: boolean;
  shouldLoadMessages: boolean;
}

/**
 * Imperative navigation — replaces reactive canvasState → effect chain.
 * 
 * Guarantees:
 * - Canvas state updated BEFORE thread resolution
 * - Thread resolved BEFORE messages loaded
 * - Active project/note updated atomically with canvas
 * - No race conditions from effect firing order
 * - No stale state from debounce/cancellation
 * 
 * @param destination - Where to navigate (view + context)
 * @param options - Navigation behavior flags
 * @returns Promise<NavigationResult> with threadId and messages
 */
export async function navigateTo(
  destination: NavigateDestination,
  options: NavigateOptions = {}
): Promise<NavigationResult> {
  const { skipMessageLoad = false, clearMessages = true, forceReload = false } = options;
  const gen = ++navigationGeneration;
  
  console.log('[NAV] navigateTo:', destination.view, 'projectId:', destination.projectId, 'gen:', gen);

  // STEP 0: Auto-resolve projectId for note_editor when absent
  if (destination.view === 'note_editor' && destination.noteId && !destination.projectId) {
    try {
      const rawNote: any = await api.getNoteRaw(destination.noteId);
      if (rawNote?.meta?.project_id) {
        destination = { ...destination, projectId: rawNote.meta.project_id };
        console.log('[NAV] Auto-resolved projectId:', destination.projectId, 'for noteId:', destination.noteId);
      }
    } catch (e) {
      console.warn('[NAV] Could not resolve projectId for note_editor:', e);
      // Fall through — same graceful no-op as today
    }
  }

  // STEP 1: Update canvas state (sync)
  // This is the authoritative navigation state — UI renders from this
  // Look up names before setting so we can include them atomically
  let projectName: string | undefined;
  let folderName: string | undefined;
  if (destination.projectId) {
    const allProjects = get(projects);
    const project = allProjects.find(p => p.id === destination.projectId);
    projectName = project?.name;
    if (destination.folderId) {
      const folder = allProjects.find(p => p.id === destination.folderId);
      folderName = folder?.name;
    }
  }

  canvasState.set({
    view: destination.view,
    data: {},
    context: {
      projectId: destination.projectId,
      projectName,
      noteId: destination.noteId,
      folderId: destination.folderId,
      folderName,
      taskId: destination.taskId,
      searchQuery: destination.searchQuery,
      tagName: destination.tagName,
    }
  });
  
  // STEP 2: Update active project (if project view)
  if (destination.projectId) {
    const project = get(projects).find(p => p.id === destination.projectId);
    if (project) {
      activeProject.set(project);
    } else {
      console.warn('[NAV] Project not found:', destination.projectId);
      // Don't fail navigation — user might be viewing a deleted project
    }
  } else if (destination.view === 'home' || destination.view === 'project_list' || 
             destination.view === 'all_tasks' || destination.view === 'trash' || 
             destination.view === 'calendar') {
    // Non-project views — clear active project
    activeProject.set(null);
  }
  
  // STEP 3: Resolve thread ID — a specific session override (§36.7's
  // click-to-open, #3050 PR 6) takes priority over the default "this
  // project's active session" resolution (§30.2.4 re-key).
  const session = destination.sessionId
    ? await api.switchSession(destination.sessionId)
    : await api.getActiveSession(destination.projectId || 'workspace');
  const threadId = session.id;
  
  // Check if stale
  if (gen !== navigationGeneration) {
    console.log('[NAV] Stale navigation cancelled (after thread resolution), gen:', gen, 'current:', navigationGeneration);
    return { threadId: '', messages: [], shouldClearMessages: false, shouldLoadMessages: false };
  }
  
  const previousThreadId = get(activeChatThreadId);
  activeChatThreadId.set(threadId);
  // #3155 C49b: mirror the resolved session's own `origin` alongside its
  // id — `showBackButton`'s active-session-origin gate (`ChatSidebar.svelte`)
  // reads this, not a fresh fetch, so it's always in lockstep with
  // `activeChatThreadId` through this ONE choke point every `navigateTo`
  // caller already goes through.
  activeSessionOrigin.set(session.origin ?? null);

  // §36.7 (#3050 PR 6): tell the backend what's now on screen. Settings is
  // the one view with no ChatSidebar mounted at all (ADR-0063 D8, #2536
  // — see +page.svelte's own comment on the `$canvasState.view !==
  // 'settings'` guard around <ChatSidebar>) — nothing is "open" there, so
  // this is chat-view teardown, not a session switch; every other view
  // keeps ChatSidebar mounted showing `threadId`. This one call site
  // covers every `navigateTo` caller: project switch, Home, the
  // notification click-to-open's `sessionId` override, and the project/
  // notes/trash/etc. navigations that resolve a session alongside their
  // own canvas view — a future `navigateTo` caller gets this for free
  // without adding its own call site.
  try {
    await api.setViewingSession(destination.view === 'settings' ? null : threadId);
  } catch (e) {
    console.warn('[NAV] setViewingSession failed (non-fatal — notification gating may be stale):', e);
  }

  console.log('[NAV] Thread resolved:', threadId, 'for projectId:', destination.projectId);
  
  // STEP 4: Load messages (always load for project views unless skipMessageLoad is set)
  // The optimization of skipping when threadId unchanged was the source of #406
  // Loading 50 messages from SQLite takes ~5ms, so always reload
  const shouldLoad = !skipMessageLoad;
  let messages: any[] = [];
  
  if (shouldLoad) {
    console.log('[NAV] Loading messages for threadId:', threadId);
    messages = await api.getChatMessages(threadId, 50);
    
    // Check if stale after async message load
    if (gen !== navigationGeneration) {
      console.log('[NAV] Stale navigation cancelled (after message load), gen:', gen, 'current:', navigationGeneration);
      return { threadId: '', messages: [], shouldClearMessages: false, shouldLoadMessages: false };
    }
    
    messages = messages ?? [];
    messages.reverse(); // DB returns DESC, display needs ASC
    
    // #3166 C55: shared with ChatSidebar.svelte's `loadMessages` via
    // `isVisibleLoadedMessage` — this filter used to be a hand-rolled,
    // independent copy with NO `synthetic` exclusion at all, which is
    // half of why the two loaders disagreed about a job-brief row.
    messages = messages.filter(isVisibleLoadedMessage);
  } else {
    console.log('[NAV] Skipping loadMessages —', skipMessageLoad ? 'skipMessageLoad=true' : `threadId unchanged (${threadId}) and forceReload=false`);
  }
  
  console.log('[NAV] Navigation complete:', destination.view);
  
  // Dispatch custom event for ChatSidebar to consume
  const event = new CustomEvent('pane-navigation-complete', {
    detail: {
      threadId,
      messages,
      shouldClearMessages: clearMessages,
      shouldLoadMessages: shouldLoad,
      destination,
      options
    }
  });
  window.dispatchEvent(event);
  
  return {
    threadId,
    messages,
    shouldClearMessages: clearMessages,
    shouldLoadMessages: shouldLoad
  };
}

/**
 * Safe wrapper with error handling and fallback recovery
 */
export async function navigateToSafe(
  destination: NavigateDestination,
  options: NavigateOptions = {}
): Promise<{ success: boolean; error?: string }> {
  try {
    await navigateTo(destination, options);
    return { success: true };
  } catch (error: any) {
    console.error('[NAV] Navigation failed:', error);
    
    // Recovery: fallback to workspace thread
    console.warn('[NAV] Navigation failed, falling back to workspace');
    try {
      activeChatThreadId.set('workspace');
      // #3155 C49b: the fallback thread is never a job session.
      activeSessionOrigin.set(null);
      return { success: false, error: error.message || 'Navigation failed' };
    } catch (fallbackError) {
      return { success: false, error: 'Navigation and fallback both failed' };
    }
  }
}
