// Editor.sheet-refresh-subscription.test.ts — #3053 item 1 (docs/audits/
// 2026-10-20-scope-1061-fixes.md, D1/D2 part 1).
//
// Before this fix, Editor.svelte's `onMount` guard was
// `if (isSheet) { return () => {}; }` — an early return out of the WHOLE
// `onMount` body for a sheet note, which skipped every listener
// registration below that point, including `subscribeDataChanged`
// (`Editor.svelte:2015` at base). A sheet never heard the one
// `pane-data-changed` refresh event at all, from either channel (backend
// `emit()` or the app `CustomEvent`).
//
// This test mounts the REAL Editor.svelte with a SHEET `noteMeta`
// (`note_type: 'sheet'`) and drives the BACKEND channel specifically
// (`simulateTauriEvent`, vitest-setup.ts's fake Tauri event-plugin
// registry) — the channel an agent-driven change would arrive on, since
// there is no "app" caller on that path to fire the DOM `CustomEvent`
// half (`dispatchDataChanged`) itself. Before the fix, this event would
// reach ZERO listeners for a sheet-mounted Editor (no `listen()` call was
// ever made) and `readNote` would never be called.
//
// Same mounting pattern as Editor.sheet-cell-editing-integration.test.ts
// (mount the real Editor + SheetGrid, mock only `$lib/api`) and the same
// window-polyfill pattern as Editor.echoSuppression.test.ts (mount
// requires `window.addEventListener`/`dispatchEvent` to exist because
// `subscribeDataChanged` registers a DOM listener as well as the Tauri
// one — this test only fires the Tauri half, but `subscribeDataChanged`
// registers both unconditionally).

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, cleanup } from '@testing-library/svelte/svelte5';
import { tick } from 'svelte';
import { get } from 'svelte/store';
import Editor from './Editor.svelte';
import { activeNote } from '$lib/stores/app';
import type { NoteMeta, SheetData } from '$lib/types';
import { simulateTauriEvent } from '../../../vitest-setup';
import { EVENT_DATA_CHANGED } from '$lib/refreshEvent';

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

const NOTE_ID = 'sheet-refresh-note-1';
const PROJECT = 'Notes';

const { readNoteMock, getSheetDataMock, listTasksByNoteMock, getBacklinksMock } = vi.hoisted(() => ({
  readNoteMock: vi.fn(),
  getSheetDataMock: vi.fn(),
  listTasksByNoteMock: vi.fn(),
  getBacklinksMock: vi.fn(),
}));

vi.mock('$lib/api', () => ({
  readNote: readNoteMock,
  getSheetData: getSheetDataMock,
  listTasksByNote: listTasksByNoteMock,
  getBacklinks: getBacklinksMock,
  resolveAttachmentSrc: vi.fn().mockResolvedValue('asset://x'),
  fetchLinkPreview: vi.fn().mockResolvedValue(null),
  getCachedLinkPreview: vi.fn().mockResolvedValue(null),
  rebuildNoteLinks: vi.fn().mockResolvedValue(undefined),
  listFolders: vi.fn().mockResolvedValue([]),
}));

function makeSheetMeta(overrides: Partial<NoteMeta> = {}): NoteMeta {
  return {
    id: NOTE_ID,
    title: 'Refresh Sheet',
    notebook: PROJECT,
    slug: NOTE_ID,
    created_at: '2026-01-01T00:00:00Z',
    updated_at: '2026-01-01T00:00:00Z',
    preview: '',
    note_type: 'sheet',
    ...overrides,
  } as NoteMeta;
}

function baseSheetData(): SheetData {
  return {
    note_meta: makeSheetMeta() as any,
    columns: [],
    rows: [],
    cells: [],
    formula_count: 0,
    formulas_paused: false,
  };
}

async function flushAsync() {
  await Promise.resolve();
  await tick();
  await tick();
}

beforeEach(() => {
  readNoteMock.mockReset();
  getSheetDataMock.mockReset();
  listTasksByNoteMock.mockReset();
  getBacklinksMock.mockReset();
  getSheetDataMock.mockResolvedValue(baseSheetData());
  listTasksByNoteMock.mockResolvedValue([]);
  getBacklinksMock.mockResolvedValue([]);
  activeNote.set({ meta: makeSheetMeta(), content: '' } as any);
});

afterEach(() => {
  cleanup();
  activeNote.set(null);
});

describe('Editor sheet subscription (#3053 item 1)', () => {
  it('an open SHEET editor receives the backend pane-data-changed event and refreshes its metadata', async () => {
    render(Editor, {
      props: {
        noteId: NOTE_ID,
        noteMeta: makeSheetMeta(),
        slug: NOTE_ID,
        currentProject: PROJECT,
      },
    });

    // Wait for the sheet to finish mounting (SheetGrid renders once
    // getSheetData resolves) — same wait condition
    // Editor.sheet-cell-editing-integration.test.ts uses.
    await vi.waitFor(() => expect(screen.getByTestId('sheet-grid')).toBeTruthy());
    await flushAsync();

    // BASELINE — mounting alone must not have triggered a metadata refetch.
    expect(readNoteMock).not.toHaveBeenCalled();

    readNoteMock.mockResolvedValueOnce({
      meta: makeSheetMeta({ title: 'Renamed by the agent' }),
      content: '',
    });

    // Drive the BACKEND channel specifically — the one an agent-driven
    // `set_status`/`move_note` call actually arrives on (there is no "app"
    // caller on that path to fire the DOM CustomEvent half itself).
    simulateTauriEvent(EVENT_DATA_CHANGED, {
      entity_id: NOTE_ID,
      entity_type: 'sheet',
      reason: 'edited',
      origin_tag: 'agent-tool-call-tag',
    });
    await flushAsync();

    // Before the fix: `onMount` returned before `subscribeDataChanged` was
    // ever called for a sheet, so `listen()` was never registered and this
    // event reached zero listeners — `readNote` would never be called and
    // this assertion would fail (still 0 calls).
    expect(readNoteMock).toHaveBeenCalledTimes(1);
    expect(readNoteMock).toHaveBeenCalledWith(PROJECT, NOTE_ID);
    expect(get(activeNote)?.meta?.title).toBe('Renamed by the agent');
  });
});
