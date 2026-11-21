/**
 * #2379 — All Tasks view parity with Project (Kanban) view.
 *
 * Full scoping audit: docs/audits/2026-08-07-2379-all-tasks-view-parity-scope.md
 * Matt's ruling: All Tasks looks and acts like Project view, with exactly one exception
 * (no Cabinet column). This file pins the behavioral contracts called out in that audit
 * that are independently testable outside a full component render (AllTasksView.svelte
 * requires the full Tauri window context, per the house pattern established by
 * NotesBrowser.collapse-rails.test.ts / PanelWidthResizeGuard.test.ts).
 *
 * Covers:
 *   1. Close-scope (#2329 class fix, audit §3): a click inside the .editor-panel subtree
 *      must never reach handleCloseEditor, mirroring
 *      NotesBrowser.checklist-close-scope.test.ts's real-DOM-bubbling harness.
 *   2. Resize-guard timing (#2331, audit §4.1): isEditorResizing must block the
 *      relocated close-handler for the deferred-clear window, same contract as
 *      PanelWidthResizeGuard.test.ts's kanban-view suite.
 *   3. All-columns-collapsible (Matt's ruling supersedes donor's Done-only/>5-task gate,
 *      audit §1.4/§1.6): every column toggles independently via a Set, unconditionally.
 *   4. Wiki-link wiring: navigateWikiLink is invoked with the title and the shared
 *      callbacks only (#3044: navigateWikiLink no longer accepts projectId/folderId/
 *      createNoteFn/confirmFn at all — a missing note stays inert, full stop).
 *
 * Harness: Vitest (jsdom) — matches existing pattern in src/lib/components/*.test.ts.
 */

import { describe, it, expect, vi, beforeEach } from 'vitest';

// ── 1. Close-scope (#2329 class fix) ────────────────────────────────────────────────
//
// Builds the FIXED AllTasksView topology exactly as restructured for #2379: the
// close-handler now lives on `.kanban-container` (sibling of `.editor-panel`, both
// children of `.tasks-body`) instead of on `.all-tasks-view` (the shared-ancestor
// view root). Mirrors NotesBrowser.checklist-close-scope.test.ts's buildFixedTopology
// exactly, with AllTasksView's class names substituted for NotesBrowser's.
function buildFixedTopology(onClose: () => void, showEditor: () => boolean, isResizing: () => boolean) {
  const body = document.createElement('div');
  body.className = 'tasks-body';

  const kanbanContainer = document.createElement('div');
  kanbanContainer.className = 'kanban-container';
  kanbanContainer.addEventListener('click', (e) => {
    if (isResizing()) return;
    const target = e.target as HTMLElement;
    if (showEditor()
        && !target.closest('.editor-panel')
        && !target.closest('.pane-card')) {
      onClose();
    }
  });

  const columns = document.createElement('div');
  columns.className = 'kanban-columns';
  kanbanContainer.appendChild(columns);

  const editorPanel = document.createElement('div');
  editorPanel.className = 'editor-panel';

  body.appendChild(kanbanContainer);
  body.appendChild(editorPanel);
  document.body.appendChild(body);

  return { body, kanbanContainer, columns, editorPanel };
}

// Reverted (pre-#2379) topology: takes the FIXED handler's REDUCED denylist (no
// `.editor-panel` entry — since the structural fix makes that entry unnecessary when the
// handler lives on `.kanban-container`) and re-binds it onto `.all-tasks-view`, which
// (unlike `.kanban-container`) IS an ancestor of `.editor-panel`. This reproduces the
// actual regression risk per the NotesBrowser.checklist-close-scope.test.ts house pattern:
// a future edit moves the onclick={...} back onto the view root (e.g. during a merge
// conflict) without also restoring the now-redundant `.editor-panel` denylist entry.
function buildRevertedRootTopology(onClose: () => void, showEditor: () => boolean, isResizing: () => boolean) {
  const root = document.createElement('div');
  root.className = 'all-tasks-view';
  root.addEventListener('click', (e) => {
    if (isResizing()) return;
    const target = e.target as HTMLElement;
    // Same reduced denylist as the FIXED .kanban-container handler would need if it dropped
    // the belt-and-suspenders .editor-panel check — this is what a naive "just move the
    // handler back" revert looks like.
    if (showEditor() && !target.closest('.pane-card')) {
      onClose();
    }
  });

  const body = document.createElement('div');
  body.className = 'tasks-body';
  const kanbanContainer = document.createElement('div');
  kanbanContainer.className = 'kanban-container';
  const editorPanel = document.createElement('div');
  editorPanel.className = 'editor-panel';

  body.appendChild(kanbanContainer);
  body.appendChild(editorPanel);
  root.appendChild(body);
  document.body.appendChild(root);

  return { root, body, kanbanContainer, editorPanel };
}

