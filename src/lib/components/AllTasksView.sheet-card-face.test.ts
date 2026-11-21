/**
 * #2339 PR 15a — Board card face: AllTasksView wiring regression test.
 *
 * AllTasksView.svelte has no bespoke list-density card renderer of its own
 * (confirmed at build time via grep) — it routes every task through the same
 * <Card data={normalizeNoteMeta(...)}> path as KanbanBoard/NotesBrowser, so
 * the sheet card face (grid glyph + micro-grid + cols × rows footer) is
 * inherited automatically once Card.svelte's sheet branch exists. This test
 * asserts that wiring stays intact: AllTasksView batch-fetches sheet card
 * summaries and threads them into normalizeNoteMeta via `sheetSummary`,
 * exactly like KanbanBoard.svelte's cardSummaryFor/sheetSummaries pattern.
 *
 * Source-text-extraction / regex assertions, matching the house pattern for
 * this heavy Tauri/store-dependent component (see
 * AllTasksView.new-sheet-button-polish.test.ts).
 */

import { describe, it, expect } from 'vitest';
import { readFileSync } from 'fs';
import { resolve } from 'path';

const COMPONENT_PATH = resolve(__dirname, 'AllTasksView.svelte');
const SOURCE = readFileSync(COMPONENT_PATH, 'utf-8');

describe('#2339 PR 15a — AllTasksView reuses Card.svelte sheet branch (no bespoke list renderer)', () => {
  it('has exactly one <Card ...> usage — confirms no separate list-density card renderer exists', () => {
    const matches = SOURCE.match(/<Card\b/g) || [];
    expect(matches.length).toBe(1);
  });

  it('batch-fetches sheet card summaries via api.getSheetCardSummaries, keyed by note_type === "sheet"', () => {
    expect(SOURCE).toMatch(/\.note_type === 'sheet'/);
    expect(SOURCE).toMatch(/api\.getSheetCardSummaries\(/);
  });

  it('threads the fetched summary into normalizeNoteMeta via a `sheetSummary` opt, mirroring KanbanBoard/NotesBrowser', () => {
    expect(SOURCE).toMatch(/normalizeNoteMeta\(note,\s*\{[^}]*sheetSummary:\s*cardSummaryFor\(note\)/);
  });

  it('cardSummaryFor returns undefined for non-sheet notes (does not force sheet fields onto note cards)', () => {
    const fnMatch = SOURCE.match(/function cardSummaryFor\([^)]*\)\s*\{[\s\S]*?\n  \}/);
    expect(fnMatch).not.toBeNull();
    expect(fnMatch![0]).toMatch(/if \(note\.note_type !== 'sheet'\) return undefined;/);
  });
});
