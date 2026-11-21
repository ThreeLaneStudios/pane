/**
 * navigationDispatch.ts — #3036 PR2
 * (`docs/audits/2026-10-16-scope-agent-instructions.md` §6, folding in
 * #3022)
 *
 * Pure decision logic for `ChatSidebar.svelte`'s `handleNavigateFromStream`.
 * Extracted so the component AND the test suite call the SAME
 * implementation (the #2995 lesson — a test that mirrors a component's
 * logic by hand proves the mirror agrees with itself, not that it agrees
 * with the real code).
 *
 * The bug this replaces (§6 of the scope doc): the old inline version was
 * `if (nav.view && !nav.note_id) { switch(nav.view) {...}; return; }` — an
 * UNCONDITIONAL `return` after the switch, regardless of whether a case
 * matched. Since the backend defaulted bare project navigation's `view` to
 * the long-removed `'folder_detail'`, and `show_view`'s own result used
 * `'notes_browser'`, NEITHER ever matched a switch case, and BOTH were
 * silently dropped before ever reaching the one branch below them that
 * would have navigated correctly. `decideNavigationAction` fixes this by
 * construction: there is no bare `return` on no match anywhere in this
 * function — every path either names a concrete action or names an error.
 *
 * Deliberately pure/synchronous. The actual side effects (`api.
 * listProjects()`, `navigateTo()`, DOM `CustomEvent`s) stay in
 * `ChatSidebar.svelte`, which is why `open_project`/`open_note` only report
 * the id — "does this project/note id actually resolve" is an async
 * lookup the caller performs and which can itself produce a visible error
 * (project/note not found) distinct from the ones decided here (bad or
 * missing navigation target shape).
 *
 * #3043 round 2 (Matt's ruling overriding Reed's delta-2 "fallthrough is
 * fine" call): the backend's `show_view` handler
 * (`build_show_view_navigate_action`, commands.rs) still REJECTS an
 * unrecognized/no-longer-offered view (defense in depth, unchanged) —
 * but now signals that rejection with an `error` field on the SAME
 * `navigate` payload shape, instead of silently substituting
 * `"notes_browser"`. `nav.error` is checked FIRST, before note_id/
 * project_id/view, so a rejected show_view call can never be
 * misinterpreted as a navigation request alongside it.
 */

import { RENDERED_CANVAS_VIEWS } from '$lib/stores/canvas';
import type { CanvasViewType } from '$lib/stores/canvas';

/** The raw `navigate` payload streamed back from the backend. */
export interface NavigateStreamPayload {
  view?: string;
  tab?: string;
  note_id?: string;
  project_id?: string;
  project_name?: string;
  folder_id?: string;
  folder_name?: string;
  /**
   * #3043 round 2 — set by the backend ONLY when it rejected an
   * unrecognized/disallowed `view` (e.g. `show_view("task_list")`).
   * When present, this is the whole story: no other field on this
   * payload names a real navigation target, and `decideNavigationAction`
   * must return `{ kind: 'error' }` without inspecting anything else.
   */
  error?: string;
}

export type NavigationDecision =
  | { kind: 'open_note'; projectId: string; noteId: string; folderId?: string }
  | { kind: 'open_project'; projectId: string; view: CanvasViewType }
  | { kind: 'open_view'; view: CanvasViewType }
  | { kind: 'toggle_kanban' }
  | { kind: 'set_filter'; tab?: string }
  | { kind: 'error'; message: string };

function isRenderedView(view: string): view is CanvasViewType {
  return (RENDERED_CANVAS_VIEWS as readonly string[]).includes(view);
}

/**
 * Decide what `handleNavigateFromStream` should do with a `navigate`
 * stream payload. Never returns silently — every input produces either a
 * concrete navigation action or an `{ kind: 'error' }` the caller must
 * surface visibly (#3036 PR2 item 4).
 */
export function decideNavigationAction(nav: NavigateStreamPayload): NavigationDecision {
  // 0. The backend already rejected this request (#3043 round 2 — an
  // unrecognized/disallowed show_view target). Must win over every other
  // check below: a rejected request never also names a usable target on
  // the same payload, and even if it did, the rejection is the truth
  // that must reach the user, not a navigation that silently proceeds
  // anyway.
  if (nav.error) {
    return { kind: 'error', message: nav.error };
  }

  // 1. Opening a specific note always wins — a note_id is the most
  // specific target this payload can name, regardless of what `view` (if
  // any) rode alongside it.
  if (nav.note_id) {
    if (!nav.project_id) {
      return {
        kind: 'error',
        message: `Could not open note "${nav.note_id}": no project context was provided.`,
      };
    }
    return {
      kind: 'open_note',
      projectId: nav.project_id,
      noteId: nav.note_id,
      folderId: nav.folder_id,
    };
  }

  // 2. Bare project navigation (#3036 PR2's headline fix) — opens that
  // project's board. Defaults to 'notes_browser' when the backend sent no
  // `view` (the common case: `navigate({project_id})`), or honors a real
  // show_view screen (e.g. 'project_config') sent alongside the project id.
  if (nav.project_id) {
    const view = nav.view || 'notes_browser';
    if (!isRenderedView(view)) {
      return {
        kind: 'error',
        message: `Navigation failed: "${view}" is not a recognized view.`,
      };
    }
    return { kind: 'open_project', projectId: nav.project_id, view };
  }

  // 3. A bare, project-free screen switch — navigate_home/_all_tasks/etc.,
  // or show_view's one remaining project-free screen ('settings' —
  // #3043 removed 'task_list'/'dashboard' from what show_view can send;
  // this `isRenderedView` check still accepts them structurally since
  // they're valid CanvasViewType render branches, but no caller sends them
  // anymore).
  if (nav.view) {
    if (nav.view === 'kanban') {
      return { kind: 'toggle_kanban' };
    }
    if (nav.view === 'set_filter') {
      return { kind: 'set_filter', tab: nav.tab };
    }
    if (!isRenderedView(nav.view)) {
      return {
        kind: 'error',
        message: `Navigation failed: "${nav.view}" is not a recognized view.`,
      };
    }
    return { kind: 'open_view', view: nav.view };
  }

  // 4. Nothing to navigate to at all (#3036 PR2 item 4) — must not be
  // silent either.
  return {
    kind: 'error',
    message: 'Navigation failed: no target (note, project, or view) was provided.',
  };
}