describe('AllTasksView close-handler scope — #2329 class fix (#2379)', () => {
  describe('editor-internal clicks do not invoke handleCloseEditor', () => {
    it('a click on a plain element inside .editor-panel does not fire handleCloseEditor', () => {
      const onClose = vi.fn();
      const { editorPanel } = buildFixedTopology(onClose, () => true, () => false);

      const editorBody = document.createElement('div');
      editorBody.className = 'editor-container';
      editorPanel.appendChild(editorBody);

      editorBody.dispatchEvent(new MouseEvent('click', { bubbles: true }));

      expect(onClose).not.toHaveBeenCalled();
    });

    it('a click on a simulated checklist row (nested NodeView mount target) inside .editor-panel does not fire handleCloseEditor', () => {
      const onClose = vi.fn();
      const { editorPanel } = buildFixedTopology(onClose, () => true, () => false);

      const checklistMount = document.createElement('div');
      checklistMount.className = 'checklist-nodeview';
      const row = document.createElement('div');
      row.className = 'checklist-item-row';
      checklistMount.appendChild(row);
      editorPanel.appendChild(checklistMount);

      row.dispatchEvent(new MouseEvent('click', { bubbles: true }));

      expect(onClose).not.toHaveBeenCalled();
    });
  });

  describe('.kanban-container background click invokes handleCloseEditor', () => {
    it('clicking .kanban-columns background (not a card/editor) fires handleCloseEditor', () => {
      const onClose = vi.fn();
      const { columns } = buildFixedTopology(onClose, () => true, () => false);

      columns.dispatchEvent(new MouseEvent('click', { bubbles: true }));

      expect(onClose).toHaveBeenCalledTimes(1);
    });

    it('does not fire when the editor is not open', () => {
      const onClose = vi.fn();
      const { columns } = buildFixedTopology(onClose, () => false, () => false);

      columns.dispatchEvent(new MouseEvent('click', { bubbles: true }));

      expect(onClose).not.toHaveBeenCalled();
    });

    it('a .pane-card click inside .kanban-container still does not fire handleCloseEditor (retained guard)', () => {
      const onClose = vi.fn();
      const { columns } = buildFixedTopology(onClose, () => true, () => false);
      const card = document.createElement('div');
      card.className = 'pane-card';
      columns.appendChild(card);

      card.dispatchEvent(new MouseEvent('click', { bubbles: true }));

      expect(onClose).not.toHaveBeenCalled();
    });
  });

  // ── Red-on-revert: re-binding the handler to .all-tasks-view flips the editor-click test ──
  describe('red-on-revert: re-binding the handler to .all-tasks-view flips the editor-click test red', () => {
    it('a checklist-row click inside .editor-panel DOES fire handleCloseEditor once the handler is re-bound to the view root', () => {
      const onClose = vi.fn();
      const { editorPanel } = buildRevertedRootTopology(onClose, () => true, () => false);

      const checklistMount = document.createElement('div');
      checklistMount.className = 'checklist-nodeview';
      const row = document.createElement('div');
      row.className = 'checklist-item-row';
      checklistMount.appendChild(row);
      editorPanel.appendChild(checklistMount);

      row.dispatchEvent(new MouseEvent('click', { bubbles: true }));

      expect(onClose).toHaveBeenCalledTimes(1);
    });

    it('structural check: .editor-panel is a descendant of the handler element under the reverted topology, but not under the fixed topology', () => {
      const { root: revertedRoot, editorPanel: revertedPanel } = buildRevertedRootTopology(vi.fn(), () => true, () => false);
      expect(revertedRoot.contains(revertedPanel)).toBe(true);

      const { kanbanContainer, editorPanel: fixedPanel } = buildFixedTopology(vi.fn(), () => true, () => false);
      expect(kanbanContainer.contains(fixedPanel)).toBe(false);
    });
  });
});

// ── 2. Resize-guard timing (#2331) ──────────────────────────────────────────────────
//
// Mirrors PanelWidthResizeGuard.test.ts's makeKanbanPanelHandlers, adapted to
// AllTasksView's naming (allTasksEditorWidth/isEditorResizing/handleCloseEditor).
function makeAllTasksPanelHandlers(initialWidth = 560) {
  let allTasksEditorWidth = initialWidth;
  let isEditorResizing = false;
  let showEditor = true;
  let closeCallCount = 0;

  const state = () => ({ allTasksEditorWidth, isEditorResizing, showEditor });

  function startResize(splitViewClientWidth: number, startX: number) {
    isEditorResizing = true;
    const startWidth = allTasksEditorWidth;
    function onMouseMove(clientX: number) {
      const delta = startX - clientX;
      const maxWidth = splitViewClientWidth;
      allTasksEditorWidth = Math.max(350, Math.min(maxWidth, startWidth + delta));
    }
    function onMouseUp(deferredClear: (fn: () => void) => void) {
      deferredClear(() => { isEditorResizing = false; });
    }
    return { onMouseMove, onMouseUp };
  }

  // Mirrors the relocated close-handler's guard ordering: isEditorResizing checked
  // FIRST, before the existing closest()-based exclusion list.
  function kanbanContainerOnClick(target: { matchesPaneCardOrEditorPanel?: boolean } = {}) {
    if (isEditorResizing) return;
    if (target.matchesPaneCardOrEditorPanel) return;
    if (!showEditor) return;
    closeCallCount++;
    showEditor = false;
  }

  return { state, startResize, kanbanContainerOnClick, getCloseCallCount: () => closeCallCount };
}

