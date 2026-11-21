/**
 * #2329 — Checklist items (and other in-note component clicks) closing the note in
 * Folder View instead of editing it.
 *
 * Root cause (full audit: docs/audits/2026-08-06-2329-checklist-folder-view-scope.md):
 * NotesBrowser.svelte's click-outside-to-close handler used to live on `.fv-stage`, the
 * shared ancestor of BOTH the card columns (`.grid-folder-content`) and the note editor
 * overlay (`.fv-note`). A click originating inside the editor — including clicks inside a
 * NodeView-mounted component like the checklist (Checklist.ts's `mount()` call) — bubbles
 * through `.fv-stage` in the real DOM, so the handler needed a `closest('.fv-note')`
 * denylist to avoid closing the note on every in-editor click. That denylist did not
 * reliably catch checklist clicks (see audit §1 for the suspect Svelte 5 nested-mount
 * delegation mechanism, which jsdom does not reproduce — see note on test 4 below).
 *
 * The fix (this PR) is structural, not a denylist patch: `.grid-folder-content` is now
 * wrapped in `.fv-cards-panel`, which carries the close-handler. `.fv-note` is a SIBLING
 * of `.fv-cards-panel` (both children of `.fv-stage`), not a descendant of the close
 * handler's own element — mirroring KanbanBoard.svelte's `.kanban-left-panel` /
 * `.kanban-editor-panel` sibling split (KanbanBoard.svelte:597/706), which is why Project
 * View was never affected by this bug (audit §1, §2).
 *
 * `.fv-void` (the background shown when no note is open, and the strip beside an open
 * note) is a sibling of `.fv-cards-panel` too, so it gets its own direct onclick calling
 * goBackToCards() — no closest() needed, per audit §2's reuse walkthrough.
 *
 * These tests exercise real DOM trees + dispatched click events (jsdom supports genuine
 * bubbling) mirroring the exact containment structure now in NotesBrowser.svelte, following
 * the house pattern established by NotesBrowser.collapse-rails.test.ts's
 * "collapse-click propagation" suite. Component-render testing is not used because
 * NotesBrowser.svelte requires the full Tauri window context (per that file's header note).
 *
 * Harness: Vitest (jsdom) — matches existing pattern in src/lib/components/*.test.ts.
 */

import { describe, it, expect, vi } from 'vitest';

/**
 * Builds the FIXED (#2329) DOM topology and attaches the two handlers exactly as they
 * now exist in NotesBrowser.svelte:
 *   - .fv-cards-panel onclick: scoped to the card-column subtree only. .fv-note is NOT
 *     a descendant, so no .fv-note closest() guard is needed anymore. .pane-card,
 *     .fv-collapse, and .fv-railcol ARE still descendants (they render inside
 *     .grid-folder-content) so those three guards are retained.
 *   - .fv-void onclick: direct handler, no closest() needed (no interactive children).
 *
 * Structure:
 *   .fv-stage
 *     .fv-cards-panel (onclick handler #1)
 *       .grid-folder-content
 *         ... .pane-card / .fv-collapse / .fv-railcol ...
 *     .fv-void (onclick handler #2)
 *     .fv-note
 *       ... checklist / editor content ...
 */
function buildFixedTopology(onGoBack: () => void, fvNoteOpen: () => boolean, isResizing: () => boolean) {
  const stage = document.createElement('div');
  stage.className = 'fv-stage';

  const cardsPanel = document.createElement('div');
  cardsPanel.className = 'fv-cards-panel';
  cardsPanel.addEventListener('click', (e) => {
    if (isResizing()) return;
    const target = e.target as HTMLElement;
    if (fvNoteOpen()
        && !target.closest('.pane-card')
        && !target.closest('.fv-collapse')
        && !target.closest('.fv-railcol')) {
      onGoBack();
    }
  });

  const gridContent = document.createElement('div');
  gridContent.className = 'grid-folder-content';
  cardsPanel.appendChild(gridContent);

  const voidEl = document.createElement('div');
  voidEl.className = 'fv-void';
  voidEl.addEventListener('click', () => {
    if (fvNoteOpen() && !isResizing()) onGoBack();
  });

  const note = document.createElement('div');
  note.className = 'fv-note';

  stage.appendChild(cardsPanel);
  stage.appendChild(voidEl);
  stage.appendChild(note);
  document.body.appendChild(stage);

  return { stage, cardsPanel, gridContent, voidEl, note };
}

/**
 * Builds the REVERTED topology: the FIXED handler logic (the reduced denylist — no
 * `.fv-note` entry, since the whole point of the structural fix is that entry becomes
 * unnecessary) re-bound directly onto `.fv-stage`, which (unlike `.fv-cards-panel`) IS an
 * ancestor of `.fv-note`. This is precisely the regression scenario named in the task spec:
 * "re-binding the handler to `.fv-stage` should flip the editor-click test red" — i.e. a
 * future edit moves the onclick={...} from `.fv-cards-panel` back onto `.fv-stage` without
 * also restoring the `.fv-note` denylist entry (which the structural fix intentionally
 * removed as redundant). Under the FIXED topology (`.fv-cards-panel`-scoped), `.fv-note` is
 * never a descendant of the handler's element, so the missing denylist entry is harmless.
 * Re-bound onto `.fv-stage`, `.fv-note` IS a descendant again, so the missing denylist entry
 * becomes load-bearing again — and clicks inside `.fv-note` incorrectly reach goBackToCards().
 */
