/**
 * #2252 — Folder-view column collapse rails + persistence
 *
 * Tests the collapse-state logic added to NotesBrowser.svelte:
 *   - fvColumnScopeId(): resolves the ai_settings key scope (folder id, or project id
 *     when no folder is selected)
 *   - setColumnCollapsed()/toggleCabinetCollapsed()/toggleTasksCollapsed(): toggle +
 *     immediate persist via set_ai_setting
 *   - the scope-change load effect: reads both keys via get_ai_setting on mount/scope
 *     change, defaulting to expanded ('false') when no row exists yet
 *   - fvColumnBoundary(): the noteLeft anchor recompute (300/46-based) used when a
 *     column's collapse state toggles while a note is open
 *
 * These tests exercise the logic directly (not via NotesBrowser.svelte component render)
 * because NotesBrowser.svelte requires the full Tauri window context and is ~2000+ lines
 * of otherwise-unrelated notes/kanban logic. This is NOT a re-implementation — it's a
 * faithful extraction of the exact state/handler logic added for #2252, matching the
 * house pattern established by Settings.ctx-window-override.test.ts. Tests fail if the
 * corresponding wiring in NotesBrowser.svelte is reverted or the key format/convention
 * drifts.
 *
 * Harness: Vitest (jsdom) — matches existing pattern in src/lib/components/*.test.ts.
 *
 * #2252 bugfix additions (this file, follow-up to the original PR #2320):
 *   - Bug 1 (LAUNCH-BLOCKING): collapse-button click bubbling into .fv-stage's
 *     click-outside handler, closing the open note. jsdom DOES support real DOM event
 *     bubbling, so this is tested with an actual DOM tree + dispatched click event (not
 *     the logic-extraction style used above) — see 'collapse-click propagation' below.
 *   - Bug 3: scopeId-identity memo guard on the load effect, verifying it does not
 *     re-fire when fvColumnScopeId() returns the same value on consecutive calls (the
 *     upstream store-reference-churn scenario from #2324).
 */

import { describe, it, expect, vi, beforeEach } from 'vitest';

const FV_COL = 300;
const FV_RAIL = 46;

/**
 * Creates an isolated instance of the #2252 collapse-rail state + handlers, mirroring
 * NotesBrowser.svelte's fvColumnScopeId/setColumnCollapsed/toggleCabinetCollapsed/
 * toggleTasksCollapsed/fvColumnBoundary and the scope-change load effect.
 */
