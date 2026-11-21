/**
 * Editor.attachment-onload-dirty.test.ts — #2950
 *
 * ROOT CAUSE:
 *   AttachmentImageInline.svelte's handleImageLoad() fires
 *   onUpdateAttributes({ width, height }) from the native <img> `load` event
 *   whenever width===null && height===null (i.e. natural dims were never
 *   persisted). That event is inherently async relative to mount — it can
 *   fire AFTER Editor.svelte's `isSettingContent` flag has already been reset
 *   to `false` by the initial `applyMarkdownContent()` call.
 *
 *   Because `isSettingContent === false` by the time the attribute write
 *   lands, it was NOT suppressed: it flowed through Attachment.ts's
 *   `updateAttrs` → a real `tr.setNodeMarkup(...)` transaction with
 *   `docChanged: true` → TipTap's `onUpdate` callback → Editor.svelte set
 *   `dirty = true` → the debounced autosave fired `api.saveNote()` and
 *   dispatched `pane-data-changed`, even though the user never touched
 *   the note.
 *
 * THE FIX:
 *   AttachmentImageInline's auto-persist-on-load call now passes
 *   `{ skipDirty: true }` through to Attachment.ts's `updateAttrs`, which
 *   tags the transaction with TipTap's own `preventUpdate` meta key (the
 *   same mechanism TipTap's `setContent({ emitUpdate: false })` uses
 *   internally). That meta key makes TipTap skip emitting the `update`
 *   event for this specific transaction, so Editor.svelte's `onUpdate`
 *   handler (which sets `dirty = true`) never runs for it. The document
 *   still updates and the NodeView still re-renders with the persisted
 *   dims — only the dirty-marking is suppressed. Real user edits (drag-
 *   resize, render-mode change) do NOT pass skipDirty, so they still mark
 *   dirty and save normally.
 *
 * INVARIANT UNDER TEST: a note with no user edits never saves.
 *
 * These tests mount the REAL Editor.svelte (same pattern as
 * Editor.sheet-cell-editing-integration.test.ts / Attachment.factory-
 * integration.test.ts) with a note whose markdown contains one inline image
 * lacking persisted width/height. Only `$lib/api` is mocked — the real
 * TipTap Editor, real Attachment extension, real AttachmentNodeView, and
 * real AttachmentImageInline all run.
 */

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, fireEvent } from '@testing-library/svelte/svelte5';
import { tick } from 'svelte';
import Editor from './Editor.svelte';
import type { NoteMeta } from '$lib/types';

// vitest-setup.ts replaces globalThis.window with a plain mock object
// lacking addEventListener/removeEventListener/DOMParser/setTimeout/
// ResizeObserver. Bind them through to the real jsdom global — same
// polyfill pattern used by Attachment.factory-integration.test.ts /
// Editor.sheet-cell-editing-integration.test.ts.
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
// TipTap's elementFromString() calls `new window.DOMParser()` when
// Editor.svelte constructs the TipTap Editor with initial content.
if (!win.DOMParser && typeof DOMParser !== 'undefined') {
  win.DOMParser = DOMParser;
}
if (!win.setTimeout && typeof setTimeout !== 'undefined') {
  win.setTimeout = setTimeout;
}
if (!win.clearTimeout && typeof clearTimeout !== 'undefined') {
  win.clearTimeout = clearTimeout;
}
// Editor.svelte's toolbar-overflow $effect observes toolbarEl with a
// ResizeObserver — not implemented in jsdom. Stub a no-op; irrelevant to
// the #2950 dirty-flag behavior under test.
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
// prosemirror-model's DOMSerializer falls back to `window.document` when no
// explicit `document` option is passed (see serializeFragment -> doc()).
// vitest-setup.ts's window mock has no `document` property, so
// editor.getHTML() (called from Editor.svelte's debounced onUpdate handler)
// throws "Cannot read properties of undefined (reading
// 'createDocumentFragment')" as an unhandled rejection once that 500ms
// timer fires after mount, unrelated to anything under test here.
if (typeof win.document === 'undefined' && typeof document !== 'undefined') {
  win.document = document;
}

const NOTE_ID = 'note-with-image';

const {
  saveNoteMock,
  resolveAttachmentSrcMock,
  listTasksByNoteMock,
  getBacklinksMock,
} = vi.hoisted(() => ({
  saveNoteMock: vi.fn(),
  resolveAttachmentSrcMock: vi.fn(),
  listTasksByNoteMock: vi.fn(),
  getBacklinksMock: vi.fn(),
}));

vi.mock('$lib/api', () => ({
  saveNote: saveNoteMock,
  resolveAttachmentSrc: resolveAttachmentSrcMock,
  listTasksByNote: listTasksByNoteMock,
  getBacklinks: getBacklinksMock,
  // Editor.svelte references these but they're not exercised by this test —
  // stub them so any incidental call doesn't throw.
  fetchLinkPreview: vi.fn().mockResolvedValue(null),
  getCachedLinkPreview: vi.fn().mockResolvedValue(null),
  readNote: vi.fn().mockResolvedValue(null),
  rebuildNoteLinks: vi.fn().mockResolvedValue(undefined),
  listFolders: vi.fn().mockResolvedValue([]),
}));