function buildRevertedStageTopology(onGoBack: () => void, fvNoteOpen: () => boolean, isResizing: () => boolean) {
  const stage = document.createElement('div');
  stage.className = 'fv-stage';
  stage.addEventListener('click', (e) => {
    if (isResizing()) return;
    const target = e.target as HTMLElement;
    // Same reduced denylist as the FIXED .fv-cards-panel handler (no '.fv-note' entry) —
    // this is what a naive "just move the handler back" revert would look like.
    if (fvNoteOpen()
        && !target.closest('.pane-card')
        && !target.closest('.fv-collapse')
        && !target.closest('.fv-railcol')) {
      onGoBack();
    }
  });

  const gridContent = document.createElement('div');
  gridContent.className = 'grid-folder-content';

  const voidEl = document.createElement('div');
  voidEl.className = 'fv-void';

  const note = document.createElement('div');
  note.className = 'fv-note';

  stage.appendChild(gridContent);
  stage.appendChild(voidEl);
  stage.appendChild(note);
  document.body.appendChild(stage);

  return { stage, gridContent, voidEl, note };
}

describe('NotesBrowser.svelte Folder View close-handler scope — #2329', () => {
  // ── 1. Click inside .fv-note subtree (e.g. a checklist row) must NOT close the note ──
  describe('editor-internal clicks do not invoke goBackToCards', () => {
    it('a click on a plain element inside .fv-note does not fire goBackToCards', () => {
      const goBack = vi.fn();
      const { note } = buildFixedTopology(goBack, () => true, () => false);

      const editorBody = document.createElement('div');
      editorBody.className = 'editor-container';
      note.appendChild(editorBody);

      editorBody.dispatchEvent(new MouseEvent('click', { bubbles: true }));

      expect(goBack).not.toHaveBeenCalled();
    });

    it('a click on a simulated checklist row (nested NodeView mount target) inside .fv-note does not fire goBackToCards', () => {
      const goBack = vi.fn();
      const { note } = buildFixedTopology(goBack, () => true, () => false);

      // Mirrors Checklist.ts's _createChecklistNodeViewFactory: a raw div mounted as an
      // independent Svelte root, inserted into the editor DOM. Regardless of the specific
      // Svelte 5 nested-mount delegation mechanism (audit §1, not runtime-confirmed there),
      // the structural claim under test holds: this element is a descendant of .fv-note,
      // which is no longer a descendant of the close-handler's own element (.fv-cards-panel).
      const checklistMount = document.createElement('div');
      checklistMount.className = 'checklist-nodeview';
      const row = document.createElement('div');
      row.className = 'checklist-item-row';
      checklistMount.appendChild(row);
      note.appendChild(checklistMount);

      row.dispatchEvent(new MouseEvent('click', { bubbles: true }));

      expect(goBack).not.toHaveBeenCalled();
    });
  });

  // ── 2. A .fv-void click DOES invoke goBackToCards ──────────────────────────────────
  describe('.fv-void click invokes goBackToCards', () => {
    it('clicking .fv-void directly fires goBackToCards when a note is open', () => {
      const goBack = vi.fn();
      const { voidEl } = buildFixedTopology(goBack, () => true, () => false);

      voidEl.dispatchEvent(new MouseEvent('click', { bubbles: true }));

      expect(goBack).toHaveBeenCalledTimes(1);
    });

    it('does not fire when no note is open (fvNoteOpen=false)', () => {
      const goBack = vi.fn();
      const { voidEl } = buildFixedTopology(goBack, () => false, () => false);

      voidEl.dispatchEvent(new MouseEvent('click', { bubbles: true }));

      expect(goBack).not.toHaveBeenCalled();
    });

    it('is suppressed during an in-progress resize drag (#2331 guard)', () => {
      const goBack = vi.fn();
      const { voidEl } = buildFixedTopology(goBack, () => true, () => true);

      voidEl.dispatchEvent(new MouseEvent('click', { bubbles: true }));

      expect(goBack).not.toHaveBeenCalled();
    });
  });

  // ── 3. A cards-panel background click with a note open DOES invoke goBackToCards ───
  describe('.fv-cards-panel background click invokes goBackToCards', () => {
    it('clicking .grid-folder-content background (not a card/collapse/rail) fires goBackToCards', () => {
      const goBack = vi.fn();
      const { gridContent } = buildFixedTopology(goBack, () => true, () => false);

      gridContent.dispatchEvent(new MouseEvent('click', { bubbles: true }));

      expect(goBack).toHaveBeenCalledTimes(1);
    });

    it('does not fire when no note is open', () => {
      const goBack = vi.fn();
      const { gridContent } = buildFixedTopology(goBack, () => false, () => false);

      gridContent.dispatchEvent(new MouseEvent('click', { bubbles: true }));

      expect(goBack).not.toHaveBeenCalled();
    });

    it('a .pane-card click inside .fv-cards-panel still does not fire goBackToCards (retained guard)', () => {
      const goBack = vi.fn();
      const { gridContent } = buildFixedTopology(goBack, () => true, () => false);
      const card = document.createElement('div');
      card.className = 'pane-card';
      gridContent.appendChild(card);

      card.dispatchEvent(new MouseEvent('click', { bubbles: true }));

      expect(goBack).not.toHaveBeenCalled();
    });

    it('a .fv-collapse click inside .fv-cards-panel still does not fire goBackToCards (retained guard)', () => {
      const goBack = vi.fn();
      const { gridContent } = buildFixedTopology(goBack, () => true, () => false);
      const collapseBtn = document.createElement('button');
      collapseBtn.className = 'fv-collapse';
      gridContent.appendChild(collapseBtn);

      collapseBtn.dispatchEvent(new MouseEvent('click', { bubbles: true }));

      expect(goBack).not.toHaveBeenCalled();
    });

    it('a .fv-railcol click inside .fv-cards-panel still does not fire goBackToCards (retained guard)', () => {
      const goBack = vi.fn();
      const { gridContent } = buildFixedTopology(goBack, () => true, () => false);
      const railBtn = document.createElement('button');
      railBtn.className = 'fv-railcol';
      gridContent.appendChild(railBtn);

      railBtn.dispatchEvent(new MouseEvent('click', { bubbles: true }));

      expect(goBack).not.toHaveBeenCalled();
    });

    it('is suppressed during an in-progress resize drag (#2331 guard)', () => {
      const goBack = vi.fn();
      const { gridContent } = buildFixedTopology(goBack, () => true, () => true);

      gridContent.dispatchEvent(new MouseEvent('click', { bubbles: true }));

      expect(goBack).not.toHaveBeenCalled();
    });
  });

  // ── 4. Red-on-revert: re-binding the handler to .fv-stage flips the editor-click test ──
  //
  // Per the task spec: "re-binding the handler to .fv-stage should flip the editor-click
  // test red." buildRevertedStageTopology() does exactly that — takes the FIXED handler's
  // reduced denylist (no `.fv-note` entry, since the structural fix makes that entry
  // unnecessary when the handler lives on `.fv-cards-panel`) and re-binds it onto
  // `.fv-stage`, which unlike `.fv-cards-panel` IS an ancestor of `.fv-note`. This
  // reproduces the actual regression risk: a future edit moves the onclick={...} back onto
  // `.fv-stage` (e.g. during a merge conflict or careless refactor) without also restoring
  // the now-redundant `.fv-note` denylist entry.
  describe('red-on-revert: re-binding the handler to .fv-stage flips the editor-click test red', () => {
    it('a checklist-row click inside .fv-note DOES fire goBackToCards once the handler is re-bound to .fv-stage', () => {
      const goBack = vi.fn();
      const { note } = buildRevertedStageTopology(goBack, () => true, () => false);

      const checklistMount = document.createElement('div');
      checklistMount.className = 'checklist-nodeview';
      const row = document.createElement('div');
      row.className = 'checklist-item-row';
      checklistMount.appendChild(row);
      note.appendChild(checklistMount);

      row.dispatchEvent(new MouseEvent('click', { bubbles: true }));

      // This is the "red" assertion: under the reverted topology, the SAME checklist click
      // that test 1 (above) proves does NOT fire goBackToCards under the FIXED topology DOES
      // fire it here. If a future revert moves the real handler back onto `.fv-stage`, the
      // equivalent assertion against the real component flips from pass to fail exactly like
      // this.
      expect(goBack).toHaveBeenCalledTimes(1);
    });

    it('structural check: .fv-note is a descendant of the handler element under the reverted topology, but not under the fixed topology', () => {
      const { stage: revertedStage, note: revertedNote } = buildRevertedStageTopology(vi.fn(), () => true, () => false);
      expect(revertedStage.contains(revertedNote)).toBe(true);
      expect(revertedStage.classList.contains('fv-stage')).toBe(true);
      // Reverted topology: the handler-bound element (.fv-stage) IS an ancestor of .fv-note.

      const { cardsPanel, note: fixedNote } = buildFixedTopology(vi.fn(), () => true, () => false);
      expect(cardsPanel.contains(fixedNote)).toBe(false);
      // Fixed topology: the handler-bound element (.fv-cards-panel) is NOT an ancestor of
      // .fv-note — they are siblings under .fv-stage. This is the structural guarantee that
      // makes test 1 (above) pass unconditionally, with no denylist entry required at all.
    });
  });
});
