import { writable, derived } from 'svelte/store';
import type { NoteMeta, NoteContent, Project, Folder, Task } from '$lib/types';

// Projects and Folders
export const projects = writable<Project[]>([]);
export const activeProject = writable<Project | null>(null);
export const activeFolder = writable<Folder | null>(null);

// Legacy notebook stores (kept for compatibility during migration)
export const notebooks = writable<string[]>([]);
export const activeNotebook = writable<string>('');

export const notes = writable<NoteMeta[]>([]);
export const activeNote = writable<NoteContent | null>(null);
export const tasks = writable<Task[]>([]);
export const activeTask = writable<Task | null>(null);
export const searchQuery = writable<string>('');
export const searchResults = writable<NoteMeta[]>([]);
export const isSearching = writable<boolean>(false);
export const darkMode = writable<boolean>(true);

// Sidebar collapsed state - defaults to expanded (false)
// Note: Navigation sidebar should remain user-controlled and is NEVER auto-collapsed
export const sidebarCollapsed = writable<boolean>(false);

// View types: 'today' | 'inbox' | 'project' | 'project-aggregate' | 'folder' | 'tag' | 'search'
export const activeView = writable<string>('today');

// For project/folder view: which tab is active
export const activeProjectTab = writable<'notes' | 'tasks'>('notes');
export const activeViewTab = writable<'all' | 'cabinet' | 'tasks'>('all');
export const editorListWidth = writable<number>(384);

// For task view within project: list or kanban
export const taskViewMode = writable<'list' | 'kanban'>('list');

// Active tag filter
export const activeTag = writable<string>('');

// Navigation history for back button
export const navigationHistory = writable<Array<{
  view: string;
  projectId?: string;
  folderId?: string;
  taskId?: string;
  notebook?: string;
  slug?: string;
  title?: string;
  tag?: string;
}>>([]);
export const activeTaskId = writable<string | null>(null);
// Card list panel collapsed state (split pane view)
export const cardListCollapsed = writable<boolean>(
  typeof window !== 'undefined' ? localStorage.getItem('pane-card-list-collapsed') === 'true' : false
);


// Chat stores
export const chatSidebarOpen = writable<boolean>(false);
export const activeChatThreadId = writable<string | null>(null);

/**
 * #3151 PR1 Commit F: one-deep "where was I before I followed a job
 * card/bell click" target. `null` = no back target — the back button
 * (`ChatPanel.svelte`'s header) and the `Cmd+[` shortcut (`+page.svelte`)
 * both render/act only when this is non-null, and a SECOND back press
 * (or Cmd+[) is a no-op by construction: `goToJobSessionBackTarget`
 * (`$lib/services/jobSessionBack.ts`) clears this back to `null` as its
 * own first step, before anything else — there is no deeper stack,
 * matching the issue's own "one-deep is sufficient" framing. Deliberately
 * NOT the pre-existing `navigationHistory` store above — that is a
 * general (and today, apparently unused beyond its own declaration)
 * breadcrumb mechanism for notes/views; this is scoped narrowly to the
 * two job-session entry points (the Jobs card's "Open session" and the
 * notification bell), set via `recordJobSessionEntry`
 * (`$lib/services/jobSessionBack.ts`), never by an ordinary sidebar
 * click between two normal sessions.
 */
export const jobSessionBackTarget = writable<string | null>(null);

/**
 * #3155 C49b: the ACTIVE session's own `origin` column, mirrored
 * reactively alongside `activeChatThreadId` at every site that resolves
 * a session (`navigation.ts`'s `navigateTo` STEP 3, and
 * `ChatSidebar.svelte`'s `loadOrCreateThread`/`handleSessionSwitch`,
 * which both already receive the full `Session` object with `origin` on
 * it). Added so `showBackButton` can gate on "is the session CURRENTLY
 * on screen actually a job session" (`origin IN ('spawn', 'schedule')`,
 * the existing §36.11 convention for a job's own dedicated session) —
 * before this, `showBackButton` only checked whether a back TARGET
 * existed, never whether the ACTIVE session was a job session at all,
 * which is how the back button followed Matt onto an unrelated session
 * (Aurum) after a manual sidebar switch. `null` on Home's `'workspace'`
 * sentinel and on navigation failure/fallback — never a job-session
 * origin value, so `showBackButton` correctly reads false in both cases.
 */
export const activeSessionOrigin = writable<string | null>(null);

// Category collapsed state (persisted in localStorage)
export const collapsedCategories = writable<Set<string>>(new Set());

// Derived store for displayed notes (search results or notebook notes)
export const displayedNotes = derived(
  [isSearching, searchResults, notes, activeNotebook],
  ([$isSearching, $searchResults, $notes, $activeNotebook]) => {
    if ($isSearching) return $searchResults;
    // Filter notes to current notebook to prevent cross-folder bleed
    if ($activeNotebook) {
      return $notes.filter(n => n.notebook === $activeNotebook || n.id?.startsWith($activeNotebook + '/'));
    }
    return $notes;
  }
);