function makeNoteMeta(overrides: Partial<NoteMeta> = {}): NoteMeta {
  return {
    id: NOTE_ID,
    title: 'Note with image',
    notebook: 'Notes',
    slug: 'note-with-image',
    created_at: '2026-01-01T00:00:00Z',
    updated_at: '2026-01-01T00:00:00Z',
    preview: '',
    ...overrides,
  };
}

// Markdown containing one inline image whose natural dims were never
// persisted (no width/height query params or attrs anywhere) — the exact
// shape that trips handleImageLoad()'s auto-persist path.
const MARKDOWN_WITH_UNSIZED_IMAGE = `# Note with image

![photo.jpg](attachment://img-attachment-001)
`;

async function flushAsync() {
  await Promise.resolve();
  await tick();
  await tick();
}

// Advances real timers in small increments while flushing microtasks/Svelte
// ticks between each step, so the 500ms save-debounce timer in Editor.svelte
// gets a chance to fire (if it incorrectly would) without deadlocking against
// the async chain used by handleImageLoad/resolveAttachmentSrc.
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
  resolveAttachmentSrcMock.mockReset();
  listTasksByNoteMock.mockReset();
  getBacklinksMock.mockReset();
  saveNoteMock.mockResolvedValue(makeNoteMeta());
  resolveAttachmentSrcMock.mockResolvedValue('asset://localhost/attachments/img-attachment-001.jpg');
  listTasksByNoteMock.mockResolvedValue([]);
  getBacklinksMock.mockResolvedValue([]);
});

afterEach(() => {
  vi.useRealTimers();
});

describe('#2950 — AttachmentImageInline onload must not mark editor dirty', () => {
  it('image load with null dims does NOT call api.saveNote()', async () => {
    const dataChangedSpy = vi.fn();
    window.addEventListener('pane-data-changed', dataChangedSpy);

    const { container } = render(Editor, {
      props: {
        content: MARKDOWN_WITH_UNSIZED_IMAGE,
        title: 'Note with image',
        slug: 'note-with-image',
        noteId: NOTE_ID,
        noteMeta: makeNoteMeta(),
        currentProject: 'Notes',
      },
    });

    await flushAsync();

    // Locate the inline <img> the NodeView mounted and fire its native load event
    // with natural dimensions — the exact trigger for handleImageLoad()'s
    // auto-persist path.
    const img = container.querySelector('.at-image-inline img') as HTMLImageElement | null;
    expect(img).not.toBeNull();

    Object.defineProperty(img!, 'naturalWidth', { value: 800, configurable: true });
    Object.defineProperty(img!, 'naturalHeight', { value: 600, configurable: true });
    await fireEvent.load(img!);
    await flushAsync();

    // Editor's onUpdate debounces the save by 500ms. Advance past that window
    // and flush again to give any (incorrect) pending save a chance to fire.
    await advanceAndFlush(600);

    expect(saveNoteMock).not.toHaveBeenCalled();
    expect(dataChangedSpy).not.toHaveBeenCalled();

    window.removeEventListener('pane-data-changed', dataChangedSpy);
  });

  it('image load with null dims does NOT dispatch pane-data-changed on window', async () => {
    const dataChangedSpy = vi.fn();
    window.addEventListener('pane-data-changed', dataChangedSpy);

    const { container } = render(Editor, {
      props: {
        content: MARKDOWN_WITH_UNSIZED_IMAGE,
        title: 'Note with image',
        slug: 'note-with-image',
        noteId: NOTE_ID,
        noteMeta: makeNoteMeta(),
        currentProject: 'Notes',
      },
    });

    await flushAsync();

    const img = container.querySelector('.at-image-inline img') as HTMLImageElement | null;
    expect(img).not.toBeNull();

    Object.defineProperty(img!, 'naturalWidth', { value: 1024, configurable: true });
    Object.defineProperty(img!, 'naturalHeight', { value: 768, configurable: true });
    await fireEvent.load(img!);
    await flushAsync();
    await advanceAndFlush(600);

    expect(dataChangedSpy).not.toHaveBeenCalled();

    window.removeEventListener('pane-data-changed', dataChangedSpy);
  });

  it('persists natural dims into the TipTap doc even though the write is dirty-suppressed', async () => {
    // Confirms the fix doesn't silently drop the persist — the document
    // attrs update must still happen; only the dirty-marking is suppressed.
    const { container } = render(Editor, {
      props: {
        content: MARKDOWN_WITH_UNSIZED_IMAGE,
        title: 'Note with image',
        slug: 'note-with-image',
        noteId: NOTE_ID,
        noteMeta: makeNoteMeta(),
        currentProject: 'Notes',
      },
    });

    await flushAsync();

    const img = container.querySelector('.at-image-inline img') as HTMLImageElement | null;
    expect(img).not.toBeNull();
    Object.defineProperty(img!, 'naturalWidth', { value: 800, configurable: true });
    Object.defineProperty(img!, 'naturalHeight', { value: 600, configurable: true });
    await fireEvent.load(img!);
    await flushAsync();

    // The NodeView re-renders the container with the persisted width once the
    // node attrs update — containerStyle derives `width:${currentW}px` from
    // the (now non-null) width prop.
    const wrapper = container.querySelector('.at-image-inline') as HTMLElement | null;
    expect(wrapper).not.toBeNull();
    // jsdom normalizes the inline style string (adds a space after the colon).
    expect(wrapper!.getAttribute('style') ?? '').toMatch(/width:\s*800px/);

    await advanceAndFlush(600);

    // Still no save triggered by the persisted write.
    expect(saveNoteMock).not.toHaveBeenCalled();
  });
});

