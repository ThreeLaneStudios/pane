/**
 * navigationDispatch.test.ts — #3036 PR2
 * (`docs/audits/2026-10-16-scope-agent-instructions.md` §6, folding in
 * #3022)
 *
 * Tests the REAL `decideNavigationAction` imported from
 * `navigationDispatch.ts` — the same function `ChatSidebar.svelte`'s
 * `handleNavigateFromStream` calls — never a hand-mirrored copy of its
 * logic (the #2995 lesson).
 *
 * Each "before" comment below describes what the OLD inline
 * `ChatSidebar.svelte` code actually did, confirmed by reading it at this
 * PR's base commit, not inferred.
 */
import { describe, it, expect } from 'vitest';
import { decideNavigationAction } from './navigationDispatch';

describe('decideNavigationAction — #3036 PR2', () => {
  describe('#3043 round 2 — backend-rejected show_view target (nav.error) wins over everything else', () => {
    it('a bare nav.error is a visible error carrying the backend\'s exact message', () => {
      const decision = decideNavigationAction({
        error: 'The agent tried to show an unrecognized screen ("task_list"). This isn\'t a view show_view can open.',
      });
      expect(decision).toEqual({
        kind: 'error',
        message: 'The agent tried to show an unrecognized screen ("task_list"). This isn\'t a view show_view can open.',
      });
    });

    it('nav.error wins even when project_id is ALSO present on the same payload — must not open_project instead of erroring (the exact silent-redirect shape #3043 round 2 forbids)', () => {
      const decision = decideNavigationAction({
        error: 'The agent tried to show an unrecognized screen ("dashboard"). This isn\'t a view show_view can open.',
        project_id: 'proj-123',
      });
      expect(decision.kind).toBe('error');
    });

    it('nav.error wins even when a (dead/unused) view field is ALSO present — the screen must never change', () => {
      const decision = decideNavigationAction({
        error: 'The agent tried to show an unrecognized screen ("task_list"). This isn\'t a view show_view can open.',
        view: 'notes_browser', // backend no longer sends this alongside error, but decision logic must not trust it if it did
      });
      expect(decision.kind).toBe('error');
    });

    it('nav.error wins even over note_id (the highest-priority target otherwise)', () => {
      const decision = decideNavigationAction({
        error: 'The agent tried to show an unrecognized screen ("bogus"). This isn\'t a view show_view can open.',
        note_id: 'note-1',
        project_id: 'proj-1',
      });
      expect(decision.kind).toBe('error');
    });
  });

  describe('bare project navigation (the headline bug fix)', () => {
    it('project_id with no view/note_id opens that project\'s board (notes_browser) — BEFORE: the backend\'s stale "folder_detail" default made this unreachable, swallowed by the view-only switch', () => {
      const decision = decideNavigationAction({ project_id: 'proj-123' });
      expect(decision).toEqual({ kind: 'open_project', projectId: 'proj-123', view: 'notes_browser' });
    });

    it('project_id with the backend\'s now-fixed "notes_browser" view produces the same decision explicitly', () => {
      const decision = decideNavigationAction({ project_id: 'proj-123', view: 'notes_browser' });
      expect(decision).toEqual({ kind: 'open_project', projectId: 'proj-123', view: 'notes_browser' });
    });

    it('project_id with a show_view project-scoped screen (project_config) honors that view instead of defaulting', () => {
      const decision = decideNavigationAction({ project_id: 'proj-123', view: 'project_config' });
      expect(decision).toEqual({ kind: 'open_project', projectId: 'proj-123', view: 'project_config' });
    });

    it('a dead/removed view name alongside project_id (the OLD stale default, "folder_detail") is now a visible error, not silence', () => {
      const decision = decideNavigationAction({ project_id: 'proj-123', view: 'folder_detail' });
      expect(decision.kind).toBe('error');
      expect((decision as any).message).toContain('folder_detail');
    });
  });

  describe('note navigation', () => {
    it('note_id + project_id opens the note', () => {
      const decision = decideNavigationAction({ note_id: 'note-1', project_id: 'proj-1' });
      expect(decision).toEqual({ kind: 'open_note', projectId: 'proj-1', noteId: 'note-1', folderId: undefined });
    });

    it('note_id + project_id + folder_id carries the folder through', () => {
      const decision = decideNavigationAction({ note_id: 'note-1', project_id: 'proj-1', folder_id: 'folder-1' });
      expect(decision).toEqual({ kind: 'open_note', projectId: 'proj-1', noteId: 'note-1', folderId: 'folder-1' });
    });

    it('note_id always wins over any view sent alongside it', () => {
      const decision = decideNavigationAction({ note_id: 'note-1', project_id: 'proj-1', view: 'kanban' });
      expect(decision.kind).toBe('open_note');
    });

    it('note_id with NO project_id is a visible error (the old code did nothing at all in this shape — no branch matched)', () => {
      const decision = decideNavigationAction({ note_id: 'note-1' });
      expect(decision.kind).toBe('error');
      expect((decision as any).message).toContain('note-1');
    });
  });

  describe('bare, project-free screen switches', () => {
    for (const view of ['home', 'project_list', 'all_tasks', 'trash', 'calendar'] as const) {
      it(`view='${view}' with no project/note context opens that view`, () => {
        const decision = decideNavigationAction({ view });
        expect(decision).toEqual({ kind: 'open_view', view });
      });
    }

    it("show_view's settings screen opens correctly", () => {
      expect(decideNavigationAction({ view: 'settings' })).toEqual({ kind: 'open_view', view: 'settings' });
    });

    it("'task_list'/'dashboard' are now unresolvable targets, not open_view (#3044 item 5: both were deleted outright — CanvasRenderer.svelte no longer has a render branch for either, so isRenderedView rejects them and this dispatch layer surfaces a visible error instead of silently 'opening' a screen that no longer exists). Supersedes the pre-#3044 assertion that these still routed through as open_view.", () => {
      expect(decideNavigationAction({ view: 'task_list' })).toEqual({
        kind: 'error',
        message: 'Navigation failed: "task_list" is not a recognized view.',
      });
      expect(decideNavigationAction({ view: 'dashboard' })).toEqual({
        kind: 'error',
        message: 'Navigation failed: "dashboard" is not a recognized view.',
      });
    });

    it("'kanban' is special-cased to a toggle event, not a view switch", () => {
      const decision = decideNavigationAction({ view: 'kanban' });
      expect(decision).toEqual({ kind: 'toggle_kanban' });
    });

    it("'set_filter' is special-cased with its tab carried through", () => {
      const decision = decideNavigationAction({ view: 'set_filter', tab: 'tasks' });
      expect(decision).toEqual({ kind: 'set_filter', tab: 'tasks' });
    });
  });

  describe('item 4 — never silently does nothing', () => {
    it('a bare, empty payload ({}) is a visible error, not silence (the OLD code: no branch matched at all, nothing happened)', () => {
      const decision = decideNavigationAction({});
      expect(decision.kind).toBe('error');
      expect((decision as any).message).toMatch(/no target/i);
    });

    it('an unrecognized view string with no project context is a visible error naming the bad value', () => {
      const decision = decideNavigationAction({ view: 'definitely_not_a_real_view' });
      expect(decision).toEqual({
        kind: 'error',
        message: 'Navigation failed: "definitely_not_a_real_view" is not a recognized view.',
      });
    });

    it('dead removed view names (folder_detail/project_detail) are explicitly rejected even with no project context', () => {
      expect(decideNavigationAction({ view: 'folder_detail' }).kind).toBe('error');
      expect(decideNavigationAction({ view: 'project_detail' }).kind).toBe('error');
    });

    it('a declared-but-unrendered CanvasViewType (task_detail/today/tag/tags-manage/welcome) is rejected the same way', () => {
      for (const deadView of ['task_detail', 'today', 'tag', 'tags-manage', 'welcome']) {
        const decision = decideNavigationAction({ view: deadView });
        expect(decision.kind, `view='${deadView}' must be rejected, not silently accepted`).toBe('error');
      }
    });
  });
});
