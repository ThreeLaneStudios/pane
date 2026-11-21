/**
 * CanvasRenderer.search-sheet-identity.test.ts — #2874 item 15 regression
 * coverage for the search-results click-through path.
 *
 * Two click sites exist in CanvasRenderer's search_results view
 * (CanvasRenderer.svelte ~line 499-534):
 *
 *   - `handleNoteClick(note)` (line 361) — navigates to `note_editor` with
 *     `noteId: note.slug`. The actual note fetch happens downstream in the
 *     `loadDataForView`/`handleCanvasAction`'s `case 'note_editor':` branch
 *     (line 229), which calls `noteService.loadNoteWithJson(projectName,
 *     noteSlug)` and assigns its FULL return value straight to
 *     `$activeNote = noteWithJson` — no hand-picked meta object.
 *     `loadNoteWithJson` (noteService.ts:197) is itself a thin wrapper
 *     around `api.readNote()`.
 *   - `handleTaskClick(task)` (line 370) — navigates to `kanban` (updated by
 *     #2874 item F: previously `notes_browser`, now the task's project
 *     board with its card open) and dispatches a `pane-select-note`
 *     CustomEvent, the exact same mechanism covered end-to-end (through
 *     KanbanBoard + handleNoteSelect) by
 *     CanvasRenderer.taskclick-completion.test.ts.
 *
 * Per the item-15 scope audit, both were traced and found ALREADY SAFE — no
 * hand-built meta object exists on this path. This file pins that finding as
 * a regression guard. Updated for #2874 item F's navigation-target change
 * (notes_browser -> kanban) without touching the underlying item-15 finding
 * this file was written to pin.
 *
 * Source-text-extraction / regex assertions, matching the house pattern for
 * this very large (1000+ line), heavily Tauri/store-dependent component
 * (see AllTasksView.sheet-card-face.test.ts's own precedent).
 */

import { describe, it, expect } from 'vitest';
import { readFileSync } from 'fs';
import { resolve } from 'path';

const CANVAS_RENDERER_PATH = resolve(__dirname, 'CanvasRenderer.svelte');
const CANVAS_RENDERER_SOURCE = readFileSync(CANVAS_RENDERER_PATH, 'utf-8');

const NOTE_SERVICE_PATH = resolve(__dirname, '../services/noteService.ts');
const NOTE_SERVICE_SOURCE = readFileSync(NOTE_SERVICE_PATH, 'utf-8');

describe('CanvasRenderer search results — note click preserves sheet identity (item 15)', () => {
  it('handleNoteClick navigates via note_editor view + noteId, with no meta hand-built at the click site', () => {
    const fnMatch = CANVAS_RENDERER_SOURCE.match(/function handleNoteClick\(note: NoteMeta\) \{[\s\S]*?\n  \}/);
    expect(fnMatch).toBeTruthy();
    const body = fnMatch![0];
    expect(body).toMatch(/view:\s*'note_editor'/);
    expect(body).toMatch(/noteId:\s*note\.slug/);
    // Must NOT construct any object literal resembling a hand-built NoteMeta here.
    expect(body).not.toMatch(/meta:\s*\{/);
  });

  it('the note_editor loader assigns the FULL noteService.loadNoteWithJson() result to $activeNote — no field-by-field re-mapping', () => {
    const caseMatch = CANVAS_RENDERER_SOURCE.match(/case 'note_editor':[\s\S]*?\n\s*break;/);
    expect(caseMatch).toBeTruthy();
    const body = caseMatch![0];
    expect(body).toMatch(/await noteService\.loadNoteWithJson\(/);
    // RED-ON-REVERT: if this line becomes `$activeNote = { meta: { id: ..., title: ... }, ... }`
    // instead of a bare assignment of the loader's return value, note_type silently drops
    // again the same way it did in the KanbanBoard.svelte bug this PR fixes.
    expect(body).toMatch(/\$activeNote = noteWithJson;?/);
  });

  it('noteService.loadNoteWithJson is a thin, unmodified wrapper around api.readNote — it does not hand-pick fields', () => {
    const fnMatch = NOTE_SERVICE_SOURCE.match(/export async function loadNoteWithJson\([\s\S]*?\n\}/);
    expect(fnMatch).toBeTruthy();
    const body = fnMatch![0];
    expect(body).toMatch(/return await api\.readNote\(notebook, slug\);?/);
    expect(body).not.toMatch(/meta:\s*\{/);
  });

  it('handleTaskClick dispatches the same pane-select-note CustomEvent mechanism covered end-to-end by CanvasRenderer.taskclick-completion.test.ts', () => {
    const fnMatch = CANVAS_RENDERER_SOURCE.match(/async function handleTaskClick\(task: Task\) \{[\s\S]*?\n  \}/);
    expect(fnMatch).toBeTruthy();
    const body = fnMatch![0];
    // #2874 item F: the destination is now the project board (view: 'kanban'
    // with the task's card open), not the notes browser.
    expect(body).toMatch(/view:\s*'kanban'/);
    expect(body).not.toMatch(/view:\s*'notes_browser'/);
    expect(body).toMatch(/dispatchEvent\(new CustomEvent\('pane-select-note',\s*\{\s*detail:\s*\{\s*noteId:\s*task\.id\s*\}\s*\}\)\)/);
  });
});
