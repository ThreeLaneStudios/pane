import { writable } from 'svelte/store';

export type CanvasViewType = 
  | 'welcome'        // Default blank/welcome screen
  | 'project_list'   // All projects grouped by category
  // | 'project_detail' // REMOVED v10.13.0 - replaced by notes_browser
  | 'note_editor'    // Single note in editor
  // | 'note_list' // REMOVED #3044 item 5 — pure rendering alias of
  // notes_browser (CanvasRenderer.svelte rendered both identically) with
  // ZERO real call sites left anywhere (user OR agent) once #3044 item 3
  // deleted +page.svelte's own handleNewNote, its only citation
  // (userReachableScan.ts's derived scan, run after that deletion,
  // confirmed zero routes — see userReachableViews.derived.test.ts).
  // | 'task_list' // REMOVED #3044 item 5 — zero real call sites anywhere
  // (user or agent; the backend's show_view/navigate tools actively
  // REJECT it per #3022/#3043). TaskList.svelte (the component this
  // rendered) is deleted in the same commit.
  | 'task_detail'    // Single task detail
  | 'kanban'         // Kanban board for a project
  | 'today'          // Today view (tasks due today)
  | 'all_tasks'      // All tasks view (list and kanban modes)
  | 'calendar'       // Calendar view
  | 'search_results' // Search results
  | 'settings'       // Settings
  | 'project_config'   // Project configuration (v0.60.0 - renamed from project_settings)
  // | 'dashboard' // REMOVED #3044 item 5 — zero real call sites anywhere
  // (user or agent), same basis as 'task_list' above. Its inline render
  // branch in CanvasRenderer.svelte is deleted in the same commit.
  | 'notes_browser'  // Unified note browsing (project or folder scope)
  // | 'folder_detail'  // REMOVED v10.13.0 - replaced by notes_browser
  | 'trash'          // Trash view
  | 'tag'            // Tag filter view
  | 'tags-manage'    // Tag management view
  | 'home';          // Home dashboard

/**
 * Views `CanvasRenderer.svelte` actually has a render branch for — NOT the
 * full `CanvasViewType` union above, which also declares `welcome`,
 * `today`, `tag`, `tags-manage`, and `task_detail` with no renderer branch
 * at all (confirmed by reading every `currentCanvasState.view ===` check in
 * `CanvasRenderer.svelte`; those five hit its own generic `{:else}`
 * fallback, "Unknown view: X", if ever set — #3036 PR2 audit).
 *
 * `ChatSidebar.svelte`'s navigation dispatcher (`navigationDispatch.ts`)
 * treats any `view` string NOT in this list as unrecognized and surfaces a
 * visible error instead of silently doing nothing (#3036 PR2 item 4).
 *
 * #3044 item 5: `task_list` and `dashboard` used to also be here —
 * CanvasRenderer.svelte COULD draw both, but #3043 had already found
 * neither had a real user control, and this PR's own re-run of the
 * derived scan confirmed neither has a real AGENT route either. Matt's
 * ruling this time: prove no route reaches it, then delete rather than
 * leave as a dead-but-drawable surface — both render branches (and
 * `TaskList.svelte`) are deleted in the same commit.
 */
export const RENDERED_CANVAS_VIEWS = [
  'home',
  'project_list',
  'notes_browser',
  'note_editor',
  'all_tasks',
  'calendar',
  'trash',
  'kanban',
  'search_results',
  'settings',
  'project_config',
] as const satisfies readonly CanvasViewType[];

/**
 * Screens with a real USER CONTROL that opens them — a button, menu item,
 * or keyboard shortcut somewhere in the app chrome. #3043, Matt's
 * live-testing ruling on #3036 PR2: **"live" means a user control opens
 * it, not that `CanvasRenderer.svelte` merely has a render branch that CAN
 * draw it.** `task_list` and `dashboard` used to be exactly this kind of
 * real, drawable, DEAD surface (tracked on #3023) — #3044 item 5 deleted
 * both outright instead (see `RENDERED_CANVAS_VIEWS`'s own comment above).
 *
 * **No longer purely hand-maintained (#3043 item 2).** This list used to
 * be a judgment call with no mechanical guard — Reed's #3043 delta-2
 * audit found that gap had already let `search_results` sit in this
 * exact array with ZERO real call site behind it (removed below, see
 * item 4). `userReachableScan.ts`'s `scanUserReachableViews` now derives
 * the same set MECHANICALLY by scanning every real `navigateTo(...)`/
 * `canvasState.set(...)`/`canvasState.update(...)` call site in `src/`
 * (excluding the agent's own navigation-dispatch code, which can't count
 * as a way IN — the agent being able to request a view is exactly what
 * this list exists to constrain). `userReachableViews.derived.test.ts`
 * asserts the derived scan and this array are IDENTICAL sets, naming any
 * extra/missing view if they ever drift — a continuous mechanical check
 * this array never had before.
 */
