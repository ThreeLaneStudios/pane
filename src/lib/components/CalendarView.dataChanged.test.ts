/**
 * CalendarView.dataChanged.test.ts — #2972 stage 2 (`docs/spec/
 * data-refresh.md` §34.5/§34.10's Stage 2 "Calendar" acceptance criterion):
 *
 * "opening a sheet from the calendar after an agent creates it in a new
 * folder shows the correct folder, matching the sidebar (no duplicate
 * 'General'-named folder)."
 *
 * `CalendarView.svelte` had zero `pane-*` listeners before this stage
 * (confirmed at spec time). This test simulates the backend emitting the
 * new event twice, in the real sequence an agent's two tool calls would
 * produce — first `create_folder` (`entity_type: "folder"`), then
 * `create_sheet` in that folder with a due date (`entity_type: "sheet"`) —
 * and asserts the resulting calendar card resolves the NEW folder's own
 * color (not the default-color fallback `getProjectColor()` uses for an
 * unmatched folder_id), proving the calendar's own `projects` lookup table
 * picked up the new folder rather than showing a stale/mismatched one.
 */

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, cleanup, screen } from '@testing-library/svelte/svelte5';
import CalendarView from './CalendarView.svelte';
import * as api from '$lib/api';
import { simulateTauriEvent } from '../../../vitest-setup';
import {
  EVENT_DATA_CHANGED,
  ENTITY_FOLDER,
  ENTITY_SHEET,
  REASON_CREATED,
  dispatchDataChanged,
} from '$lib/refreshEvent';
import type { Project, NoteContent } from '$lib/types';

// #2972 stage 3 (item 4): unconditional — vitest-setup.ts's fake `window`
// already defines a `dispatchEvent` property (a no-op stub); a
// conditional guard would see that as "already present" and never wire it
// to the real jsdom EventTarget.
(globalThis.window as any).addEventListener = globalThis.addEventListener.bind(globalThis);
(globalThis.window as any).removeEventListener = globalThis.removeEventListener.bind(globalThis);
(globalThis.window as any).dispatchEvent = globalThis.dispatchEvent.bind(globalThis);

const EXISTING_PROJECT: Project = {
  id: 'proj-calendar-datachanged-1',
  name: 'Calendar Project',
  parent_id: undefined,
  sort_order: 0,
  created_at: '2026-01-01T00:00:00Z',
  updated_at: '2026-01-01T00:00:00Z',
} as any;

const NEW_FOLDER_ID = 'folder-agent-created-for-calendar';
const NEW_FOLDER_COLOR = '#FF00FF'; // distinct from getProjectColor()'s '#00E4E4' default
const SHEET_ID = 'sheet-agent-created-for-calendar';
const DUE_DATE = '2026-11-05';

function todayPlusDueDate(): string {
  // Use a fixed future date string — CalendarView filters tasks onto the
  // calendar month currently displayed (defaults to "today"'s month/year).
  // To keep this test simple and independent of the real current date,
  // we assert on the task's presence via its title text (rendered inside
  // the matching day cell only if getTasksForDate's due_date match
  // succeeds) rather than navigating months.
  const now = new Date();
  return `${now.getFullYear()}-${String(now.getMonth() + 1).padStart(2, '0')}-15`;
}

