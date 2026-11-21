/**
 * Editor.echoSuppression.test.ts — #2972 stage 3 (`docs/spec/
 * data-refresh.md` §34.4, §34.10's Stage 3 acceptance criteria: "open a
 * note, save from that editor instance, assert no re-fetch/re-render from
 * its own resulting event — and, in the same test, a second independent
 * event for the same note ID... does still trigger a refresh").
 *
 * NOT to be confused with the unrelated, pre-existing
 * `ChatSidebar.echoSuppression.test.ts` (#2410, transcript-ingest gating)
 * — §34.11 point 2 flags that naming collision explicitly; this file is
 * the real origin-tag mechanism (§34.4), on a completely different
 * component.
 *
 * Mounts the REAL Editor.svelte (same pattern as
 * Editor.mount-no-dirty.test.ts / Editor.attachment-onload-dirty.test.ts).
 * Only `$lib/api` is mocked; `@tauri-apps/api/core`'s `invoke()` goes
 * through vitest-setup.ts's real mock-data-backed `update_note` handler.
 */

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, cleanup } from '@testing-library/svelte/svelte5';
import { tick } from 'svelte';
import { get } from 'svelte/store';
import Editor from './Editor.svelte';
import { activeNote } from '$lib/stores/app';
import type { NoteMeta } from '$lib/types';
import { mockData, simulateTauriEvent } from '../../../vitest-setup';
import { EVENT_DATA_CHANGED } from '$lib/refreshEvent';

// vitest-setup.ts replaces globalThis.window with a plain mock object
// lacking addEventListener/removeEventListener/dispatchEvent/etc. — same
// polyfill pattern as Editor.mount-no-dirty.test.ts.
const win = globalThis.window as any;
// NOTE: dispatchEvent is NOT conditionally patched (unlike addEventListener/
// removeEventListener below) — vitest-setup.ts's fake `window` already
// defines a `dispatchEvent` property (a no-op stub that returns `true`
// without invoking any listener). A `typeof !== 'function'` guard would
// see that stub as "already present" and skip patching it, silently
// leaving every `window.dispatchEvent(...)` call (including this file's
// own app-channel sends, via `dispatchDataChanged`) a no-op that never
// reaches any `addEventListener` registration — which is exactly the kind
// of false-pass this echo-suppression test must NOT be vulnerable to.
win.dispatchEvent = globalThis.dispatchEvent.bind(globalThis);
if (typeof win.addEventListener !== 'function') {
  win.addEventListener = globalThis.addEventListener?.bind(globalThis) ?? (() => {});
}
if (typeof win.removeEventListener !== 'function') {
  win.removeEventListener = globalThis.removeEventListener?.bind(globalThis) ?? (() => {});
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

const NOTE_ID = 'echo-note-1';

const { readNoteMock } = vi.hoisted(() => ({
  readNoteMock: vi.fn(),
}));

vi.mock('$lib/api', () => ({
  readNote: readNoteMock,
  saveNote: vi.fn().mockResolvedValue({}),
  resolveAttachmentSrc: vi.fn().mockResolvedValue('asset://x'),
  listTasksByNote: vi.fn().mockResolvedValue([]),
  getBacklinks: vi.fn().mockResolvedValue([]),
  fetchLinkPreview: vi.fn().mockResolvedValue(null),
  getCachedLinkPreview: vi.fn().mockResolvedValue(null),
  rebuildNoteLinks: vi.fn().mockResolvedValue(undefined),
  listFolders: vi.fn().mockResolvedValue([]),
}));

function makeNoteMeta(overrides: Partial<NoteMeta> = {}): NoteMeta {
  return {
    id: NOTE_ID,
    title: 'Echo note',
    notebook: 'Notes',
    slug: NOTE_ID,
    created_at: '2026-01-01T00:00:00Z',
    updated_at: '2026-01-01T00:00:00Z',
    preview: '',
    task_status: 'cabinet',
    ...overrides,
  } as NoteMeta;
}

async function flushAsync() {
  await Promise.resolve();
  await tick();
  await tick();
}

beforeEach(() => {
  readNoteMock.mockReset();
  mockData.tasks = [
    { id: NOTE_ID, title: 'Echo note', status: 'cabinet', priority: 'medium', project_id: null, folder_id: null },
  ];
  activeNote.set({ meta: makeNoteMeta(), content: '' } as any);
});

afterEach(() => {
  cleanup();
  activeNote.set(null);
});

describe('Editor echo suppression (#2972 stage 3, §34.4)', () => {
  it('skips the refetch on its OWN save echo, but still refreshes on a later independent edit to the same note', async () => {
    const { getByText } = render(Editor, {
      props: {
        content: 'Some body text.',
        title: 'Echo note',
        slug: NOTE_ID,
        noteId: NOTE_ID,
        noteMeta: makeNoteMeta(),
        currentProject: 'Notes',
      },
    });

    await flushAsync();
    // BASELINE — mounting alone must not have triggered any metadata refetch.
    expect(readNoteMock).not.toHaveBeenCalled();

    // ── Half 1: save FROM this editor instance (the status pill) ──────
    // Open the status pill dropdown, then pick "Todo" — this is a real
    // save (updateEditorMeta -> invoke('update_note') -> the shared
    // dispatchDataChanged sender), mints+registers its OWN origin tag
    // before the save starts (§34.4), and its resulting event echoes
    // right back to this SAME Editor instance's own `pane-data-changed`
    // subscription (same-process, synchronous DOM dispatch).
    getByText('Cabinet').click();
    await flushAsync();
    getByText('Todo').click();
    await flushAsync();

    // BEFORE the fix (no echo suppression — Stage 2's bare entity_id gate
    // with no origin_tag check at all), this assertion is exactly what
    // would fail: the Editor's own save's echo would ALSO trigger a
    // readNote() refetch, because entity_id matches and nothing checks
    // whose save produced it.
    expect(readNoteMock).not.toHaveBeenCalled();

    // ── Half 2: a SECOND, independent edit to the SAME note ───────────
    // Simulates an agent/background edit arriving over the backend
    // channel, carrying a tag this Editor never registered. Must NOT be
    // suppressed — §34.4's second rule, required in the same test as the
    // first so a broken "always skip this note" implementation can't
    // pass by accident.
    readNoteMock.mockResolvedValueOnce({
      meta: makeNoteMeta({ title: 'Edited by someone else' }),
      content: 'Changed elsewhere',
    });
    simulateTauriEvent(EVENT_DATA_CHANGED, {
      entity_id: NOTE_ID,
      entity_type: 'note',
      reason: 'edited',
      origin_tag: 'a-completely-foreign-tag',
    });
    await flushAsync();

    expect(readNoteMock).toHaveBeenCalledTimes(1);
    expect(readNoteMock).toHaveBeenCalledWith('Notes', NOTE_ID);
    expect(get(activeNote)?.meta?.title).toBe('Edited by someone else');
  });
});