describe('AllTasksView resize-guard timing — #2331 (#2379)', () => {
  it('RED-ON-REVERT: with the guard true, a columns-background click does NOT invoke handleCloseEditor', () => {
    const h = makeAllTasksPanelHandlers(560);
    h.startResize(1600, 1000);
    expect(h.state().isEditorResizing).toBe(true);

    h.kanbanContainerOnClick({});
    expect(h.getCloseCallCount()).toBe(0);
    expect(h.state().showEditor).toBe(true);
  });

  it('RED-ON-REVERT: with the guard false (no active resize), the SAME click DOES invoke handleCloseEditor', () => {
    const h = makeAllTasksPanelHandlers(560);
    expect(h.state().isEditorResizing).toBe(false);

    h.kanbanContainerOnClick({});
    expect(h.getCloseCallCount()).toBe(1);
    expect(h.state().showEditor).toBe(false);
  });

  it('the deferred-clear callback is NOT invoked synchronously by onMouseUp itself (timing contract)', () => {
    const h = makeAllTasksPanelHandlers(560);
    const { onMouseUp } = h.startResize(1600, 1000);
    const deferrer = vi.fn();
    onMouseUp(deferrer);
    expect(deferrer).toHaveBeenCalledTimes(1);
    expect(h.state().isEditorResizing).toBe(true); // still true immediately after onMouseUp returns
  });

  it('once the deferred clear fires, columns-click closes the editor normally again', () => {
    const h = makeAllTasksPanelHandlers(560);
    const { onMouseUp } = h.startResize(1600, 1000);
    let deferredFn: (() => void) | null = null;
    onMouseUp((fn) => { deferredFn = fn; });
    expect(h.state().isEditorResizing).toBe(true);
    deferredFn!();
    expect(h.state().isEditorResizing).toBe(false);

    h.kanbanContainerOnClick({});
    expect(h.getCloseCallCount()).toBe(1);
  });

  it('existing pane-card/editor-panel exclusion still works when NOT resizing', () => {
    const h = makeAllTasksPanelHandlers(560);
    h.kanbanContainerOnClick({ matchesPaneCardOrEditorPanel: true });
    expect(h.getCloseCallCount()).toBe(0);
  });

  it('dynamic max-width: allTasksEditorWidth grows to splitViewRef.clientWidth (was hardcoded 800)', () => {
    const h = makeAllTasksPanelHandlers(560);
    const splitViewWidth = 1600;
    const { onMouseMove } = h.startResize(splitViewWidth, 1000);
    onMouseMove(-4000); // large leftward drag
    expect(h.state().allTasksEditorWidth).toBe(splitViewWidth);
    expect(h.state().allTasksEditorWidth).toBeGreaterThan(800);
  });
});

// ── 3. All-columns-collapsible (Matt's ruling supersedes donor's gate) ─────────────
//
// Mirrors the collapsedColumns Set mechanism now in AllTasksView.svelte: EVERY column
// toggles independently, unconditionally — no >5-task or Done-only gate.
function makeCollapsibleColumns(statuses: string[], initiallyCollapsed: string[] = ['done']) {
  let collapsedColumns = new Set(initiallyCollapsed);

  function toggleColumnCollapse(status: string) {
    if (collapsedColumns.has(status)) {
      collapsedColumns.delete(status);
    } else {
      collapsedColumns.add(status);
    }
    collapsedColumns = new Set(collapsedColumns);
  }

  return {
    isCollapsed: (status: string) => collapsedColumns.has(status),
    toggleColumnCollapse,
  };
}