beforeEach(() => {
  vi.spyOn(api, 'listTasks').mockResolvedValue([]);
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe('CalendarView pane-data-changed handler (#2972 stage 2, §34.5)', () => {
  it('shows a new folder-created-then-sheet-created-in-it with the NEW folder\'s own color, not a stale default', async () => {
    const dueDate = todayPlusDueDate();

    // #2972 stage 2 WARN closure (PR #3016 / Reed's 2026-10-11 audit, Matt's
    // sign-off "fix before merge"): the folder-created event used to
    // trigger a full `api.listProjects()` -> `get_projects()` refetch
    // (Reed's WARN). It now patches the ONE changed folder in via a new
    // single-entity read (`api.getProjectOrFolder()`) — `listProjectsSpy`
    // must stay at its mount-time count of 1, never climb to 2.
    const listProjectsSpy = vi.spyOn(api, 'listProjects')
      .mockResolvedValueOnce([EXISTING_PROJECT]); // initial mount load only
    const getOneSpy = vi.spyOn(api, 'getProjectOrFolder').mockResolvedValue({
      id: NEW_FOLDER_ID,
      name: 'Agent Folder For Calendar',
      parent_id: EXISTING_PROJECT.id,
      color: NEW_FOLDER_COLOR,
      sort_order: 0,
      created_at: '2026-01-01T00:00:00Z',
      updated_at: '2026-01-01T00:00:00Z',
    } as any);

    render(CalendarView, { props: {} });

    await vi.waitFor(() => {
      expect(listProjectsSpy).toHaveBeenCalledTimes(1);
    });

    // Step 1: agent's create_folder tool — the backend emit (#2972 stage 1)
    // for ENTITY_FOLDER/REASON_CREATED.
    simulateTauriEvent(EVENT_DATA_CHANGED, {
      entity_id: NEW_FOLDER_ID,
      entity_type: ENTITY_FOLDER,
      reason: REASON_CREATED,
      origin_tag: 'test-origin-tag',
    });

    await vi.waitFor(() => {
      // The single read fires on the event...
      expect(getOneSpy).toHaveBeenCalledTimes(1);
    });
    expect(getOneSpy).toHaveBeenCalledWith(NEW_FOLDER_ID);
    // ...and the list fetch does NOT — stays at its mount-time count.
    expect(listProjectsSpy).toHaveBeenCalledTimes(1);

    // Step 2: agent's create_sheet tool, in that new folder, with a due
    // date — the backend emit for ENTITY_SHEET/REASON_CREATED. The
    // calendar's single-entity read (api.readNote) returns the new sheet.
    const readNoteSpy = vi.spyOn(api, 'readNote').mockResolvedValue({
      meta: {
        id: SHEET_ID,
        title: 'Agent Sheet',
        notebook: '',
        slug: SHEET_ID,
        preview: '',
        // Deliberately NOT EXISTING_PROJECT.id — isolates getProjectColor()'s
        // folder_id match (production code matches on EITHER project_id OR
        // folder_id, "whichever's found first in the array"; this test's
        // job is proving the NEW folder is present and resolvable via
        // folder_id, not re-litigating that pre-existing either/or design).
        project_id: 'irrelevant-project-id-not-in-projects-list',
        folder_id: NEW_FOLDER_ID,
        task_due_date: dueDate,
        note_type: 'sheet',
        created_at: '2026-01-01T00:00:00Z',
        updated_at: '2026-01-01T00:00:00Z',
      },
      content: '',
    } as unknown as NoteContent);

    simulateTauriEvent(EVENT_DATA_CHANGED, {
      entity_id: SHEET_ID,
      entity_type: ENTITY_SHEET,
      reason: REASON_CREATED,
      origin_tag: 'test-origin-tag',
    });

    await screen.findByText('Agent Sheet');
    expect(readNoteSpy).toHaveBeenCalledWith('', SHEET_ID);

    // The rendered card must resolve the NEW folder's own color — proving
    // the calendar's `projects` lookup picked up the agent-created folder
    // (matching what the sidebar would show), not a stale/default fallback.
    const taskItem = screen.getByText('Agent Sheet').closest('.task-item') as HTMLElement;
    expect(taskItem).toBeTruthy();
    // jsdom normalizes the inline hex color to rgb() — #FF00FF === rgb(255, 0, 255).
    expect(taskItem.getAttribute('style') ?? '').toMatch(/border-left-color:\s*rgb\(255,\s*0,\s*255\)/i);
  });

  it('patches a new sheet card the SAME way via the APP channel (DOM CustomEvent), not just the backend Tauri channel (#2972 stage 3, item 4)', async () => {
    const dueDate = todayPlusDueDate();
    vi.spyOn(api, 'listProjects').mockResolvedValueOnce([EXISTING_PROJECT]);

    render(CalendarView, { props: {} });
    await vi.waitFor(() => expect(vi.mocked(api.listProjects).mock.calls.length).toBeGreaterThanOrEqual(1));

    vi.spyOn(api, 'readNote').mockResolvedValue({
      meta: {
        id: 'sheet-app-channel-1',
        title: 'App Channel Sheet',
        notebook: '',
        slug: 'sheet-app-channel-1',
        preview: '',
        project_id: EXISTING_PROJECT.id,
        folder_id: null,
        task_due_date: dueDate,
        note_type: 'sheet',
        created_at: '2026-01-01T00:00:00Z',
        updated_at: '2026-01-01T00:00:00Z',
      },
      content: '',
    } as unknown as NoteContent);

    dispatchDataChanged('sheet-app-channel-1', ENTITY_SHEET, REASON_CREATED, 'test-origin-tag-app');

    await screen.findByText('App Channel Sheet');
  });
});
