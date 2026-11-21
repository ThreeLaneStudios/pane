/**
 * #2335 / #2370 — CanvasRenderer note_editor reload-guard (corrected, v2)
 *
 * History:
 *   - #2380 shipped a guard comparing `context.noteId` against a CanvasRenderer-local
 *     `lastLoadedNoteId` bookkeeping var. Reed's audit (docs/audits/2380-delta-audit.md)
 *     found — and empirically verified — that this comparison NEVER matches for a genuine
 *     different-note card click, because `lastLoadedNoteId` only updates once
 *     CanvasRenderer's OWN async `loadDataForView` resolves, one full cycle behind. #2380
 *     was reverted (see `1f4593b1`, PR #2384).
 *   - This file replaces #2380's test suite with tests against the corrected guard, which
 *     compares `context.noteId` against `$activeNote`'s CURRENT id instead.
 *
 * Root cause (confirmed in Reed's audit §1b): `noteService.selectNote()` does, in order,
 * on every card click:
 *   1. `activeNote.set(enriched)`       — noteService.ts:91 — $activeNote NOW reflects the
 *                                          just-clicked note.
 *   2. `canvasState.update(...)`        — noteService.ts:95 — fires CanvasRenderer's
 *                                          `$effect` via reference churn (view stays
 *                                          'note_editor'; only context.noteId/noteTitle are
 *                                          refreshed, per FIX #619).
 * By the time step 2's echo reaches CanvasRenderer's `$effect`, `$activeNote` already equals
 * `context.noteId` — the load is already done (selectNote() did its own full fetch); nothing
 * new needs fetching. This is true for EVERY card-to-card click via selectNote(), not just
 * same-note re-selection — which is exactly why #2380's guard (only catching the same-note
 * case) didn't fix the reported bounce.
 *
 * Corrected guard: skip `loadDataForView('note_editor', ...)` entirely whenever
 * `context.noteId === $activeNote?.meta?.id` AND `context.projectId` matches the last
 * successfully-loaded project. `lastLoadedNoteId` is gone — `$activeNote`'s own id is the
 * authoritative, already-correct reference it used to (incorrectly) approximate.
 * `lastLoadedProjectId` is kept: project membership isn't tracked by `$activeNote`, so
 * cross-project detection still needs it, and it's also what makes "view away and back"
 * force a real reload (see reset condition #2 below).
 *
 * Harness note: faithful line-for-line mirror of the `$effect` guard and the `note_editor`
 * branch of `loadDataForView`, exactly as fixed in CanvasRenderer.svelte (same house pattern
 * as NotesBrowser.collapse-rails.test.ts — CanvasRenderer.svelte requires full Tauri window
 * context and heavy child components, so this extracts the guard/branch logic verbatim into
 * a standalone, directly-testable form).
 */

import { describe, it, expect, vi, beforeEach } from 'vitest';

/**
 * Mirrors CanvasRenderer.svelte's:
 *   - `let lastLoadedProjectId: string | null = $state(null);`
 *   - the `$effect` guard ahead of `loadDataForView(state.view, state.context)`, comparing
 *     `context.noteId` against `get(activeNote)?.meta?.id` (not a local bookkeeping var)
 *   - the `note_editor` case of `loadDataForView` (success/failure paths that set/clear
 *     `lastLoadedProjectId`, and set `$activeNote` on success)
 *
 * `fetchNote` stands in for `noteService.loadNoteWithJson`.
 */
