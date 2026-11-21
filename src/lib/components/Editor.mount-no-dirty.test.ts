/**
 * Editor.mount-no-dirty.test.ts — #2950
 *
 * ROOT CAUSE (second, independent trigger found while fixing #2950):
 *   Editor.svelte has a `$effect` that runs on every mount (and whenever
 *   `isReadOnly` changes):
 *
 *     $effect(() => {
 *       if (editor) {
 *         editor.setEditable(!isReadOnly);
 *       }
 *     });
 *
 *   TipTap's `Editor.setEditable(editable, emitUpdate = true)` unconditionally
 *   emits an `update` event whenever `emitUpdate` is left at its default —
 *   with an EMPTY transaction (`this.state.tr`, zero steps). This does not
 *   go through `dispatchTransaction`/`docChanged`/`preventUpdate` at all; it's
 *   a direct `this.emit('update', ...)` call inside `setEditable` itself.
 *
 *   Because this $effect fires on every single Editor mount — completely
 *   independent of attachments, images, or any NodeView — it was tripping
 *   Editor.svelte's `onUpdate` handler and setting `dirty = true` on every
 *   note opened, even a plain note with no images at all. 500ms later the
 *   debounced autosave fired `api.saveNote()` and dispatched
 *   `pane-data-changed`, purely from opening the note.
 *
 * THE FIX:
 *   `editor.setEditable(!isReadOnly, false)` — pass `emitUpdate: false`.
 *   Toggling editable state is never itself a content change, so it should
 *   never mark the editor dirty. This is the same category of fix as the
 *   `skipDirty` mechanism used for AttachmentImageInline's auto-persist-on-
 *   load path (see Editor.attachment-onload-dirty.test.ts), applied at the
 *   one other call site that unconditionally emits a TipTap `update` event
 *   on every mount.
 *
 * INVARIANT UNDER TEST: mounting a note, with no user edits and no
 * attachments at all, never saves.
 *
 * This mounts the REAL Editor.svelte (same pattern as
 * Editor.attachment-onload-dirty.test.ts / Editor.sheet-cell-editing-
 * integration.test.ts). Only `$lib/api` is mocked.
 */

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render } from '@testing-library/svelte/svelte5';
import { tick } from 'svelte';
import Editor from './Editor.svelte';
import type { NoteMeta } from '$lib/types';

// vitest-setup.ts replaces globalThis.window with a plain mock object
// lacking addEventListener/removeEventListener/DOMParser/setTimeout/
// ResizeObserver/document. Bind them through to the real jsdom globals —
// same polyfill pattern used by Editor.attachment-onload-dirty.test.ts.
const win = globalThis.window as any;
if (typeof win.addEventListener !== 'function') {
  win.addEventListener = globalThis.addEventListener?.bind(globalThis) ?? (() => {});
}
if (typeof win.removeEventListener !== 'function') {
  win.removeEventListener = globalThis.removeEventListener?.bind(globalThis) ?? (() => {});
}
if (typeof win.dispatchEvent !== 'function') {
  win.dispatchEvent = globalThis.dispatchEvent?.bind(globalThis) ?? (() => true);
}
if (!win.DOMParser && typeof DOMParser !== 'undefined') {
  win.DOMParser = DOMParser;
}
if (!win.setTimeout && typeof setTimeout !== 'undefined') {
  win.setTimeout = setTimeout;
}
if (!win.clearTimeout && typeof clearTimeout !== 'undefined') {
  win.clearTimeout = clearTimeout;
}
if (typeof (globalThis as any).ResizeObserver === 'undefined') {
  (globalThis as any).ResizeObserver = class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
}
if (typeof win.ResizeObserver === 'undefined') {
  win.ResizeObserver = (globalThis as any).ResizeObserver;
}
if (typeof win.document === 'undefined' && typeof document !== 'undefined') {
  win.document = document;
}

const NOTE_ID = 'plain-note';

const { saveNoteMock } = vi.hoisted(() => ({
  saveNoteMock: vi.fn(),
}));

vi.mock('$lib/api', () => ({
  saveNote: saveNoteMock,
  resolveAttachmentSrc: vi.fn().mockResolvedValue('asset://x'),
  listTasksByNote: vi.fn().mockResolvedValue([]),
  getBacklinks: vi.fn().mockResolvedValue([]),
  fetchLinkPreview: vi.fn().mockResolvedValue(null),
  getCachedLinkPreview: vi.fn().mockResolvedValue(null),
  readNote: vi.fn().mockResolvedValue(null),
  rebuildNoteLinks: vi.fn().mockResolvedValue(undefined),
  listFolders: vi.fn().mockResolvedValue([]),
}));

function makeNoteMeta(overrides: Partial<NoteMeta> = {}): NoteMeta {
  return {
    id: NOTE_ID,
    title: 'Plain note',
    notebook: 'Notes',
    slug: 'plain-note',
    created_at: '2026-01-01T00:00:00Z',
    updated_at: '2026-01-01T00:00:00Z',
    preview: '',
    ...overrides,
  };
}

const PLAIN_MARKDOWN = `# Plain note

Just some text, no attachments at all.
`;

async function flushAsync() {
  await Promise.resolve();
  await tick();
  await tick();
}

async function advanceAndFlush(totalMs: number, stepMs = 50) {
  let waited = 0;
  while (waited < totalMs) {
    await new Promise((resolve) => setTimeout(resolve, stepMs));
    await flushAsync();
    waited += stepMs;
  }
}

beforeEach(() => {
  saveNoteMock.mockReset();
  saveNoteMock.mockResolvedValue(makeNoteMeta());
});

afterEach(() => {
  vi.useRealTimers();
});

describe('#2950 — mounting an Editor never marks it dirty on its own', () => {
  it('plain note with no attachments and no user edits does NOT call api.saveNote()', async () => {
    const dataChangedSpy = vi.fn();
    window.addEventListener('pane-data-changed', dataChangedSpy);

    render(Editor, {
      props: {
        content: PLAIN_MARKDOWN,
        title: 'Plain note',
        slug: 'plain-note',
        noteId: NOTE_ID,
        noteMeta: makeNoteMeta(),
        currentProject: 'Notes',
      },
    });

    await flushAsync();
    // Give the setEditable $effect and any (incorrectly) pending debounced
    // save a chance to fire.
    await advanceAndFlush(600);

    expect(saveNoteMock).not.toHaveBeenCalled();
    expect(dataChangedSpy).not.toHaveBeenCalled();

    window.removeEventListener('pane-data-changed', dataChangedSpy);
  });

  it('a read-only system note (isReadOnly transitions editable off) does NOT call api.saveNote()', async () => {
    // note_type in SYSTEM_NOTE_TYPES makes isReadOnly derive to true, so the
    // setEditable $effect runs with editable=false on mount instead of the
    // default true — exercises the same code path with the opposite value.
    render(Editor, {
      props: {
        content: PLAIN_MARKDOWN,
        title: 'Soul note',
        slug: 'soul',
        noteId: 'soul-note',
        noteMeta: makeNoteMeta({ id: 'soul-note', slug: 'soul', note_type: 'soul' } as any),
        currentProject: 'Notes',
      },
    });

    await flushAsync();
    await advanceAndFlush(600);

    expect(saveNoteMock).not.toHaveBeenCalled();
  });
});
