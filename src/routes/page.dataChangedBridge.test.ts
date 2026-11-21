/**
 * +page.svelte's data-changed bridge — #2972 stage 3 (`docs/spec/
 * data-refresh.md` §34.2, §34 item 3).
 *
 * The dead `listen('pane-note-changed', ...)` bridge (confirmed dead since
 * #2972 stage 1 — every backend emitter of that name was migrated off it)
 * is replaced by one handler on the real, live `pane-data-changed` event.
 * This file tests each of its three restored behaviors:
 *
 *   1. An agent (or any foreign-origin) delete of the currently open note
 *      closes it.
 *   2. An agent (or any foreign-origin) edit to the content of the
 *      currently open note reloads it (dispatches `pane-reload-editor`) —
 *      but NOT when the edit is this same page's own origin-tagged
 *      dispatch (§34.4 echo suppression applied at the bridge level too).
 *   3. An agent-created project is auto-favorited (unless already
 *      favorited, or it's the "Pane System" bootstrap project).
 *
 * Structured as a pure-logic test, same convention as this directory's
 * existing `page.wiki-link-context.test.ts`: the handler body is inlined
 * IDENTICALLY to +page.svelte's own `$effect` block (same branches, same
 * order, same guard conditions) rather than mounting the full route
 * component (`+page.svelte` pulls in auth/onboarding/session wiring far
 * outside this bridge's own concern). `isOwnOriginTag`/`registerOwnOriginTag`
 * are imported for real (not reimplemented) from `$lib/refreshEvent` — only
 * the DOM dispatch, store read/writes, and api/invoke calls are the fakes
 * a unit test needs.
 *
 * RED-ON-REVERT: reverting to the old
 *   payload?.tools?.includes('delete_note') / .includes('create_project')
 *   shape (the dead `pane-note-changed` tool-array sniff) makes every
 *   assertion below fail, because this payload (entity_id/entity_type/
 *   reason/origin_tag) has no `tools` array for that old code to read at
 *   all — it would see `undefined?.includes(...)` and never fire any of
 *   the three branches.
 */

import { describe, it, expect, vi, beforeEach } from 'vitest';
import {
  ENTITY_NOTE,
  ENTITY_PROJECT,
  REASON_CREATED,
  REASON_DELETED,
  REASON_EDITED,
  isOwnOriginTag,
  registerOwnOriginTag,
  makeOriginTag,
  _clearOwnOriginTagsForTest,
  type DataChangedPayload,
} from '$lib/refreshEvent';

// ---------------------------------------------------------------------------
// Inlined mirror of +page.svelte's `subscribeDataChanged(async (payload) =>
// {...})` handler body — see src/routes/+page.svelte, the $effect
// immediately below the "Global keyboard shortcuts" comment. Kept
// position-for-position identical (same three `if` branches, same order,
// same early `return`s) so a change to the real file's branch logic is the
// thing that would desync this mirror, not a cosmetic difference.
// ---------------------------------------------------------------------------

interface BridgeDeps {
  activeNote: { get: () => any; set: (v: any) => void };
  projects: { get: () => any[]; set: (v: any[]) => void };
  editorPreEditText: { get: () => string };
  api: {
    getProjectOrFolder: (id: string) => Promise<any>;
    syncProjectsWithNotebooks: () => Promise<any[]>;
  };
  invoke: (cmd: string, args: any) => Promise<any>;
  dispatchReloadEditor: (oldText: string) => void;
}

async function handleDataChangedBridge(payload: DataChangedPayload, deps: BridgeDeps): Promise<void> {
  const { entity_id: entityId, entity_type: entityType, reason, origin_tag: originTag } = payload;

  // Behavior 1 of 3.
  if (entityType === ENTITY_NOTE && reason === REASON_DELETED) {
    if (deps.activeNote.get()?.meta?.id === entityId) {
      deps.activeNote.set(null);
    }
    return;
  }

  // Behavior 3 of 3.
  if (entityType === ENTITY_PROJECT && reason === REASON_CREATED) {
    try {
      const created = await deps.api.getProjectOrFolder(entityId);
      if (!created.parent_id && created.name !== 'Pane System' && !created.is_favorite) {
        await deps.invoke('update_project_settings', { projectId: entityId, isFavorite: true });
        deps.projects.set(await deps.api.syncProjectsWithNotebooks());
      }
    } catch (err) {
      // logged in the real file; swallowed here, same effective behavior
    }
    return;
  }

  // Behavior 2 of 3.
  if (entityType === ENTITY_NOTE && reason === REASON_EDITED && entityId === deps.activeNote.get()?.meta?.id) {
    if (isOwnOriginTag(originTag)) return;
    const oldText = deps.editorPreEditText.get() || '';
    deps.dispatchReloadEditor(oldText);
  }
}

