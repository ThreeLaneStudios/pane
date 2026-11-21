/**
 * Shared wiki-link click navigation logic — #2419, #3044.
 *
 * Both NotesBrowser (folder-view), KanbanBoard (kanban-view), and
 * AllTasksView (cross-project view) need the same open-note flow.
 *
 * Extracted here so all callers stay thin and the logic is unit-testable
 * without mounting Svelte components.
 *
 * #3044 ruling (Matt): a `[[Title]]` wiki link to a note that does NOT
 * exist stays inert — no create, no confirm dialog, nothing happens on
 * click. This applies identically whether the link was just typed or the
 * note is being reopened later; `Editor.svelte`'s render pass (turning
 * `[[Title]]` text into a `data-wiki-link` span) and the live
 * `WikiLink.ts` ProseMirror node both funnel every click through this one
 * function via `onNavigate`, so there is exactly one place this rule needs
 * to live. The previous "search → not found → confirm → create" path is
 * deleted outright, not merely gated — see wikiLinkNavigation.test.ts for
 * the inert-on-miss contract.
 */

import type { NoteMeta } from '$lib/types';

export interface WikiLinkNavOptions {
  /** Injected search function — callers pass api.searchNotes */
  searchFn: (query: string) => Promise<NoteMeta[]>;
  /**
   * Injected navigate function — callers pass noteService.selectNote.
   *
   * #2443 re-land (nav-lockup diagnosis §7, option 2 — single-writer): this
   * is the ONLY canvasState writer for the exact-match branch. selectNote()
   * accepts an optional context override (projectId/folderId) so the target
   * note's project/folder land in canvasState.context atomically with
   * view: 'note_editor' — no separate navigateFn call, no double write, no
   * flip-flop. See docs/audits/2026-08-05-nav-lockup-diagnosis.md §2/§7.
   */
  selectNoteFn: (note: NoteMeta, contextOverride?: { projectId?: string; folderId?: string }) => Promise<void>;
  /**
   * Injected raw-note-fetch function — callers pass
   * `(id) => api.getNoteRaw(id)`.
   *
   * #2438 defense-in-depth: `searchFn` results are not guaranteed to carry
   * `project_id`/`folder_id` (the backend gap that caused that incident —
   * see docs/audits/2026-08-05-2438-incident-diagnosis.md). If an exact
   * match arrives with a missing `project_id`, this is used to resolve the
   * real ids authoritatively before navigating, instead of silently falling
   * through to `''`.
   */
  getNoteRawFn: (noteId: string) => Promise<NoteMeta | null>;
}

/**
 * Navigate to the note matching `title`, if one exists.
 *
 * 1. Search for an exact-title match via `searchFn`.
 * 2. Found    → call `selectNoteFn(match, { projectId, folderId })` — single
 *    write to canvasState (view + context), no separate navigateFn call.
 * 3. Not found → do nothing. #3044 ruling: a missing note stays inert, no
 *    create-on-click, no confirm dialog.
 */
export async function navigateWikiLink(
  title: string,
  opts: WikiLinkNavOptions
): Promise<void> {
  const { searchFn, selectNoteFn, getNoteRawFn } = opts;

  const results = await searchFn(title);
  let exactMatch = results.find((note) => note.title === title);

  if (!exactMatch) {
    // #3044 ruling: missing note → inert. No create, no confirm, no navigate.
    return;
  }

  // #2438 defense-in-depth: search results are NOT guaranteed to carry
  // project_id/folder_id (that incident's root cause — search_notes'
  // backend row was missing both columns entirely; see
  // docs/audits/2026-08-05-2438-incident-diagnosis.md). If the backend
  // ever regresses/omits these fields again, resolve them authoritatively
  // via getNoteRawFn (same donor pattern as navigation.ts:57) instead of
  // silently falling through to '' — the console.warn is what makes this
  // visible; a silent `?? ''` swallow is what hid the original incident.
  if (!exactMatch.project_id) {
    console.warn(`[navigateWikiLink] Search result missing project_id for note ${exactMatch.id} — resolving via getNoteRaw fallback`);
    const fullNote = await getNoteRawFn(exactMatch.id);
    if (fullNote) {
      exactMatch = { ...exactMatch, project_id: fullNote.project_id, folder_id: fullNote.folder_id };
    }
  }

  // #2443 re-land (nav-lockup diagnosis §7, option 2 — single-writer):
  // Previously this called a separate `navigateFn({ view: 'notes_browser',
  // ... })` here, immediately before `selectNoteFn`, which unconditionally
  // overwrites canvasState.view to 'note_editor' ~20-25ms later
  // (noteService.ts's DB-flush setTimeout). Every wiki-link click produced
  // two opposite canvasState.view writes in quick succession — a confirmed
  // flip-flop that made every navigation snap back (nav lockup, v0.68.93).
  //
  // Fix: pass the target note's project_id/folder_id straight into
  // selectNoteFn's own canvasState.update() call as a context override, so
  // there is exactly ONE canvasState write for this whole click — view AND
  // context land together, atomically, with no race.
  await selectNoteFn(exactMatch, {
    projectId: exactMatch.project_id || undefined,
    folderId: exactMatch.folder_id || undefined,
  });
}
