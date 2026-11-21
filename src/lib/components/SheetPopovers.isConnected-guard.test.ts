// SheetPopovers.isConnected-guard.test.ts — jsdom coverage for the
// `!e.target.isConnected` early-return guard added to all six sheet
// popovers' `handleWindowClick` handlers (#2874 item 30).
//
// IMPORTANT: these tests exercise the GUARD itself, not the original bug.
// The actual Item 30 defect — a click handler synchronously removing its
// own clicked element from the DOM mid-bubble, causing a still-bubbling
// click event's `e.target` to go stale/detached by the time an ancestor's
// outside-click handler inspects it — only reproduces under real-browser
// (Chromium) event-dispatch timing; jsdom's synchronous dispatch can't
// recreate that race (see the approved scope doc's §6 "Secondary jsdom
// tests" note and Matt's amendment ruling out a Playwright spec for this
// item). What IS verified here, directly and deterministically: given a
// window click event whose `target` is a `Node` with `isConnected ===
// false` (i.e. `document.createElement('div')` never appended anywhere),
// each of the six `handleWindowClick` functions bails out via the new
// guard's early return and does NOT call its close/cancel callback.
//
// vitest-setup.ts replaces `globalThis.window` with a plain mock object
// lacking real `addEventListener`/`removeEventListener`/`dispatchEvent`,
// which every other Sheets popover test file patches with a no-op stub
// (so mount/unmount succeeds but `<svelte:window onclick>` never actually
// fires in response to a dispatched event). To exercise the guard for
// real, this file follows `ShStylePanel.test.ts`'s precedent (#2874 item
// 16 Part B) and installs a genuine minimal pub/sub instead of a no-op,
// scoped to this file only.

import { describe, test, expect, vi } from 'vitest';
import { render, screen } from '@testing-library/svelte/svelte5';
import SheetCellEditorSelect from './sheetEditors/SheetCellEditorSelect.svelte';
import ShHeaderMenu from './ShHeaderMenu.svelte';
import ShSheetMenu from './ShSheetMenu.svelte';
import ShCellContextMenu from './ShCellContextMenu.svelte';
import ShGhostCellContextMenu from './ShGhostCellContextMenu.svelte';
import ShStylePanel from './ShStylePanel.svelte';
import type { SheetColumn } from '$lib/types';

const windowListeners: Record<string, Array<(e: Event) => void>> = {};
(globalThis.window as any).addEventListener = (type: string, fn: (e: Event) => void) => {
  (windowListeners[type] ||= []).push(fn);
};
(globalThis.window as any).removeEventListener = (type: string, fn: (e: Event) => void) => {
  windowListeners[type] = (windowListeners[type] || []).filter((f) => f !== fn);
};
(globalThis.window as any).dispatchEvent = (evt: Event) => {
  for (const fn of windowListeners[evt.type] || []) fn(evt);
  return true;
};
if (typeof (globalThis.window as any).innerWidth !== 'number') {
  (globalThis.window as any).innerWidth = 1200;
}
if (typeof (globalThis.window as any).innerHeight !== 'number') {
  (globalThis.window as any).innerHeight = 900;
}

// A click target that has never been appended to `document` — its
// `isConnected` is `false`, matching what a synchronously-removed clicked
// element looks like mid-bubble.
function detachedClickEvent(): MouseEvent {
  const detached = document.createElement('div');
  const evt = new MouseEvent('click', { bubbles: true, button: 0 });
  Object.defineProperty(evt, 'target', { value: detached });
  return evt;
}

function col(overrides: Partial<SheetColumn> = {}): SheetColumn {
  return {
    id: overrides.id ?? 'col-a',
    note_id: 'note-1',
    name: overrides.name ?? 'Status',
    col_type: overrides.col_type ?? 'text',
    options_json: overrides.options_json ?? '{}',
    width: overrides.width ?? 148,
    sort_order: overrides.sort_order ?? 0,
    created_at: '2026-01-01T00:00:00Z',
    updated_at: '2026-01-01T00:00:00Z',
  };
}

function makeAnchor(left = 10): HTMLElement {
  const el = document.createElement('div');
  el.getBoundingClientRect = () => ({
    left,
    right: left + 148,
    top: 0,
    bottom: 38,
    width: 148,
    height: 38,
    x: left,
    y: 0,
    toJSON: () => ({}),
  });
  return el;
}

describe('Sheet popovers — isConnected guard on outside-click handlers (#2874 item 30)', () => {
  test('SheetCellEditorSelect: outside-click with detached target does not cancel', () => {
    const onCancel = vi.fn();
    render(SheetCellEditorSelect, {
      props: { value: 'Draft', choices: ['Draft', 'Shipped'], onCommit: vi.fn(), onCancel },
    });
    window.dispatchEvent(detachedClickEvent());
    expect(onCancel).not.toHaveBeenCalled();
  });

  test('ShHeaderMenu: outside-click with detached target does not close', () => {
    const onClose = vi.fn();
    render(ShHeaderMenu, {
      props: {
        column: col(),
        anchorEl: makeAnchor(),
        filterValues: null,
        onRename: vi.fn(),
        onTypeChange: vi.fn(),
        onInsertColumn: vi.fn(),
        onSortAsc: vi.fn(),
        onSortDesc: vi.fn(),
        onFilterChange: vi.fn(),
        onDelete: vi.fn(),
        onClose,
        onOptionsChange: vi.fn(),
      },
    });
    window.dispatchEvent(detachedClickEvent());
    expect(onClose).not.toHaveBeenCalled();
  });

  test('ShSheetMenu: outside-click with detached target does not close', () => {
    const onClose = vi.fn();
    render(ShSheetMenu, {
      props: {
        anchorEl: makeAnchor(),
        onExportCsv: vi.fn(),
        onVersionHistory: vi.fn(),
        onDelete: vi.fn(),
        onClose,
      },
    });
    window.dispatchEvent(detachedClickEvent());
    expect(onClose).not.toHaveBeenCalled();
  });

  test('ShCellContextMenu: outside-click with detached target does not close', () => {
    const onClose = vi.fn();
    render(ShCellContextMenu, {
      props: {
        x: 100,
        y: 100,
        cellCount: 1,
        onStyleCells: vi.fn(),
        onClearStyling: vi.fn(),
        onInsertRow: vi.fn(),
        onDeleteContents: vi.fn(),
        onClose,
      },
    });
    window.dispatchEvent(detachedClickEvent());
    expect(onClose).not.toHaveBeenCalled();
  });

  test('ShGhostCellContextMenu: outside-click with detached target does not close', () => {
    const onClose = vi.fn();
    render(ShGhostCellContextMenu, {
      props: {
        x: 100,
        y: 100,
        onInsertRow: vi.fn(),
        onInsertColumn: vi.fn(),
        onClose,
      },
    });
    window.dispatchEvent(detachedClickEvent());
    expect(onClose).not.toHaveBeenCalled();
  });

  test('ShStylePanel: outside-click with detached target does not close', () => {
    const onClose = vi.fn();
    render(ShStylePanel, {
      props: {
        currentBg: null,
        currentFg: null,
        currentBorder: null,
        onApplyFill: vi.fn(),
        onApplyInk: vi.fn(),
        onApplyBorder: vi.fn(),
        onClearStyling: vi.fn(),
        onClose,
      },
    });
    window.dispatchEvent(detachedClickEvent());
    expect(onClose).not.toHaveBeenCalled();
  });
});