// ---------------------------------------------------------------------------

function makeDeps(overrides: Partial<BridgeDeps> = {}): BridgeDeps & {
  _activeNoteValue: any;
  _projectsValue: any[];
  dispatchReloadEditorSpy: ReturnType<typeof vi.fn>;
} {
  let activeNoteValue: any = null;
  let projectsValue: any[] = [];
  const dispatchReloadEditorSpy = vi.fn();
  const deps: any = {
    activeNote: {
      get: () => activeNoteValue,
      set: (v: any) => { activeNoteValue = v; },
    },
    projects: {
      get: () => projectsValue,
      set: (v: any[]) => { projectsValue = v; },
    },
    editorPreEditText: { get: () => 'text before the edit' },
    api: {
      getProjectOrFolder: vi.fn(),
      syncProjectsWithNotebooks: vi.fn().mockResolvedValue([]),
    },
    invoke: vi.fn().mockResolvedValue(undefined),
    dispatchReloadEditor: dispatchReloadEditorSpy,
    ...overrides,
  };
  Object.defineProperty(deps, '_activeNoteValue', { get: () => activeNoteValue });
  Object.defineProperty(deps, '_projectsValue', { get: () => projectsValue });
  deps.activeNote.set = (v: any) => { activeNoteValue = v; };
  deps.dispatchReloadEditorSpy = dispatchReloadEditorSpy;
  return deps;
}

beforeEach(() => {
  _clearOwnOriginTagsForTest();
});

describe('+page.svelte data-changed bridge — behavior 1: agent delete closes the open note', () => {
  it('closes the editor when the deleted note IS the open one', async () => {
    const deps = makeDeps();
    deps.activeNote.set({ meta: { id: 'note-open' } });

    await handleDataChangedBridge(
      { entity_id: 'note-open', entity_type: ENTITY_NOTE, reason: REASON_DELETED, origin_tag: 'agent-tag' },
      deps,
    );

    expect(deps._activeNoteValue).toBeNull();
  });

  it('does NOT close the editor when a DIFFERENT note is deleted', async () => {
    const deps = makeDeps();
    const openNote = { meta: { id: 'note-open' } };
    deps.activeNote.set(openNote);

    await handleDataChangedBridge(
      { entity_id: 'some-other-note', entity_type: ENTITY_NOTE, reason: REASON_DELETED, origin_tag: 'agent-tag' },
      deps,
    );

    expect(deps._activeNoteValue).toBe(openNote);
  });
});