function makeCanvasRendererGuard(fetchNote: (projectName: string, noteSlug: string) => Promise<any | null>) {
  let lastLoadedProjectId: string | null = null;
  let lastLoadedNoteId: string | null = null;
  let isLoading = false;
  let activeNote: any = null;
  let loadDataForViewCallCount = 0;
  // Every loadDataForView call sets isLoading = true as its first statement, which (per the
  // real component's un-excluded loading gate) unmounts/remounts <NotesBrowser/>. Counting
  // calls IS counting unmount/remount cycles for the note_editor path.
  let unmountRemountCount = 0;
  // #2443 re-land §7 fix 3: separate counter for note_editor-branch invocations specifically.
  // A 'notes_browser' transient pass-through genuinely DOES call loadDataForView('notes_browser',
  // ...) in the real component (it loads project/folder browser data — a necessary call, not a
  // redundant one). The bug this fix closes is specifically a REDUNDANT note_editor-branch
  // fetch on return; total loadDataForViewCallCount conflates the two, so tests exercising the
  // fix use this counter instead.
  let noteEditorLoadCount = 0;

  async function loadDataForView(view: string, context: any) {
    loadDataForViewCallCount++;
    isLoading = true;
    unmountRemountCount++;
    try {
      switch (view) {
        case 'note_editor':
          noteEditorLoadCount++;
          if (context.noteId && context.projectId) {
            const noteSlug = context.noteId;
            const projectName = context.projectName || 'Notes';
            try {
              const noteWithJson = await fetchNote(projectName, noteSlug);
              if (noteWithJson) {
                activeNote = noteWithJson;
                lastLoadedProjectId = context.projectId ?? null;
                lastLoadedNoteId = context.noteId ?? null;
              } else {
                lastLoadedProjectId = null;
                lastLoadedNoteId = null;
              }
            } catch (error) {
              lastLoadedProjectId = null;
              lastLoadedNoteId = null;
            }
          }
          break;
        default:
          // Other views (project_list, notes_browser, etc.) are irrelevant to this guard —
          // exercised elsewhere. Simulate a generic non-note_editor load.
          break;
      }
    } finally {
      isLoading = false;
    }
  }

  // Mirrors the `$effect` body exactly (minus the `untrack()` wrapper, a Svelte reactivity
  // primitive with no bearing on the guard logic itself). #2443 re-land (nav-lockup diagnosis
  // §7 fix 3): the reset-on-leaving-note_editor branch no longer unconditionally nulls the
  // guard — it only does so when the departing transition's context doesn't still point at
  // the already-loaded note or project, so a transient notes_browser pass-through immediately
  // followed by a note_editor return to the SAME note/project doesn't force a redundant load.
  async function runEffect(state: { view: string; context: any }) {
    if (
      state.view === 'note_editor' &&
      state.context?.noteId &&
      state.context.noteId === activeNote?.meta?.id &&
      (state.context.projectId ?? null) === lastLoadedProjectId
    ) {
      return; // skip entirely — $activeNote already reflects this exact note
    }
    if (state.view !== 'note_editor') {
      const departingContext = state.context ?? {};
      const stillSameNote = lastLoadedNoteId !== null && departingContext.noteId === lastLoadedNoteId;
      const stillSameProject = lastLoadedProjectId !== null && (departingContext.projectId ?? null) === lastLoadedProjectId;
      if (!stillSameNote && !stillSameProject) {
        lastLoadedProjectId = null;
        lastLoadedNoteId = null;
      }
    }
    await loadDataForView(state.view, state.context);
  }

  // Simulates noteService.selectNote()'s exact ordering (noteService.ts:91,95): set
  // $activeNote to the fully-enriched note FIRST, THEN fire the canvasState.update() echo
  // that reaches CanvasRenderer's $effect. This is what every real card-to-card click does.
  function simulateCardClick(noteId: string, projectId: string) {
    activeNote = { meta: { id: noteId, slug: noteId, title: `Note ${noteId}` }, content: '', jsonContent: null };
    return runEffect({
      view: 'note_editor',
      context: { noteId, projectId, noteTitle: `Note ${noteId}` },
    });
  }

  return {
    runEffect,
    simulateCardClick,
    getState: () => ({ lastLoadedProjectId, lastLoadedNoteId, isLoading, activeNote }),
    getLoadDataForViewCallCount: () => loadDataForViewCallCount,
    getNoteEditorLoadCount: () => noteEditorLoadCount,
    getUnmountRemountCount: () => unmountRemountCount,
    isLoadingNow: () => isLoading,
  };
}

