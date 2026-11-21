/**
 * KanbanBoard.cabinetDragOverStuckGreen.test.ts — #3166 C52.
 *
 * Matt's repro: the Cabinet column stays green (the `.drag-target`
 * affordance) after a card reorder drop within it — a card showing a
 * (freshly generated) image thumbnail reproduced it, but the root cause
 * is nothing image-specific.
 *
 * Root cause (Reed): `cabinetColumnDrop` (the Cabinet column's own
 * `.column-content` drop handler) calls `e.stopPropagation()` as its
 * first action whenever a Cabinet-internal reorder is in progress
 * (`cabinetDragSourceId` set) — by design, so the reorder doesn't also
 * trigger a status change via the column-level `ondrop={handleDrop}`.
 * But `handleDrop` is the ONLY place that ever cleared `dragOverColumn`
 * — `stopPropagation()` means it never runs for a Cabinet-internal
 * drop, while `dragOverColumn` was already set by `handleDragOver`/
 * `handleDragEnter` (neither of which checks `cabinetDragSourceId` —
 * they fire unconditionally as the drag crosses the column). Result:
 * the column stays stuck green after any Cabinet-internal reorder.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, cleanup, fireEvent } from '@testing-library/svelte/svelte5';
import KanbanBoard from './KanbanBoard.svelte';
import * as api from '$lib/api';
import * as entityService from '$lib/services/entityService';
import { canvasState } from '$lib/stores/canvas';
import { mockData } from '../../../vitest-setup';

const win = globalThis.window as any;
win.addEventListener = globalThis.addEventListener.bind(globalThis);
win.removeEventListener = globalThis.removeEventListener.bind(globalThis);
win.dispatchEvent = globalThis.dispatchEvent.bind(globalThis);

if (typeof (globalThis as any).ResizeObserver !== 'function') {
  (globalThis as any).ResizeObserver = class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
}

const PROJECT_ID = 'project-c52-cabinet-dragover';

function mountKanban() {
  canvasState.set({
    view: 'kanban',
    data: {},
    context: { projectId: PROJECT_ID, projectName: 'C52 Cabinet Drag Project' },
  } as any);
  return render(KanbanBoard, { props: {} });
}

beforeEach(() => {
  mockData.tasks = [
    {
      id: 'cabinet-card-1',
      title: 'First cabinet card',
      content_preview: '',
      status: 'cabinet',
      priority: 'medium',
      due_date: null,
      project_id: PROJECT_ID,
      folder_id: null,
      note_type: null,
      sort_order: 0,
      tags: [],
      created_at: new Date().toISOString(),
      updated_at: new Date().toISOString(),
    },
    {
      id: 'cabinet-card-2',
      title: 'Second cabinet card',
      content_preview: '',
      status: 'cabinet',
      priority: 'medium',
      due_date: null,
      project_id: PROJECT_ID,
      folder_id: null,
      note_type: null,
      sort_order: 1000,
      tags: [],
      created_at: new Date().toISOString(),
      updated_at: new Date().toISOString(),
    },
  ];
  mockData.noteContents = {};
  mockData.nextId = 1;
  mockData.projects = [
    { id: PROJECT_ID, name: 'C52 Cabinet Drag Project', parent_id: null,
      created_at: new Date().toISOString(), updated_at: new Date().toISOString() },
  ];
  vi.spyOn(api, 'listFolders').mockResolvedValue([]);
  vi.spyOn(api, 'getSheetCardSummaries').mockResolvedValue([]);
  vi.spyOn(entityService, 'reorderNotes').mockResolvedValue(undefined as any);
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe('#3166 C52 — Cabinet column drag-target highlight after an internal reorder drop', () => {
  it('clears .drag-target on the Cabinet column after a Cabinet-internal card reorder drop', async () => {
    const { container } = mountKanban();

    await vi.waitFor(() => {
      expect(container.querySelector('[data-cabinet-id="cabinet-card-1"]')).toBeTruthy();
    });

    const cabinetColumn = Array.from(container.querySelectorAll('.kanban-column'))
      .find(col => col.querySelector('.column-title')?.textContent === 'Cabinet') as HTMLElement;
    expect(cabinetColumn).toBeTruthy();
    const columnContent = cabinetColumn.querySelector('.column-content') as HTMLElement;
    expect(columnContent).toBeTruthy();

    const draggedCardWrap = container.querySelector('[data-cabinet-id="cabinet-card-1"]') as HTMLElement;
    const draggedCardEl = draggedCardWrap.querySelector('[draggable="true"]') as HTMLElement;
    expect(draggedCardEl).toBeTruthy();

    // 1. Drag starts on the first Cabinet card — sets cabinetDragSourceId.
    const dataTransfer = { dropEffect: '', effectAllowed: '', setData: vi.fn(), getData: vi.fn() };
    const dragStartEvent = Object.assign(new Event('dragstart', { bubbles: true, cancelable: true }), { dataTransfer });
    await fireEvent(draggedCardEl, dragStartEvent);

    // 2. Drag crosses the column — the column-level `ondragenter`/
    //    `ondragover` (`.kanban-column`, wired to `handleDragEnter`/
    //    `handleDragOver`) fires as the pointer enters the column's own
    //    area, which is what sets `dragOverColumn='cabinet'` (the green
    //    highlight) in BOTH the real app and this test — fired directly
    //    on `.kanban-column` rather than relying on a synthetic event's
    //    bubbling through jsdom, which does not reliably replicate a
    //    real pointer-drag's own separate enter/over dispatch sequence
    //    across nested drop targets.
    const dragOverEvent = Object.assign(new Event('dragover', { bubbles: true, cancelable: true }), {
      clientY: 50,
      dataTransfer,
    });
    await fireEvent(cabinetColumn, dragOverEvent);

    await vi.waitFor(() => {
      expect(cabinetColumn.className).toContain('drag-target');
    });

    // Then the pointer moves onto the inner Cabinet reorder surface —
    // `cabinetColumnDragOver` (`.column-content`) resolves the drop
    // indicator, independent of the outer highlight set above.
    await fireEvent(columnContent, Object.assign(new Event('dragover', { bubbles: true, cancelable: true }), {
      clientY: 50,
      dataTransfer,
    }));

    // 3. Drop lands on .column-content (the Cabinet-internal reorder
    //    path) — this is the exact drop that used to leave the column
    //    stuck green, because `cabinetColumnDrop`'s own
    //    `stopPropagation()` means `handleDrop` (the column-level
    //    handler, the ONLY place that used to clear `dragOverColumn`)
    //    never runs for this drop.
    const dropEvent = Object.assign(new Event('drop', { bubbles: true, cancelable: true }), {
      clientY: 50,
      dataTransfer,
    });
    await fireEvent(columnContent, dropEvent);

    await vi.waitFor(() => {
      expect(cabinetColumn.className).not.toContain('drag-target');
    });
  });
});