// ─── Case 2: drag resize STILL saves ───────────────────────────────────────
//
// The other half of the #2950 invariant: skipDirty must be scoped ONLY to
// the auto-persist-on-load path (handleImageLoad). A real user-initiated
// drag-resize (AttachmentImageInline.svelte's onDragEnd — see
// AttachmentImageInline.test.ts I-4) calls onUpdateAttributes({ width,
// height }) WITHOUT { skipDirty: true }, so it must still flow through
// Attachment.ts's updateAttrs as a normal transaction (no 'preventUpdate'
// meta), reach Editor.svelte's onUpdate handler, set dirty = true, and fire
// the debounced api.saveNote() call. If skipDirty ever leaked onto the
// drag-resize path (or onDragEnd's call site regressed to pass it), this
// test fails and catches the over-suppression before it ships.
describe('#2950 — drag resize still marks editor dirty and saves', () => {
  it('user-initiated drag resize calls api.saveNote() (dirty=true path still works)', async () => {
    // Note: unlike Case 1/3 above, this test does NOT assert on
    // 'pane-data-changed' — that event is dispatched only from the trash/
    // move-folder paths (see handleMoveToTrash/moveToFolder), never from the
    // normal onUpdate→saveContent debounced-save path exercised here. The
    // positive proof for THIS half of the invariant is api.saveNote() itself.
    const { container } = render(Editor, {
      props: {
        content: MARKDOWN_WITH_UNSIZED_IMAGE,
        title: 'Note with image',
        slug: 'note-with-image',
        noteId: NOTE_ID,
        noteMeta: makeNoteMeta(),
        currentProject: 'Notes',
      },
    });

    await flushAsync();

    // Let the image load first (its own skipDirty-tagged persist happens
    // here, per Case 1/3 above) so the drag-resize below is the ONLY
    // user-initiated write under test.
    const img = container.querySelector('.at-image-inline img') as HTMLImageElement | null;
    expect(img).not.toBeNull();
    Object.defineProperty(img!, 'naturalWidth', { value: 800, configurable: true });
    Object.defineProperty(img!, 'naturalHeight', { value: 600, configurable: true });
    await fireEvent.load(img!);
    await flushAsync();

    // Sanity: confirm the onload persist did NOT already save (Case 1
    // covers this in detail; re-asserting here isolates the drag-resize
    // save from any onload side effect).
    expect(saveNoteMock).not.toHaveBeenCalled();

    // Locate a corner resize handle and perform the same drag sequence as
    // AttachmentImageInline.test.ts I-4 ('calls onUpdateAttributes with
    // integer dimensions on mouseup'), but against the real mounted Editor
    // (real TipTap doc, real Attachment NodeView, real updateAttrs).
    const handle = container.querySelector('.at-handle-br') as HTMLElement | null;
    expect(handle).not.toBeNull();

    await fireEvent.mouseDown(handle!, { clientX: 100, clientY: 100 });
    await fireEvent.mouseMove(document, { clientX: 150, clientY: 125 });
    await fireEvent.mouseUp(document, { clientX: 150, clientY: 125 });
    await flushAsync();

    // Drag-resize's onUpdateAttributes call omits skipDirty (see
    // AttachmentImageInline.svelte's onDragEnd), so the resulting
    // tr.setNodeMarkup(...) transaction is NOT tagged with 'preventUpdate' —
    // it flows through TipTap's onUpdate → dirty = true → the 500ms
    // debounced save in Editor.svelte.
    await advanceAndFlush(600);

    expect(saveNoteMock).toHaveBeenCalled();

    // saveContent() passes the current markdown to api.saveNote(project, slug,
    // content, title). Sanity-check the content arg round-trips the resized
    // image's persisted attachment id and NEW width — confirms this was a
    // real save of the resized doc, not an incidental call from some other
    // code path.
    const [, , savedContent] = saveNoteMock.mock.calls[0] ?? [];
    expect(typeof savedContent).toBe('string');
    expect(savedContent).toContain('img-attachment-001');
    // The drag sequence above (100,100)→(150,125) grows width from the
    // natural 800px by the horizontal delta (+50px, then clamped/rounded by
    // AttachmentImageInline's applyDrag) — assert some non-800 data-width
    // landed in the saved markdown, proving the resize (not just the
    // original natural-dims persist) is what got saved.
    expect(savedContent).toMatch(/data-width="(?!800")\d+"/);
  });
});