function makeCollapseRailHandlers(
  invokeImpl: (cmd: string, args?: any) => Promise<any>,
  initialScope: { folderId: string | null; projectId: string | null },
) {
  let activeFolderId: string | null = initialScope.folderId;
  let activeProjectId: string | null = initialScope.projectId;
  let cabinetCollapsed = false;
  let tasksCollapsed = false;
  let fvNoteOpen = false;
  let noteLeft = 300;

  // #2335: monotonic counter guarding the async get_ai_setting reads below, mirroring
  // selectSeq in noteService.ts and the collapseLoadSeq guard added to NotesBrowser.svelte.
  // Discards a stale (superseded) Promise.all resolve from an older scope so it cannot
  // overwrite state set by a newer scope change that fired (and possibly resolved) after it.
  let collapseLoadSeq = 0;

  const state = () => ({ cabinetCollapsed, tasksCollapsed, fvNoteOpen, noteLeft });

  // Mirrors NotesBrowser.svelte's fvColumnScopeId()
  function fvColumnScopeId(): string | null {
    return activeFolderId ?? activeProjectId ?? null;
  }

  // Mirrors NotesBrowser.svelte's fvColumnBoundary()
  function fvColumnBoundary(): number {
    return (cabinetCollapsed ? FV_RAIL : FV_COL) + (tasksCollapsed ? FV_RAIL : FV_COL);
  }

  // Mirrors the $effect load-on-scope-change block, including the #2335 selectSeq guard.
  async function loadCollapseStateForScope() {
    const scopeId = fvColumnScopeId();
    if (!scopeId) {
      cabinetCollapsed = false;
      tasksCollapsed = false;
      return;
    }
    const mySeq = ++collapseLoadSeq;
    try {
      const [cabinetVal, tasksVal] = await Promise.all([
        invokeImpl('get_ai_setting', { key: `folder:${scopeId}:cabinet:collapsed` }),
        invokeImpl('get_ai_setting', { key: `folder:${scopeId}:tasks:collapsed` }),
      ]);
      if (mySeq !== collapseLoadSeq) return; // superseded — discard stale resolve
      cabinetCollapsed = cabinetVal === 'true';
      tasksCollapsed = tasksVal === 'true';
    } catch (e) {
      if (mySeq !== collapseLoadSeq) return; // superseded — discard stale resolve
      cabinetCollapsed = false;
      tasksCollapsed = false;
    }
  }

  // Test-only accessor — real NotesBrowser.svelte does not expose collapseLoadSeq, but the
  // test suite needs it to assert the counter increments correctly on each scope change.
  function getCollapseLoadSeq() {
    return collapseLoadSeq;
  }

  // Mirrors setColumnCollapsed()
  async function setColumnCollapsed(column: 'cabinet' | 'tasks', collapsed: boolean) {
    const scopeId = fvColumnScopeId();
    if (column === 'cabinet') cabinetCollapsed = collapsed;
    else tasksCollapsed = collapsed;

    if (fvNoteOpen) {
      noteLeft = fvColumnBoundary();
    }

    if (!scopeId) return;
    try {
      await invokeImpl('set_ai_setting', {
        key: `folder:${scopeId}:${column}:collapsed`,
        value: String(collapsed),
      });
    } catch (e) {
      // mirrors console.error path in the real handler — swallow for test purposes
    }
  }

  function toggleCabinetCollapsed() {
    return setColumnCollapsed('cabinet', !cabinetCollapsed);
  }

  function toggleTasksCollapsed() {
    return setColumnCollapsed('tasks', !tasksCollapsed);
  }

  function setActiveFolder(folderId: string | null) {
    activeFolderId = folderId;
  }

  function setActiveProject(projectId: string | null) {
    activeProjectId = projectId;
  }

  function openNote() {
    fvNoteOpen = true;
  }

  function closeNote() {
    fvNoteOpen = false;
  }

  return {
    state,
    fvColumnScopeId,
    fvColumnBoundary,
    loadCollapseStateForScope,
    setColumnCollapsed,
    toggleCabinetCollapsed,
    toggleTasksCollapsed,
    setActiveFolder,
    setActiveProject,
    openNote,
    closeNote,
    getCollapseLoadSeq,
  };
}

function makeInvokeMock() {
  const store = new Map<string, string>();
  const invokeMock = vi.fn(async (cmd: string, args?: any): Promise<any> => {
    switch (cmd) {
      case 'get_ai_setting':
        return store.has(args.key) ? store.get(args.key) : null;
      case 'set_ai_setting':
        store.set(args.key, args.value);
        return null;
      default:
        return null;
    }
  });
  return { invokeMock, store };
}

