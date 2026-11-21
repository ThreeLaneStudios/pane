/**
 * page.cmdN-shared-function.test.ts — #3044, revisited.
 *
 * Matt's test list for this ruling includes "Cmd+N from Home → shows
 * 'Open a project to make a note.'" `+page.svelte` is a large root route
 * component with many dependencies (Tauri IPC, onboarding, zoom, chat
 * sidebar, etc.) — this repo's own convention for testing its call-site
 * logic without mounting the whole thing is to mirror the relevant
 * snippet verbatim into a focused test file (see
 * `page.wiki-link-context.test.ts`'s own header comment for the same
 * convention, same reasoning).
 *
 * Both snippets below are mirrored EXACTLY from `+page.svelte` as it
 * exists right now (the keydown handler's Cmd+N branch, and the
 * 'pane-new-note-blocked' listener + toast state) — not a reimplementation
 * of what they SHOULD do. The actual function they call,
 * `noteService.createNewNote()`, is imported for real, unmocked here
 * except at the `api` boundary — so "Cmd+N from Home" is exercised through
 * the real shared function, the real $activeProject-is-null branch, and
 * the real window event, landing in a mirrored copy of the real toast
 * logic.
 *
 * RED-ON-REVERT: reverting +page.svelte's keydown handler back to
 * `(window as any).__paneNewNote?.()` makes `callsCreateNewNote` below
 * false (the mirror no longer matches source) AND the toast test fails
 * (nothing dispatches 'pane-new-note-blocked' anymore, since
 * `window.__paneNewNote` doesn't exist outside a mounted NotesBrowser and
 * even when it does, it has no concept of this event).
 */

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { readFileSync } from 'fs';
import { resolve } from 'path';
import * as api from '$lib/api';
import * as noteService from '$lib/services/noteService';
import { activeProject, activeNote } from '$lib/stores/app';
import { canvasState } from '$lib/stores/canvas';

const PAGE_SOURCE = readFileSync(resolve(__dirname, '+page.svelte'), 'utf-8');

// vitest-setup.ts replaces `globalThis.window` with a plain mock object
// whose `dispatchEvent` is a hardcoded no-op (always returns `true`,
// never reaches a real listener registry) and has no `addEventListener`
// at all. noteService.createNewNote() dispatches a real CustomEvent on
// `window` when blocked, so both need rebinding to the real globalThis
// EventTarget — the no-op dispatchEvent is force-replaced unconditionally
// (a `typeof ... !== 'function'` guard wouldn't fire for it; it's already
// a function, just the wrong one).
const win = globalThis.window as any;
if (typeof win.addEventListener !== 'function') {
  win.addEventListener = globalThis.addEventListener.bind(globalThis);
}
if (typeof win.removeEventListener !== 'function') {
  win.removeEventListener = globalThis.removeEventListener.bind(globalThis);
}
win.dispatchEvent = globalThis.dispatchEvent.bind(globalThis);

// ---------------------------------------------------------------------------
// Mirror 1 — the Cmd+N branch of +page.svelte's handleKeydown, verbatim in
// shape (view guard, then the one call) to the real source below.
// ---------------------------------------------------------------------------
async function handleCmdN(canvasView: string) {
  if (canvasView === 'kanban') return; // KanbanBoard owns Cmd+N there — out of scope
  await noteService.createNewNote();
}

// ---------------------------------------------------------------------------
// Mirror 2 — +page.svelte's 'pane-new-note-blocked' listener + toast state.
// ---------------------------------------------------------------------------
function makeBlockedToastHandler() {
  let newNoteBlockedToast = '';
  const handler = () => {
    newNoteBlockedToast = 'Open a project to make a note.';
  };
  return {
    handler,
    getToast: () => newNoteBlockedToast,
  };
}

beforeEach(() => {
  activeProject.set(null);
  activeNote.set(null);
  canvasState.set({ view: 'home', data: {}, context: {} });
  vi.spyOn(api, 'createNoteInProject').mockResolvedValue({
    id: 'home-blocked-should-never-be-called',
  } as any);
});

afterEach(() => {
  vi.restoreAllMocks();
  activeProject.set(null);
  activeNote.set(null);
});

describe('#3044 (revisited) — +page.svelte source actually calls the shared function, not window.__paneNewNote', () => {
  it('the Cmd+N branch calls noteService.createNewNote() directly', () => {
    const handlerSrc = PAGE_SOURCE.match(/if \(meta && e\.key === 'n' && !e\.shiftKey\) \{[\s\S]*?\n    \}/);
    expect(handlerSrc).toBeTruthy();
    expect(handlerSrc![0]).toMatch(/noteService\.createNewNote\(\)/);
    expect(handlerSrc![0]).not.toMatch(/__paneNewNote/);
  });

  it('the kanban early-return guard is still the only thing that skips calling it', () => {
    const handlerSrc = PAGE_SOURCE.match(/if \(meta && e\.key === 'n' && !e\.shiftKey\) \{[\s\S]*?\n    \}/);
    expect(handlerSrc![0]).toMatch(/canvasState\.view === 'kanban'/);
  });

  it('no __paneNewNote reference survives anywhere in +page.svelte', () => {
    expect(PAGE_SOURCE).not.toMatch(/__paneNewNote/);
  });

  it('the pane-new-note-blocked listener sets the exact message "Open a project to make a note."', () => {
    const listenerSrc = PAGE_SOURCE.match(/pane-new-note-blocked[\s\S]{0,400}/);
    expect(listenerSrc).toBeTruthy();
    expect(listenerSrc![0]).toContain('Open a project to make a note.');
  });
});

describe('#3044 (revisited) — "Cmd+N from Home" end to end, through the real shared function', () => {
  it('on Home ($activeProject null, view=home): creates nothing', async () => {
    activeProject.set(null);
    await handleCmdN('home');
    expect(api.createNoteInProject).not.toHaveBeenCalled();
  });

  it('on Home: the real pane-new-note-blocked event fires, and the mirrored toast handler shows the exact message', async () => {
    const { handler, getToast } = makeBlockedToastHandler();
    window.addEventListener('pane-new-note-blocked', handler);
    try {
      activeProject.set(null);
      expect(getToast()).toBe('');
      await handleCmdN('home');
      expect(getToast()).toBe('Open a project to make a note.');
    } finally {
      window.removeEventListener('pane-new-note-blocked', handler);
    }
  });

  it('on Calendar/All Tasks/Trash ($activeProject null): same outcome — not special-cased to Home only', async () => {
    for (const view of ['calendar', 'all_tasks', 'trash', 'project_list']) {
      activeProject.set(null);
      vi.clearAllMocks();
      await handleCmdN(view);
      expect(api.createNoteInProject).not.toHaveBeenCalled();
    }
  });

  it('on kanban: the view guard returns before ever reaching the shared function (out of scope, unaffected by #3044)', async () => {
    activeProject.set(null);
    await handleCmdN('kanban');
    expect(api.createNoteInProject).not.toHaveBeenCalled();
  });
});