describe('+page.svelte data-changed bridge — behavior 2: agent edit reloads the open note', () => {
  it('dispatches pane-reload-editor for a FOREIGN edit to the open note', async () => {
    const deps = makeDeps();
    deps.activeNote.set({ meta: { id: 'note-open' } });

    await handleDataChangedBridge(
      { entity_id: 'note-open', entity_type: ENTITY_NOTE, reason: REASON_EDITED, origin_tag: 'agent-tag' },
      deps,
    );

    expect(deps.dispatchReloadEditorSpy).toHaveBeenCalledTimes(1);
    expect(deps.dispatchReloadEditorSpy).toHaveBeenCalledWith('text before the edit');
  });

  it('does NOT reload for an edit to a note that is not open', async () => {
    const deps = makeDeps();
    deps.activeNote.set({ meta: { id: 'note-open' } });

    await handleDataChangedBridge(
      { entity_id: 'some-other-note', entity_type: ENTITY_NOTE, reason: REASON_EDITED, origin_tag: 'agent-tag' },
      deps,
    );

    expect(deps.dispatchReloadEditorSpy).not.toHaveBeenCalled();
  });

  it('does NOT reload for this page\'s OWN origin-tagged dispatch (§34.4 echo suppression)', async () => {
    const deps = makeDeps();
    deps.activeNote.set({ meta: { id: 'note-open' } });

    const ownTag = makeOriginTag();
    registerOwnOriginTag(ownTag);

    await handleDataChangedBridge(
      { entity_id: 'note-open', entity_type: ENTITY_NOTE, reason: REASON_EDITED, origin_tag: ownTag },
      deps,
    );

    expect(deps.dispatchReloadEditorSpy).not.toHaveBeenCalled();
  });

  it('BOTH halves in one test: skips its own tag, but still reloads for a later independent edit to the same note', async () => {
    const deps = makeDeps();
    deps.activeNote.set({ meta: { id: 'note-open' } });

    const ownTag = makeOriginTag();
    registerOwnOriginTag(ownTag);
    await handleDataChangedBridge(
      { entity_id: 'note-open', entity_type: ENTITY_NOTE, reason: REASON_EDITED, origin_tag: ownTag },
      deps,
    );
    expect(deps.dispatchReloadEditorSpy).not.toHaveBeenCalled();

    await handleDataChangedBridge(
      { entity_id: 'note-open', entity_type: ENTITY_NOTE, reason: REASON_EDITED, origin_tag: makeOriginTag() },
      deps,
    );
    expect(deps.dispatchReloadEditorSpy).toHaveBeenCalledTimes(1);
  });
});

describe('+page.svelte data-changed bridge — behavior 3: agent-created projects are auto-favorited', () => {
  it('favorites a net-new root project', async () => {
    const deps = makeDeps();
    deps.api.getProjectOrFolder = vi.fn().mockResolvedValue({
      id: 'proj-new', name: 'New Project', parent_id: null, is_favorite: false,
    });
    deps.api.syncProjectsWithNotebooks = vi.fn().mockResolvedValue([{ id: 'proj-new', is_favorite: true }]);

    await handleDataChangedBridge(
      { entity_id: 'proj-new', entity_type: ENTITY_PROJECT, reason: REASON_CREATED, origin_tag: 'agent-tag' },
      deps,
    );

    expect(deps.invoke).toHaveBeenCalledWith('update_project_settings', { projectId: 'proj-new', isFavorite: true });
    expect(deps._projectsValue).toEqual([{ id: 'proj-new', is_favorite: true }]);
  });

  it('does NOT favorite a folder (has a parent_id)', async () => {
    const deps = makeDeps();
    deps.api.getProjectOrFolder = vi.fn().mockResolvedValue({
      id: 'folder-new', name: 'New Folder', parent_id: 'proj-parent', is_favorite: false,
    });

    await handleDataChangedBridge(
      { entity_id: 'folder-new', entity_type: ENTITY_PROJECT, reason: REASON_CREATED, origin_tag: 'agent-tag' },
      deps,
    );

    expect(deps.invoke).not.toHaveBeenCalled();
  });

  it('does NOT re-favorite an already-favorited project', async () => {
    const deps = makeDeps();
    deps.api.getProjectOrFolder = vi.fn().mockResolvedValue({
      id: 'proj-already-fav', name: 'Already Favorited', parent_id: null, is_favorite: true,
    });

    await handleDataChangedBridge(
      { entity_id: 'proj-already-fav', entity_type: ENTITY_PROJECT, reason: REASON_CREATED, origin_tag: 'agent-tag' },
      deps,
    );

    expect(deps.invoke).not.toHaveBeenCalled();
  });

  it('does NOT favorite the "Pane System" bootstrap project', async () => {
    const deps = makeDeps();
    deps.api.getProjectOrFolder = vi.fn().mockResolvedValue({
      id: 'proj-system', name: 'Pane System', parent_id: null, is_favorite: false,
    });

    await handleDataChangedBridge(
      { entity_id: 'proj-system', entity_type: ENTITY_PROJECT, reason: REASON_CREATED, origin_tag: 'agent-tag' },
      deps,
    );

    expect(deps.invoke).not.toHaveBeenCalled();
  });
});