describe('CanvasRenderer note_editor reload guard — #2335 / #2370 (corrected, v2)', () => {
  let fetchNote: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    fetchNote = vi.fn(async (_projectName: string, noteSlug: string) => ({
      meta: { id: noteSlug, slug: noteSlug, title: `Note ${noteSlug}` },
      content: '',
      jsonContent: null,
    }));
  });

  // ── RED-ON-REVERT (mandatory): the actual #2335 repro — card-to-card, DIFFERENT notes ──
  //
  // The very FIRST transition into 'note_editor' (from 'notes_browser'/'folder' view) is a
  // genuine, unavoidable load — the guard's project-tuple half correctly requires it (reset
  // condition #4/#5). That first open is NOT part of the reported bounce; the bug is about
  // every SUBSEQUENT card-to-card click while the editor pane is already showing. So these
  // tests prime an initial note session (asserting it DOES load, as it must), then assert
  // the three genuinely-reported repro clicks (A → B → C) add ZERO further loads.
  describe('#2335 literal repro: click note-A, then note-B, then note-C (three different notes)', () => {
    it('ZERO loadDataForView calls, ZERO isLoading flips, ZERO unmount/remount cycles across all three card-to-card transitions', async () => {
      const guard = makeCanvasRendererGuard(fetchNote);

      // Prime: open the very first note from the folder view. This is a genuine transition
      // into note_editor and MUST load — not part of what we're asserting zero on.
      await guard.simulateCardClick('note-0', 'proj-1');
      expect(guard.getLoadDataForViewCallCount()).toBe(1);
      const primedCalls = guard.getLoadDataForViewCallCount();
      const primedFetches = fetchNote.mock.calls.length;

      // Each simulateCardClick mirrors noteService.selectNote(): $activeNote is set to the
      // fully-loaded note BEFORE the canvasState.update() echo reaches CanvasRenderer. Since
      // selectNote() already did the full fetch, CanvasRenderer's own load is redundant for
      // every one of these subsequent card-to-card clicks — not just same-note re-selection.
      await guard.simulateCardClick('note-A', 'proj-1');
      await guard.simulateCardClick('note-B', 'proj-1');
      await guard.simulateCardClick('note-C', 'proj-1');

      expect(guard.getLoadDataForViewCallCount()).toBe(primedCalls); // 0 additional calls
      expect(guard.getUnmountRemountCount()).toBe(primedCalls); // 0 additional unmount/remount cycles
      expect(fetchNote.mock.calls.length).toBe(primedFetches); // 0 additional fetches
      expect(guard.getState().isLoading).toBe(false);
      expect(guard.getState().activeNote?.meta?.id).toBe('note-C');
    });
  });

  describe('same-note re-selection via card click — also skipped (subset of the above)', () => {
    it('re-clicking the currently-open note fires zero additional loads', async () => {
      const guard = makeCanvasRendererGuard(fetchNote);
      await guard.simulateCardClick('note-A', 'proj-1'); // primes the first genuine load
      const primedCalls = guard.getLoadDataForViewCallCount();
      expect(primedCalls).toBe(1);

      await guard.simulateCardClick('note-A', 'proj-1');
      await guard.simulateCardClick('note-A', 'proj-1');

      expect(guard.getLoadDataForViewCallCount()).toBe(primedCalls); // 0 additional
      expect(fetchNote).toHaveBeenCalledTimes(1); // only the priming fetch
    });
  });

  // ── Guard-safety: genuine transitions NOT preceded by an activeNote.set() must still reload ──
  // These model direct `navigateTo({ view: 'note_editor', noteId })` call sites that do NOT
  // go through noteService.selectNote() first (e.g. ProjectSettings.svelte:482,
  // DueTodayNodeView.svelte:211, NavPlayer.svelte:188) — $activeNote is NOT already synced,
  // so the guard must correctly fall through to a real load.
  describe('guard-safety — genuine transitions (not preceded by activeNote sync) always reload', () => {
    it('1. different noteId reaching the effect directly (activeNote not yet synced) → loadDataForView IS called', async () => {
      const guard = makeCanvasRendererGuard(fetchNote);
      await guard.runEffect({ view: 'note_editor', context: { noteId: 'note-A', projectId: 'proj-1' } });
      await guard.runEffect({ view: 'note_editor', context: { noteId: 'note-B', projectId: 'proj-1' } });

      expect(guard.getLoadDataForViewCallCount()).toBe(2);
      expect(fetchNote).toHaveBeenCalledTimes(2);
      expect(guard.getState().activeNote?.meta?.id).toBe('note-B');
    });

    it('2. view change away from note_editor and back → loadDataForView IS called on return', async () => {
      const guard = makeCanvasRendererGuard(fetchNote);
      // Genuine direct load of note-A (activeNote not pre-synced).
      await guard.runEffect({ view: 'note_editor', context: { noteId: 'note-A', projectId: 'proj-1' } });
      expect(guard.getLoadDataForViewCallCount()).toBe(1);
      expect(guard.getState().activeNote?.meta?.id).toBe('note-A');

      // Navigate away (e.g. All Tasks) — guard must clear lastLoadedProjectId.
      await guard.runEffect({ view: 'all_tasks', context: {} });
      expect(guard.getState().lastLoadedProjectId).toBeNull();

      // Return to the SAME note. Even though $activeNote still equals 'note-A' (never
      // cleared), lastLoadedProjectId is null and context.projectId is 'proj-1' — the guard
      // fails on the project-tuple half, forcing a genuine reload. This is why
      // lastLoadedProjectId must be kept even after switching the noteId comparison to
      // $activeNote.
      await guard.runEffect({ view: 'note_editor', context: { noteId: 'note-A', projectId: 'proj-1' } });
      expect(guard.getLoadDataForViewCallCount()).toBe(3); // note-A, all_tasks, note-A again
      expect(fetchNote).toHaveBeenCalledTimes(2); // note-A fetched twice total
    });

    it('3. same noteId after project switch → loadDataForView IS called (projectId differs)', async () => {
      const guard = makeCanvasRendererGuard(fetchNote);
      await guard.runEffect({ view: 'note_editor', context: { noteId: 'note-A', projectId: 'proj-1' } });
      await guard.runEffect({ view: 'note_editor', context: { noteId: 'note-A', projectId: 'proj-2' } });

      expect(guard.getLoadDataForViewCallCount()).toBe(2);
      expect(fetchNote).toHaveBeenCalledTimes(2);
      expect(guard.getState().lastLoadedProjectId).toBe('proj-2');
    });

    it('4. first load of a note from a non-note_editor view → loadDataForView IS called', async () => {
      const guard = makeCanvasRendererGuard(fetchNote);
      // Fresh guard: activeNote is null, so nothing can match.
      await guard.runEffect({ view: 'note_editor', context: { noteId: 'note-A', projectId: 'proj-1' } });

      expect(guard.getLoadDataForViewCallCount()).toBe(1);
      expect(fetchNote).toHaveBeenCalledTimes(1);
    });

    it('5. view change (all_tasks → note_editor) with a note never before loaded → IS called', async () => {
      const guard = makeCanvasRendererGuard(fetchNote);
      await guard.runEffect({ view: 'all_tasks', context: {} });
      await guard.runEffect({ view: 'note_editor', context: { noteId: 'note-C', projectId: 'proj-1' } });

      expect(guard.getLoadDataForViewCallCount()).toBe(2);
      expect(fetchNote).toHaveBeenCalledTimes(1);
    });
  });

  // ── #2443 re-land — fix 3: transient notes_browser pass-through no longer forces a
  // redundant reload (nav-lockup diagnosis §2.3 / §7 fix 3) ──────────────────────────
  //
  // Root cause this closes: the old unconditional `if (state.view !== 'note_editor')
  // lastLoadedProjectId = null;` reset fired on EVERY transient pass through 'notes_browser'
  // — including the wiki-link double-write's own `navigateFn({ view: 'notes_browser', ... })`
  // call, which always preceded selectNote()'s 'note_editor' write-back to the SAME note/
  // project. That reset always won the race, so by the time the note_editor echo arrived,
  // the guard had already been cleared — forcing a second, redundant loadDataForView call on
  // top of the load selectNote() itself already did. Confirmed in the diagnosis (§2.3): every
  // wiki-link click produced two independent note loads, not one.
  //
  // This is a real, separately-triggerable defect (not just wiki-link-specific) — ANY
  // notes_browser→note_editor transition to the same note re-triggers a redundant load. These
  // tests pin the fix directly, independent of whether the wiki-link single-writer change
  // (wikiLinkNavigation.ts) also lands — reverting *this* guard fix alone must flip them red.
  describe('#2443 re-land — fix 3: transient notes_browser pass-through does not force a redundant reload', () => {
    it('notes_browser pass-through to the SAME note/project, immediately followed by note_editor → zero additional loads', async () => {
      const guard = makeCanvasRendererGuard(fetchNote);

      // Genuine first load of note-A in proj-1.
      await guard.runEffect({ view: 'note_editor', context: { noteId: 'note-A', projectId: 'proj-1' } });
      expect(guard.getNoteEditorLoadCount()).toBe(1);
      expect(fetchNote).toHaveBeenCalledTimes(1);

      // Transient pass-through: view flips to 'notes_browser' carrying the SAME noteId/
      // projectId (this is exactly what the old navigateFn call did), then immediately back
      // to 'note_editor' for the same note — mirroring selectNote()'s canvasState.update()
      // echo a beat later. $activeNote already reflects note-A (selectNote() sets it first).
      // The 'notes_browser' leg itself DOES call loadDataForView (it's a genuine view with its
      // own data to load in the real component) — what must NOT happen is a second, redundant
      // note_editor-branch fetch when we land back on the SAME note/project a beat later.
      await guard.runEffect({ view: 'notes_browser', context: { noteId: 'note-A', projectId: 'proj-1' } });
      await guard.runEffect({ view: 'note_editor', context: { noteId: 'note-A', projectId: 'proj-1' } });

      // Red-on-revert: reverting fix 3 (restoring the unconditional reset) makes the
      // 'notes_browser' pass-through null the guard, so the final note_editor re-entry's
      // project-tuple half fails and a redundant note fetch fires — this count would be 2, not 1.
      expect(guard.getNoteEditorLoadCount()).toBe(1);
      expect(fetchNote).toHaveBeenCalledTimes(1);
    });

    it('notes_browser pass-through carrying a DIFFERENT project still correctly invalidates the guard', async () => {
      const guard = makeCanvasRendererGuard(fetchNote);

      await guard.runEffect({ view: 'note_editor', context: { noteId: 'note-A', projectId: 'proj-1' } });
      expect(guard.getLoadDataForViewCallCount()).toBe(1);

      // Genuine departure: notes_browser targets a DIFFERENT project than what's loaded.
      await guard.runEffect({ view: 'notes_browser', context: { noteId: undefined, projectId: 'proj-2' } });
      expect(guard.getState().lastLoadedProjectId).toBeNull();
      expect(guard.getState().lastLoadedNoteId).toBeNull();

      // Returning to note-A in proj-1 must reload for real — the guard was correctly cleared.
      await guard.runEffect({ view: 'note_editor', context: { noteId: 'note-A', projectId: 'proj-1' } });
      expect(guard.getLoadDataForViewCallCount()).toBe(3); // note-A, notes_browser(other proj), note-A again
      expect(fetchNote).toHaveBeenCalledTimes(2);
    });

    it('notes_browser pass-through with no note/project context at all still invalidates the guard (safe default)', async () => {
      const guard = makeCanvasRendererGuard(fetchNote);

      await guard.runEffect({ view: 'note_editor', context: { noteId: 'note-A', projectId: 'proj-1' } });
      expect(guard.getLoadDataForViewCallCount()).toBe(1);

      await guard.runEffect({ view: 'notes_browser', context: {} });
      expect(guard.getState().lastLoadedProjectId).toBeNull();
      expect(guard.getState().lastLoadedNoteId).toBeNull();

      await guard.runEffect({ view: 'note_editor', context: { noteId: 'note-A', projectId: 'proj-1' } });
      expect(guard.getLoadDataForViewCallCount()).toBe(3);
    });
  });

  // ── #2379 audit §5.3 — new coverage: all_tasks → wiki-link → note_editor ───────────────
  //
  // Adding wiki-link navigation to AllTasksView (audit §4.4) is the one change in the
  // #2379 parity effort that newly exposes ATV to this reload-guard machinery — ATV's own
  // card-click path (handleCardClick) deliberately never sets canvasState.view, but a
  // wiki-link click DOES go through navigateWikiLink's selectNoteFn (noteService.selectNote),
  // which sets canvasState.view = 'note_editor'. This mirrors Kanban's existing, accepted
  // wiki-link-navigates-away-from-the-board behavior (KanbanBoard.svelte:538-556, no
  // "stay in kanban" special-casing) — All Tasks gets the same behavior, no special-casing.
  describe('#2379 audit §5.3 — all_tasks → wiki-link → note_editor transition', () => {
    it('a genuine transition from all_tasks into note_editor (via a wiki-link click) loads the target note', async () => {
      const guard = makeCanvasRendererGuard(fetchNote);

      // Start on All Tasks — no note loaded yet, guard is at its fresh-mount defaults.
      await guard.runEffect({ view: 'all_tasks', context: {} });
      expect(guard.getState().lastLoadedProjectId).toBeNull();
      const afterAllTasksCalls = guard.getLoadDataForViewCallCount();

      // Wiki-link click inside a task opened from All Tasks resolves via navigateWikiLink →
      // selectNoteFn, which (like every other selectNote-driven transition) sets $activeNote
      // BEFORE the canvasState.update() echo reaches this effect — mirrored here the same way
      // simulateCardClick does for the direct-card-click case. Because the preceding all_tasks
      // leg carried no noteId/projectId, the leaving-view reset nulls lastLoadedProjectId, so
      // the project-tuple half of the skip-check correctly fails and a genuine load occurs —
      // this is NOT the #2443 notes_browser-passthrough redundant-load case (that fix is
      // specifically for a passthrough carrying the SAME note/project); this is a normal,
      // required first load of a note that was never in the guard before.
      await guard.simulateCardClick('wiki-target-note', 'proj-wiki');

      expect(guard.getLoadDataForViewCallCount()).toBe(afterAllTasksCalls + 1); // one genuine note_editor load
      expect(fetchNote).toHaveBeenCalledTimes(1);
      expect(guard.getState().activeNote?.meta?.id).toBe('wiki-target-note');
      expect(guard.getState().lastLoadedProjectId).toBe('proj-wiki');
    });

    it('a DIRECT note_editor transition (not preceded by activeNote sync) from all_tasks still triggers a real load', async () => {
      const guard = makeCanvasRendererGuard(fetchNote);

      await guard.runEffect({ view: 'all_tasks', context: {} });
      await guard.runEffect({ view: 'note_editor', context: { noteId: 'wiki-target-note-2', projectId: 'proj-wiki' } });

      expect(guard.getLoadDataForViewCallCount()).toBe(2); // all_tasks, then the genuine note_editor load
      expect(fetchNote).toHaveBeenCalledTimes(1);
      expect(guard.getState().activeNote?.meta?.id).toBe('wiki-target-note-2');
    });
  });

  // ── Failure/edge-case handling for the guard's own bookkeeping ───────────────────────────
  describe('guard bookkeeping on failed/empty loads', () => {
    it('a failed fetch does not poison the guard — retrying the same note reloads instead of being skipped', async () => {
      const guard = makeCanvasRendererGuard(fetchNote);
      fetchNote.mockRejectedValueOnce(new Error('DB locked'));

      await guard.runEffect({ view: 'note_editor', context: { noteId: 'note-A', projectId: 'proj-1' } });
      expect(guard.getState().lastLoadedProjectId).toBeNull(); // failure must NOT set the guard
      expect(guard.getState().activeNote).toBeNull(); // activeNote never got set either

      // Re-click the same card — must NOT be skipped, since the guard was never set and
      // activeNote still doesn't match.
      await guard.runEffect({ view: 'note_editor', context: { noteId: 'note-A', projectId: 'proj-1' } });
      expect(guard.getLoadDataForViewCallCount()).toBe(2);
      expect(fetchNote).toHaveBeenCalledTimes(2);
      expect(guard.getState().lastLoadedProjectId).toBe('proj-1'); // second attempt succeeded
    });

    it('a null/empty fetch result does not poison the guard either', async () => {
      const guard = makeCanvasRendererGuard(fetchNote);
      fetchNote.mockResolvedValueOnce(null);

      await guard.runEffect({ view: 'note_editor', context: { noteId: 'note-A', projectId: 'proj-1' } });
      expect(guard.getState().lastLoadedProjectId).toBeNull();

      await guard.runEffect({ view: 'note_editor', context: { noteId: 'note-A', projectId: 'proj-1' } });
      expect(guard.getLoadDataForViewCallCount()).toBe(2);
      expect(guard.getState().lastLoadedProjectId).toBe('proj-1');
    });

    it('missing noteId/projectId in context never sets or matches the guard', async () => {
      const guard = makeCanvasRendererGuard(fetchNote);
      await guard.runEffect({ view: 'note_editor', context: {} });
      expect(guard.getLoadDataForViewCallCount()).toBe(1); // falls through the `if` in loadDataForView, no fetch
      expect(fetchNote).not.toHaveBeenCalled();
      expect(guard.getState().lastLoadedProjectId).toBeNull();
    });
  });
});