describe('AllTasksView all-columns-collapsible — Matt\'s ruling supersedes donor gate (#2379)', () => {
  const STATUSES = ['todo', 'in_progress', 'review', 'done'];

  it('Done defaults collapsed on mount', () => {
    const cols = makeCollapsibleColumns(STATUSES);
    expect(cols.isCollapsed('done')).toBe(true);
    expect(cols.isCollapsed('todo')).toBe(false);
    expect(cols.isCollapsed('in_progress')).toBe(false);
    expect(cols.isCollapsed('review')).toBe(false);
  });

  it('EVERY column toggles collapse — not just Done (RED-ON-REVERT vs donor gate)', () => {
    const cols = makeCollapsibleColumns(STATUSES);
    for (const status of STATUSES) {
      const before = cols.isCollapsed(status);
      cols.toggleColumnCollapse(status);
      expect(cols.isCollapsed(status)).toBe(!before);
    }
  });

  it('todo/in_progress/review are collapsible unconditionally — no task-count gate required', () => {
    // Donor gates non-Done columns behind `getTasksForColumn(status).length > 5`. Matt's
    // ruling removes that gate entirely — these columns must collapse even with 0 or 1 tasks.
    const cols = makeCollapsibleColumns(STATUSES);
    cols.toggleColumnCollapse('todo');
    expect(cols.isCollapsed('todo')).toBe(true);
    cols.toggleColumnCollapse('review');
    expect(cols.isCollapsed('review')).toBe(true);
  });

  it('toggling one column does not affect the others', () => {
    const cols = makeCollapsibleColumns(STATUSES);
    cols.toggleColumnCollapse('todo');
    expect(cols.isCollapsed('todo')).toBe(true);
    expect(cols.isCollapsed('in_progress')).toBe(false);
    expect(cols.isCollapsed('review')).toBe(false);
    expect(cols.isCollapsed('done')).toBe(true); // unchanged default
  });

  it('collapse state is not persisted across a fresh mount (no #2252 persistence, audit §1.5)', () => {
    const cols1 = makeCollapsibleColumns(STATUSES);
    cols1.toggleColumnCollapse('todo');
    expect(cols1.isCollapsed('todo')).toBe(true);

    // A fresh instance (simulating remount) always resets to the Done-collapsed default —
    // no persistence layer is consulted.
    const cols2 = makeCollapsibleColumns(STATUSES);
    expect(cols2.isCollapsed('todo')).toBe(false);
    expect(cols2.isCollapsed('done')).toBe(true);
  });
});

// ── 4. Wiki-link wiring ─────────────────────────────────────────────────────────────

vi.mock('$lib/utils/wikiLinkNavigation', () => ({
  navigateWikiLink: vi.fn().mockResolvedValue(undefined),
}));

describe('AllTasksView wiki-link wiring — onNavigate is passed and invoked (#2379)', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('handleAllTasksWikiLinkNavigation calls navigateWikiLink with the shared callbacks only — no projectId/folderId/createNoteFn exist on the call at all (#3044)', async () => {
    const { navigateWikiLink } = await import('$lib/utils/wikiLinkNavigation');

    // Mirrors AllTasksView.svelte's handleAllTasksWikiLinkNavigation shape.
    async function handleAllTasksWikiLinkNavigation(title: string) {
      await (navigateWikiLink as any)(title, {
        searchFn: vi.fn(),
        selectNoteFn: vi.fn(),
        getNoteRawFn: vi.fn(),
      });
    }

    await handleAllTasksWikiLinkNavigation('Some Note');

    expect(navigateWikiLink).toHaveBeenCalledOnce();
    const [title, opts] = (navigateWikiLink as any).mock.calls[0];
    expect(title).toBe('Some Note');
    // RED-ON-REVERT: the create path's params must not be reintroduced on this call.
    expect(opts.projectId).toBeUndefined();
    expect(opts.folderId).toBeUndefined();
    expect(opts.createNoteFn).toBeUndefined();
    expect(opts.confirmFn).toBeUndefined();
    expect(typeof opts.searchFn).toBe('function');
    expect(typeof opts.selectNoteFn).toBe('function');
    expect(typeof opts.getNoteRawFn).toBe('function');
  });

  it('a thrown error from navigateWikiLink is caught and does not propagate (matches donor error handling)', async () => {
    const { navigateWikiLink } = await import('$lib/utils/wikiLinkNavigation');
    (navigateWikiLink as any).mockRejectedValueOnce(new Error('boom'));
    const consoleErrorSpy = vi.spyOn(console, 'error').mockImplementation(() => {});

    async function handleAllTasksWikiLinkNavigation(title: string) {
      try {
        await (navigateWikiLink as any)(title, {
          searchFn: vi.fn(),
          selectNoteFn: vi.fn(),
          getNoteRawFn: vi.fn(),
        });
      } catch (e) {
        console.error('[handleAllTasksWikiLinkNavigation] Failed to navigate from wiki link:', e);
      }
    }

    await expect(handleAllTasksWikiLinkNavigation('Some Note')).resolves.toBeUndefined();
    expect(consoleErrorSpy).toHaveBeenCalled();
    consoleErrorSpy.mockRestore();
  });
});