export const USER_REACHABLE_VIEWS = [
  'home',
  'project_list',
  'notes_browser',
  'note_editor',
  'all_tasks',
  'calendar',
  'trash',
  'kanban',
  'settings',
  'project_config',
] as const satisfies readonly (typeof RENDERED_CANVAS_VIEWS)[number][];

/**
 * The subset of `USER_REACHABLE_VIEWS` the AI agent's `show_view` tool can
 * target directly — a bare screen switch with no entity id required.
 * Pinned 1:1 against `show_view`'s `view` enum in
 * `src-tauri/src/ai/tools.rs` (the `SHOW_VIEW_SCREENS` Rust constant there)
 * by `canvas.showViewParity.test.ts` — #3036 PR2's original screen
 * enumeration, narrowed by #3043 (Matt's ruling removed `task_list` and
 * `dashboard`: both render fine but no user control opens either one).
 *
 * Deliberately excluded, with reasons (see the #3036 PR2 PR body and the
 * #3043 ruling for the full writeup):
 * - `home`, `project_list`, `all_tasks`, `calendar`, `trash` — already
 *   reachable via their own dedicated `navigate_home`/`navigate_all_
 *   projects`/`navigate_all_tasks`/`navigate_calendar`/`navigate_trash`
 *   tools; not duplicated here so only one tool owns each screen.
 * - `kanban` — already reachable via `navigate_kanban` (which relies on
 *   ambient project context already in `canvasState`, a different
 *   mechanism than `show_view`'s explicit `project_id` resolution); not
 *   duplicated here to avoid two divergent code paths for the same screen.
 * - `note_editor` — contextual to a specific note (reached via `navigate`'s
 *   `note_id`); not a "switch to a named screen" request. (`note_list`,
 *   the other contextual alias this bullet used to also name, was removed
 *   outright in #3044 item 5 — it had zero real call sites, agent or user.)
 * - `search_results` — requires live search-result state (`$notes`/
 *   `tasks` already filtered by a prior query) that `show_view` has no way
 *   to populate from a bare navigation call; already served in chat by
 *   `search_notes`/`search_memory`. #3043 item 4 (Reed's finding): this
 *   screen is now ALSO removed from `USER_REACHABLE_VIEWS` entirely — the
 *   old chat-regex handler that used to set it was deleted pre-#3036, and
 *   the derived scan (`userReachableScan.ts`) confirms zero real call
 *   sites set it anywhere in `src/` today. It was never reachable via
 *   `show_view` either way, so this was dead weight in the reachable-list,
 *   not an exploitable gap — but the derived scan drops it automatically
 *   now, closing the WARN instead of just disclosing it.
 * - `task_list`, `dashboard` — REMOVED outright in #3044 item 5 (see
 *   `RENDERED_CANVAS_VIEWS`'s own comment above) — neither was ever a
 *   candidate for `show_view` (no user control opened either, same as
 *   #3043 had already found), and now neither exists as a view at all.
 *   All Tasks (`navigate_all_tasks`) is the real, user-reachable tasks
 *   screen — `task_list` was never it.
 */
export const SHOW_VIEW_SCREENS = [
  'notes_browser',
  'settings',
  'project_config',
] as const satisfies readonly (typeof USER_REACHABLE_VIEWS)[number][];

export interface CanvasState {
  view: CanvasViewType;
  data: Record<string, any>;  // View-specific data
  context: {                   // Current navigation context
    projectId?: string;
    projectName?: string;
    folderId?: string;
    folderName?: string;
    noteId?: string;
    noteTitle?: string;
    taskId?: string;
    searchQuery?: string;
    projectFilter?: string[];  // Filter project list to these names
    tagName?: string;          // Tag name for tag view
  };
}

// Create base store
const baseCanvasState = writable<CanvasState>({
  view: 'home',
  data: {},
  context: {}
});

// Wrap store to log all changes with stack trace
export const canvasState = {
  subscribe: baseCanvasState.subscribe,
  set: (value: CanvasState) => {
    const stack = new Error().stack?.split('\n').slice(2, 5).join('\n') || 'no stack';
    console.log('[NAV-DIAG] canvasState changed: view=' + value.view + ' projectId=' + (value.context.projectId || 'none') + ' at ' + Date.now());
    console.log('[NAV-DIAG] Stack trace:', stack);
    baseCanvasState.set(value);
  },
  update: (fn: (state: CanvasState) => CanvasState) => {
    baseCanvasState.update((state) => {
      const newState = fn(state);
      const stack = new Error().stack?.split('\n').slice(2, 5).join('\n') || 'no stack';
      console.log('[NAV-DIAG] canvasState changed: view=' + newState.view + ' projectId=' + (newState.context.projectId || 'none') + ' at ' + Date.now());
      console.log('[NAV-DIAG] Stack trace:', stack);
      return newState;
    });
  }
};