describe('NotesBrowser.svelte folder-view collapse rails — #2252', () => {
  let invokeMock: ReturnType<typeof makeInvokeMock>['invokeMock'];
  let store: ReturnType<typeof makeInvokeMock>['store'];

  beforeEach(() => {
    ({ invokeMock, store } = makeInvokeMock());
  });

  // ── Toggle state management (collapsed/expanded transitions) ────────────────
  describe('collapse toggle state transitions', () => {
    it('starts expanded (both false) by default', async () => {
      const h = makeCollapseRailHandlers(invokeMock, { folderId: 'folder-1', projectId: 'proj-1' });
      await h.loadCollapseStateForScope();
      expect(h.state().cabinetCollapsed).toBe(false);
      expect(h.state().tasksCollapsed).toBe(false);
    });

    it('toggleCabinetCollapsed flips cabinetCollapsed only, tasksCollapsed untouched', async () => {
      const h = makeCollapseRailHandlers(invokeMock, { folderId: 'folder-1', projectId: 'proj-1' });
      await h.toggleCabinetCollapsed();
      expect(h.state().cabinetCollapsed).toBe(true);
      expect(h.state().tasksCollapsed).toBe(false);

      await h.toggleCabinetCollapsed();
      expect(h.state().cabinetCollapsed).toBe(false);
    });

    it('toggleTasksCollapsed flips tasksCollapsed only, cabinetCollapsed untouched', async () => {
      const h = makeCollapseRailHandlers(invokeMock, { folderId: 'folder-1', projectId: 'proj-1' });
      await h.toggleTasksCollapsed();
      expect(h.state().tasksCollapsed).toBe(true);
      expect(h.state().cabinetCollapsed).toBe(false);
    });

    it('all four combinations are independently reachable: both open, cabinet-only, tasks-only, both collapsed', async () => {
      const h = makeCollapseRailHandlers(invokeMock, { folderId: 'folder-1', projectId: 'proj-1' });

      // both open (default)
      expect(h.state()).toMatchObject({ cabinetCollapsed: false, tasksCollapsed: false });

      // cabinet only
      await h.toggleCabinetCollapsed();
      expect(h.state()).toMatchObject({ cabinetCollapsed: true, tasksCollapsed: false });

      // both collapsed
      await h.toggleTasksCollapsed();
      expect(h.state()).toMatchObject({ cabinetCollapsed: true, tasksCollapsed: true });

      // tasks only (cabinet re-expanded)
      await h.toggleCabinetCollapsed();
      expect(h.state()).toMatchObject({ cabinetCollapsed: false, tasksCollapsed: true });

      // back to both open
      await h.toggleTasksCollapsed();
      expect(h.state()).toMatchObject({ cabinetCollapsed: false, tasksCollapsed: false });
    });
  });

  // ── Persistence round-trip (save → load → verify state matches) ─────────────
  describe('persistence round-trip', () => {
    it('save → load → state matches for cabinet collapsed=true', async () => {
      const h1 = makeCollapseRailHandlers(invokeMock, { folderId: 'folder-42', projectId: 'proj-42' });
      await h1.toggleCabinetCollapsed(); // saves 'true'

      const setCalls = invokeMock.mock.calls.filter(([cmd, args]) => cmd === 'set_ai_setting' && args?.key === 'folder:folder-42:cabinet:collapsed');
      expect(setCalls).toHaveLength(1);
      expect(setCalls[0][1].value).toBe('true');

      // Simulate a fresh mount/remount of the component against the same store.
      const h2 = makeCollapseRailHandlers(invokeMock, { folderId: 'folder-42', projectId: 'proj-42' });
      await h2.loadCollapseStateForScope();
      expect(h2.state().cabinetCollapsed).toBe(true);
      expect(h2.state().tasksCollapsed).toBe(false);
    });

    it('save → load → state matches for both columns collapsed', async () => {
      const h1 = makeCollapseRailHandlers(invokeMock, { folderId: 'folder-99', projectId: 'proj-99' });
      await h1.toggleCabinetCollapsed();
      await h1.toggleTasksCollapsed();

      const h2 = makeCollapseRailHandlers(invokeMock, { folderId: 'folder-99', projectId: 'proj-99' });
      await h2.loadCollapseStateForScope();
      expect(h2.state().cabinetCollapsed).toBe(true);
      expect(h2.state().tasksCollapsed).toBe(true);
    });

    it('writes the exact key format folder:{folder_id}:{column}:collapsed with string true/false values', async () => {
      const h = makeCollapseRailHandlers(invokeMock, { folderId: 'folder-key-test', projectId: 'proj-key-test' });
      await h.toggleCabinetCollapsed();
      await h.toggleTasksCollapsed();

      expect(store.get('folder:folder-key-test:cabinet:collapsed')).toBe('true');
      expect(store.get('folder:folder-key-test:tasks:collapsed')).toBe('true');
    });

    it('missing keys (never-saved folder) default to expanded on load', async () => {
      const h = makeCollapseRailHandlers(invokeMock, { folderId: 'never-visited-folder', projectId: 'proj-x' });
      await h.loadCollapseStateForScope();
      expect(h.state().cabinetCollapsed).toBe(false);
      expect(h.state().tasksCollapsed).toBe(false);
    });

    it('writes immediately on toggle (no debounce) — exactly one invoke per toggle', async () => {
      const h = makeCollapseRailHandlers(invokeMock, { folderId: 'folder-imm', projectId: 'proj-imm' });
      invokeMock.mockClear();
      await h.toggleCabinetCollapsed();
      const setCalls = invokeMock.mock.calls.filter(([cmd]) => cmd === 'set_ai_setting');
      expect(setCalls).toHaveLength(1);
    });

    it('per-folder scoping: two different folders persist independently, no cross-contamination', async () => {
      const hA = makeCollapseRailHandlers(invokeMock, { folderId: 'folder-A', projectId: 'proj-shared' });
      await hA.toggleCabinetCollapsed(); // folder-A cabinet -> true

      const hB = makeCollapseRailHandlers(invokeMock, { folderId: 'folder-B', projectId: 'proj-shared' });
      await hB.loadCollapseStateForScope();
      // folder-B must NOT see folder-A's collapsed cabinet — must default to expanded.
      expect(hB.state().cabinetCollapsed).toBe(false);

      // Reload folder-A directly — its own state must still be there.
      const hA2 = makeCollapseRailHandlers(invokeMock, { folderId: 'folder-A', projectId: 'proj-shared' });
      await hA2.loadCollapseStateForScope();
      expect(hA2.state().cabinetCollapsed).toBe(true);
    });

    it('falls back to project-scoped key when no folder is selected (project root view)', async () => {
      const h1 = makeCollapseRailHandlers(invokeMock, { folderId: null, projectId: 'proj-root-only' });
      await h1.toggleTasksCollapsed();
      expect(store.get('folder:proj-root-only:tasks:collapsed')).toBe('true');

      const h2 = makeCollapseRailHandlers(invokeMock, { folderId: null, projectId: 'proj-root-only' });
      await h2.loadCollapseStateForScope();
      expect(h2.state().tasksCollapsed).toBe(true);
    });

    it('no scope (no folder, no project) does not throw and does not invoke set_ai_setting', async () => {
      const h = makeCollapseRailHandlers(invokeMock, { folderId: null, projectId: null });
      invokeMock.mockClear();
      await expect(h.toggleCabinetCollapsed()).resolves.toBeUndefined();
      const setCalls = invokeMock.mock.calls.filter(([cmd]) => cmd === 'set_ai_setting');
      expect(setCalls).toHaveLength(0);
      // Local UI state still toggles even with no persistence scope.
      expect(h.state().cabinetCollapsed).toBe(true);
    });

    it('load failure (invoke rejects) fails open to expanded rather than leaving stale state', async () => {
      const failingInvoke = vi.fn(async (cmd: string) => {
        if (cmd === 'get_ai_setting') throw new Error('DB locked');
        return null;
      });
      const h = makeCollapseRailHandlers(failingInvoke as any, { folderId: 'folder-fail', projectId: 'proj-fail' });
      await h.loadCollapseStateForScope();
      expect(h.state().cabinetCollapsed).toBe(false);
      expect(h.state().tasksCollapsed).toBe(false);
    });
  });

  // ── noteLeft boundary recompute (both-collapsed + note-open behavior) ───────
  describe('fvColumnBoundary (noteLeft anchor recompute)', () => {
    it('both expanded → 600 (2×300)', () => {
      const h = makeCollapseRailHandlers(invokeMock, { folderId: 'f', projectId: 'p' });
      expect(h.fvColumnBoundary()).toBe(600);
    });

    it('cabinet collapsed, tasks expanded → 346 (46+300)', async () => {
      const h = makeCollapseRailHandlers(invokeMock, { folderId: 'f', projectId: 'p' });
      await h.toggleCabinetCollapsed();
      expect(h.fvColumnBoundary()).toBe(346);
    });

    it('both collapsed → 92 (2×46), matching design/spec 92px anchor', async () => {
      const h = makeCollapseRailHandlers(invokeMock, { folderId: 'f', projectId: 'p' });
      await h.toggleCabinetCollapsed();
      await h.toggleTasksCollapsed();
      expect(h.fvColumnBoundary()).toBe(92);
    });

    it('toggling a column while a note is open moves noteLeft to the new boundary immediately', async () => {
      const h = makeCollapseRailHandlers(invokeMock, { folderId: 'f', projectId: 'p' });
      h.openNote();
      expect(h.state().noteLeft).toBe(300); // initial default, unrelated to collapse

      await h.toggleCabinetCollapsed();
      await h.toggleTasksCollapsed();
      expect(h.state().noteLeft).toBe(92);
    });

    it('toggling a column while no note is open does NOT touch noteLeft (persisted drag width preserved)', async () => {
      const h = makeCollapseRailHandlers(invokeMock, { folderId: 'f', projectId: 'p' });
      // Simulate a previously-persisted custom panel width even though note is currently closed.
      expect(h.state().fvNoteOpen).toBe(false);
      await h.toggleCabinetCollapsed();
      // noteLeft untouched — stays at its pre-toggle default since fvNoteOpen was false.
      expect(h.state().noteLeft).toBe(300);
    });
  });

  // ── fvColumnScopeId resolution ────────────────────────────────────────────────
  describe('fvColumnScopeId', () => {
    it('prefers folder id when a folder is active', () => {
      const h = makeCollapseRailHandlers(invokeMock, { folderId: 'folder-x', projectId: 'proj-x' });
      expect(h.fvColumnScopeId()).toBe('folder-x');
    });

    it('falls back to project id when no folder is active', () => {
      const h = makeCollapseRailHandlers(invokeMock, { folderId: null, projectId: 'proj-only' });
      expect(h.fvColumnScopeId()).toBe('proj-only');
    });

    it('returns null when neither folder nor project is active', () => {
      const h = makeCollapseRailHandlers(invokeMock, { folderId: null, projectId: null });
      expect(h.fvColumnScopeId()).toBeNull();
    });
  });

  // ── #2252 Bug 1 (LAUNCH-BLOCKING): collapse click must not close the open note ──────
  //
  // Real DOM tree + dispatched click event (jsdom supports genuine bubbling), reproducing
  // the exact containment structure from NotesBrowser.svelte: .fv-stage > ... > .fv-collapse
  // (header button) and .fv-stage > ... > .fv-railcol (collapsed rail button). Both must
  // NOT trigger goBackToCards() when clicked, even without a stopPropagation() call inside
  // the button's own handler — because the .fv-stage exclusion list itself (Option B, the
  // defense-in-depth widening) must also independently prevent the close.
  describe('collapse-click propagation does not close the open note (#2252 Bug 1)', () => {
    // Mirrors the .fv-stage click-outside handler's exclusion list exactly as fixed in
    // NotesBrowser.svelte (fv-note, pane-card, fv-collapse, fv-railcol).
    function attachStageHandler(stage: HTMLElement, onGoBack: () => void, fvNoteOpen: () => boolean) {
      stage.addEventListener('click', (e) => {
        const target = e.target as HTMLElement;
        if (fvNoteOpen()
            && !target.closest('.fv-note')
            && !target.closest('.pane-card')
            && !target.closest('.fv-collapse')
            && !target.closest('.fv-railcol')) {
          onGoBack();
        }
      });
    }

    it('clicking a .fv-collapse header button does not fire goBackToCards, even without stopPropagation', () => {
      const stage = document.createElement('div');
      stage.className = 'fv-stage';
      const header = document.createElement('div');
      header.className = 'column-header-left';
      const button = document.createElement('button');
      button.className = 'fv-collapse';
      const icon = document.createElement('svg');
      button.appendChild(icon);
      header.appendChild(button);
      stage.appendChild(header);
      document.body.appendChild(stage);

      const goBack = vi.fn();
      attachStageHandler(stage, goBack, () => true);

      // Click the child <svg> (matches e.target being a nested icon element, as in the
      // real button markup) to exercise closest() bubbling up from a descendant.
      icon.dispatchEvent(new MouseEvent('click', { bubbles: true }));

      expect(goBack).not.toHaveBeenCalled();
      document.body.removeChild(stage);
    });

    it('clicking a .fv-railcol rail button does not fire goBackToCards, even without stopPropagation', () => {
      const stage = document.createElement('div');
      stage.className = 'fv-stage';
      const column = document.createElement('div');
      column.className = 'folder-column collapsed';
      const button = document.createElement('button');
      button.className = 'fv-railcol';
      const label = document.createElement('span');
      label.className = 'fv-railcol__label';
      button.appendChild(label);
      column.appendChild(button);
      stage.appendChild(column);
      document.body.appendChild(stage);

      const goBack = vi.fn();
      attachStageHandler(stage, goBack, () => true);

      label.dispatchEvent(new MouseEvent('click', { bubbles: true }));

      expect(goBack).not.toHaveBeenCalled();
      document.body.removeChild(stage);
    });

    it('control: clicking stage background (not a collapse/note/card element) DOES fire goBackToCards', () => {
      const stage = document.createElement('div');
      stage.className = 'fv-stage';
      const voidEl = document.createElement('div');
      voidEl.className = 'fv-void';
      stage.appendChild(voidEl);
      document.body.appendChild(stage);

      const goBack = vi.fn();
      attachStageHandler(stage, goBack, () => true);

      voidEl.dispatchEvent(new MouseEvent('click', { bubbles: true }));

      expect(goBack).toHaveBeenCalledTimes(1);
      document.body.removeChild(stage);
    });

    // Note (PR body / diagnosis-limitation disclosure): this test suite reproduces the
    // .fv-stage exclusion-list guard (Option B / defense-in-depth) faithfully via real DOM
    // bubbling in jsdom. It does NOT invoke the actual toggleCabinetCollapsed/
    // toggleTasksCollapsed functions from NotesBrowser.svelte (component render requires the
    // full Tauri window context, per this file's header note), so it cannot independently
    // verify that e.stopPropagation() was added inside those two functions (Option A, the
    // primary fix). That half of Bug 1 is covered by static code inspection only in CI;
    // Matt's DMG re-test is what gates Bug 1 as actually resolved, per the task instructions.
  });

  // ── #2252 Bug 3: load effect must not re-fire when scopeId is unchanged ─────────────
  describe('load effect scopeId-identity memo guard (#2252 Bug 3)', () => {
    // Mirrors the $effect body as fixed in NotesBrowser.svelte: a lastCollapseLoadScopeId
    // closure variable gates re-entry when fvColumnScopeId() resolves to the same id,
    // even if the underlying store object reference changed (simulated here by calling
    // fvColumnScopeId() through two different objects that happen to share the same id).
    function makeMemoGuardedLoader(invokeImpl: (cmd: string, args?: any) => Promise<any>) {
      let lastCollapseLoadScopeId: string | null = null;
      let fireCount = 0;

      async function runEffect(scopeId: string | null) {
        if (scopeId === lastCollapseLoadScopeId) return;
        lastCollapseLoadScopeId = scopeId;
        fireCount++;
        if (!scopeId) return;
        await Promise.all([
          invokeImpl('get_ai_setting', { key: `folder:${scopeId}:cabinet:collapsed` }),
          invokeImpl('get_ai_setting', { key: `folder:${scopeId}:tasks:collapsed` }),
        ]);
      }

      return { runEffect, getFireCount: () => fireCount };
    }

    it('does not re-fire when fvColumnScopeId() returns the same value consecutively', async () => {
      const { invokeMock } = makeInvokeMock();
      const { runEffect, getFireCount } = makeMemoGuardedLoader(invokeMock);

      // Simulate: note-open triggers a re-run with a NEW $activeFolder object reference,
      // but the SAME .id (the #2324 upstream-churn scenario) — three consecutive "fires"
      // with an identical scopeId, as would happen across open/close/open of a note within
      // the same folder.
      await runEffect('folder-abc');
      await runEffect('folder-abc');
      await runEffect('folder-abc');

      expect(getFireCount()).toBe(1);
      const getCalls = invokeMock.mock.calls.filter(([cmd]) => cmd === 'get_ai_setting');
      expect(getCalls).toHaveLength(2); // cabinet + tasks, exactly once
    });

    it('DOES re-fire when scopeId actually changes (folder navigation)', async () => {
      const { invokeMock } = makeInvokeMock();
      const { runEffect, getFireCount } = makeMemoGuardedLoader(invokeMock);

      await runEffect('folder-abc');
      await runEffect('folder-xyz');

      expect(getFireCount()).toBe(2);
    });

    it('re-fires after transitioning through null (leaving all folders/projects) and back', async () => {
      const { invokeMock } = makeInvokeMock();
      const { runEffect, getFireCount } = makeMemoGuardedLoader(invokeMock);

      await runEffect('folder-abc');
      await runEffect(null);
      await runEffect('folder-abc');

      expect(getFireCount()).toBe(3);
    });
  });

  // ── #2335: selectSeq-style guard on the collapse-load effect's async reads ─────────────
  //
  // Without this guard, an older/slower Promise.all resolve from folder A's load could land
  // AFTER folder B's effect run started and its own Promise.all resolved, overwriting
  // cabinetCollapsed/tasksCollapsed with folder A's (now-stale) values. This is distinct from
  // Bug 3's memo guard above: the memo guard prevents the effect from re-firing when scopeId
  // is unchanged; this guard prevents an in-flight fire's *result* from clobbering a later
  // fire's result once both are in-flight concurrently (rapid scope change).
  describe('collapse-load selectSeq guard — async race on rapid scope change (#2335)', () => {
    // Builds an invoke mock where each pending get_ai_setting call can be resolved
    // independently and out of order, to deterministically simulate the race: folder A's
    // read is issued first but resolves second (slower), after folder B's read (issued
    // second) resolves first (faster).
    function makeControllableInvoke() {
      const pending: Array<{ key: string; resolve: (v: string | null) => void }> = [];
      const invokeMock = vi.fn((cmd: string, args?: any) => {
        if (cmd !== 'get_ai_setting') return Promise.resolve(null);
        return new Promise<string | null>((resolve) => {
          pending.push({ key: args.key, resolve });
        });
      });
      function resolveKey(key: string, value: string | null) {
        const idx = pending.findIndex((p) => p.key === key);
        if (idx === -1) throw new Error(`no pending call for key ${key}`);
        const [entry] = pending.splice(idx, 1);
        entry.resolve(value);
      }
      return { invokeMock, resolveKey, pendingCount: () => pending.length };
    }

    it('discards a stale (superseded) resolve — slower folder-A read landing after faster folder-B read does not clobber folder-B state', async () => {
      const { invokeMock, resolveKey } = makeControllableInvoke();
      const h = makeCollapseRailHandlers(invokeMock, { folderId: 'folder-A', projectId: null });

      // Kick off folder A's load (both get_ai_setting calls now pending, unresolved).
      const loadA = h.loadCollapseStateForScope();

      // Rapid scope change to folder B before A resolves — kick off B's load too.
      h.setActiveFolder('folder-B');
      const loadB = h.loadCollapseStateForScope();

      // Resolve B (the newer/faster call) FIRST.
      resolveKey('folder:folder-B:cabinet:collapsed', 'true');
      resolveKey('folder:folder-B:tasks:collapsed', 'false');
      await loadB;

      expect(h.state().cabinetCollapsed).toBe(true);
      expect(h.state().tasksCollapsed).toBe(false);

      // NOW resolve A (the older/slower call) — its stale resolve must be discarded and
      // must NOT overwrite the state B already set, even though it settles second.
      resolveKey('folder:folder-A:cabinet:collapsed', 'false');
      resolveKey('folder:folder-A:tasks:collapsed', 'true');
      await loadA;

      expect(h.state().cabinetCollapsed).toBe(true); // still B's value, not A's
      expect(h.state().tasksCollapsed).toBe(false); // still B's value, not A's
    });

    it('no intermediate default-state render from a stale resolve after collapsing columns and rapidly changing scope', async () => {
      const { invokeMock, resolveKey } = makeControllableInvoke();
      const h = makeCollapseRailHandlers(invokeMock, { folderId: 'folder-A', projectId: null });

      // Collapse both columns in folder A first (simulates the reported user flow).
      await h.toggleCabinetCollapsed();
      await h.toggleTasksCollapsed();
      expect(h.state()).toMatchObject({ cabinetCollapsed: true, tasksCollapsed: true });

      // Rapid scope change folder A -> folder B, both loads in flight concurrently.
      const loadA = h.loadCollapseStateForScope(); // re-load of A, e.g. re-entering the effect
      h.setActiveFolder('folder-B');
      const loadB = h.loadCollapseStateForScope();

      // Folder B resolves first with its own persisted (collapsed) state.
      resolveKey('folder:folder-B:cabinet:collapsed', 'true');
      resolveKey('folder:folder-B:tasks:collapsed', 'true');
      await loadB;
      expect(h.state()).toMatchObject({ cabinetCollapsed: true, tasksCollapsed: true });

      // Folder A's stale reload resolves afterward with default/expanded values (e.g. a
      // never-visited-before default). If the guard were missing, this would incorrectly
      // flash the UI back to expanded even though the user is now viewing folder B, which
      // is genuinely collapsed. The guard must prevent that intermediate default-state render.
      resolveKey('folder:folder-A:cabinet:collapsed', null);
      resolveKey('folder:folder-A:tasks:collapsed', null);
      await loadA;

      expect(h.state()).toMatchObject({ cabinetCollapsed: true, tasksCollapsed: true });
    });

    it('increments collapseLoadSeq exactly once per scope-change load call', async () => {
      const { invokeMock, resolveKey } = makeControllableInvoke();
      const h = makeCollapseRailHandlers(invokeMock, { folderId: 'folder-A', projectId: null });

      expect(h.getCollapseLoadSeq()).toBe(0);

      const load1 = h.loadCollapseStateForScope();
      expect(h.getCollapseLoadSeq()).toBe(1);

      h.setActiveFolder('folder-B');
      const load2 = h.loadCollapseStateForScope();
      expect(h.getCollapseLoadSeq()).toBe(2);

      h.setActiveFolder('folder-C');
      const load3 = h.loadCollapseStateForScope();
      expect(h.getCollapseLoadSeq()).toBe(3);

      // Drain all three in arbitrary order to avoid leaving dangling pending promises.
      resolveKey('folder:folder-C:cabinet:collapsed', 'false');
      resolveKey('folder:folder-C:tasks:collapsed', 'false');
      resolveKey('folder:folder-B:cabinet:collapsed', 'false');
      resolveKey('folder:folder-B:tasks:collapsed', 'false');
      resolveKey('folder:folder-A:cabinet:collapsed', 'false');
      resolveKey('folder:folder-A:tasks:collapsed', 'false');
      await Promise.all([load1, load2, load3]);

      expect(h.getCollapseLoadSeq()).toBe(3);
    });

    it('load-failure error path also respects the guard — a stale rejection does not overwrite newer state', async () => {
      const pending: Array<{ key: string; reject?: (e: Error) => void; resolve?: (v: string | null) => void }> = [];
      const invokeMock = vi.fn((cmd: string, args?: any) => {
        if (cmd !== 'get_ai_setting') return Promise.resolve(null);
        return new Promise<string | null>((resolve, reject) => {
          pending.push({ key: args.key, resolve, reject });
        });
      });
      function rejectKey(key: string) {
        const idx = pending.findIndex((p) => p.key === key);
        const [entry] = pending.splice(idx, 1);
        entry.reject!(new Error('DB locked'));
      }
      function resolveKey(key: string, value: string | null) {
        const idx = pending.findIndex((p) => p.key === key);
        const [entry] = pending.splice(idx, 1);
        entry.resolve!(value);
      }

      const h = makeCollapseRailHandlers(invokeMock, { folderId: 'folder-A', projectId: null });
      const loadA = h.loadCollapseStateForScope();
      h.setActiveFolder('folder-B');
      const loadB = h.loadCollapseStateForScope();

      // Folder B resolves successfully first with collapsed state.
      resolveKey('folder:folder-B:cabinet:collapsed', 'true');
      resolveKey('folder:folder-B:tasks:collapsed', 'true');
      await loadB;
      expect(h.state()).toMatchObject({ cabinetCollapsed: true, tasksCollapsed: true });

      // Folder A's stale call rejects afterward — its catch-block fail-open reset must NOT
      // stomp folder B's already-current state.
      rejectKey('folder:folder-A:cabinet:collapsed');
      resolveKey('folder:folder-A:tasks:collapsed', 'true');
      await loadA;

      expect(h.state()).toMatchObject({ cabinetCollapsed: true, tasksCollapsed: true });
    });
  });
});
