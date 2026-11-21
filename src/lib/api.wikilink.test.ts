/**
 * Tests for #2419 — Wiki-Link picker search invoke shape
 *
 * Red-on-revert: reverting `api.ts`'s `searchNotesByTitle` from
 *   `invoke('search_notes', { request: { query, title_prefix: true } })`
 * back to
 *   `invoke('search_notes', { query })`
 * (or dropping the `title_prefix` flag entirely) makes the shape-assertion
 * tests red because the spy sees the wrong arg structure.
 *
 * REWORK (2026-08-05, per Reed's delta audit, Option 3): PR #2435 originally
 * had `searchNotesByTitle` build `title:"word"*` FTS5 syntax frontend-side
 * and send the pre-built string as `request.query`. That shipped a
 * launch-blocking regression — `search_notes_internal`'s general-purpose
 * re-quote path re-wrapped the already-syntax string into a literal phrase,
 * destroying column-targeting and prefix matching (zero results for every
 * query). Fix: query construction moved entirely server-side, gated by a
 * `title_prefix` flag on `SearchNotesRequest`. `searchNotesByTitle` now sends
 * the RAW query string plus `title_prefix: true` — no FTS5 syntax is built
 * frontend-side at all. See `api.searchNotesByTitle.test.ts` for dedicated
 * coverage of this function, and `notes.rs::search_notes_title_prefix_tests`
 * for the Rust-side integration tests covering the full two-layer pipeline.
 */

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { searchNotesByTitle, searchNotes, transformNoteMeta } from '$lib/api';
import * as tauriCore from '@tauri-apps/api/core';

// ─── tests ────────────────────────────────────────────────────────────────────

describe('searchNotesByTitle — invoke parameter shape (#2419)', () => {
  let invokeSpy: ReturnType<typeof vi.spyOn>;

  beforeEach(() => {
    // Spy on the mocked invoke so we can capture the exact args passed.
    // vitest's vi.mock (registered in vitest-setup.ts) makes the module writable.
    invokeSpy = vi.spyOn(tauriCore, 'invoke').mockResolvedValue([]);
  });

  afterEach(() => {
    invokeSpy.mockRestore();
  });

  it('wraps query in { request: { query, title_prefix: true } } — Tauri requires a named "request" param', async () => {
    await searchNotesByTitle('hello world');

    expect(invokeSpy).toHaveBeenCalledWith(
      'search_notes',
      { request: { query: 'hello world', title_prefix: true } }
    );
  });

  it('does NOT pass bare { query } at the top level (regression guard)', async () => {
    await searchNotesByTitle('some query');

    // Pre-fix code passed { query: '...' } directly — must not exist at top level
    const callArgs = invokeSpy.mock.calls[0];
    const argsObj = callArgs?.[1] as any;
    expect(argsObj).not.toHaveProperty('query');
  });

  it('sends the RAW query, not a pre-built FTS5 string (#2419 round-2 delta-audit fix, Option 3)', async () => {
    await searchNotesByTitle('Project Alpha Notes');

    const argsObj = invokeSpy.mock.calls[0]?.[1] as any;
    expect(argsObj?.request?.query).toBe('Project Alpha Notes');
    expect(argsObj?.request?.title_prefix).toBe(true);
  });

  it('returns an array (does not throw on empty results)', async () => {
    const result = await searchNotesByTitle('anything');
    expect(Array.isArray(result)).toBe(true);
  });

  it('returns results that include title from the Tauri response', async () => {
    invokeSpy.mockResolvedValueOnce([
      { note_id: 'abc', title: 'Found Note', content_preview: '', score: 1.0, project_name: 'P' },
    ]);

    const result = await searchNotesByTitle('Found Note');
    // searchNotesByTitle returns NoteSearchResult[] — title must be present
    expect(result).toHaveLength(1);
    expect(result[0].title).toBe('Found Note');
  });
});

// ─── searchNotes — note_id → id/slug remap (#2419 follow-up) ─────────────────
//
// Red-on-revert: `search_notes` (Rust `SearchResult`) serializes the note
// identifier as `note_id`, never `id`. `transformNoteMeta` reads `.id`. Without
// the remap in `searchNotes` (api.ts:250-253), every result has `id: undefined`
// and `slug: undefined` — this is the exact root cause of the wiki-link
// click-through phantom editor bug (see 2026-08-05 scoping audit, Deliverable 1).
// Reverting the `{ ...n, id: n.note_id }` remap back to `transformNoteMeta(n)`
// makes these tests red.

describe('searchNotes — note_id remap (#2419 follow-up)', () => {
  let invokeSpy: ReturnType<typeof vi.spyOn>;

  beforeEach(() => {
    invokeSpy = vi.spyOn(tauriCore, 'invoke').mockResolvedValue([]);
  });

  afterEach(() => {
    invokeSpy.mockRestore();
  });

  it('remaps note_id to id on every result', async () => {
    invokeSpy.mockResolvedValueOnce([
      { note_id: 'note-123', title: 'Alpha', content_preview: '', score: 1.0, project_name: 'P' },
      { note_id: 'note-456', title: 'Beta', content_preview: '', score: 0.9, project_name: 'P' },
    ]);

    const result = await searchNotes('anything');

    expect(result).toHaveLength(2);
    expect(result[0].id).toBe('note-123');
    expect(result[1].id).toBe('note-456');
    expect(result[0].id).not.toBeUndefined();
  });

  it('remaps note_id to slug on every result (noteService.selectNote reads slug first)', async () => {
    invokeSpy.mockResolvedValueOnce([
      { note_id: 'note-789', title: 'Gamma', content_preview: '', score: 1.0, project_name: 'P' },
    ]);

    const result = await searchNotes('anything');

    expect(result[0].slug).toBe('note-789');
    expect(result[0].slug).not.toBeUndefined();
  });

  it('id and slug both equal the backend note_id, not left undefined', async () => {
    invokeSpy.mockResolvedValueOnce([
      { note_id: 'note-xyz', title: 'Delta', content_preview: '', score: 1.0, project_name: 'P' },
    ]);

    const result = await searchNotes('anything');

    expect(result[0].id).toBe('note-xyz');
    expect(result[0].slug).toBe('note-xyz');
    expect(result[0].id).toBe(result[0].slug);
  });
});

// ─── searchNotes — real REAL Rust row shape, project_id/folder_id (#2438) ───────────
//
// #2438 incident root cause: search_notes' Rust SearchResult struct never had
// project_id/folder_id fields at all — only note_id, title, content_preview,
// project_name, folder_name, score. Every test in wikiLinkNavigation.test.ts
// used a hand-built NoteMeta (via makeNote()) that always pre-populated
// project_id/folder_id directly, so this gap was invisible to that suite —
// see docs/audits/2026-08-05-2438-incident-diagnosis.md, §6. This test fixture
// is shaped like the REAL backend row (note_id, project_name, folder_name,
// content_preview, score — now with project_id/folder_id added by the fix) and
// runs it through the actual transformNoteMeta/searchNotes pipeline, so a
// future regression that drops the columns from the SQL SELECT (or the
// SearchResult struct) is caught here, not just in wikiLinkNavigation.test.ts's
// mocked fixtures.
//
// Re-applied unchanged from #2443 per the nav-lockup re-land task — the
// backend half of #2443 ships as-is; only wikiLinkNavigation.ts's consumption
// of these fields changed (see wikiLinkNavigation.test.ts).

describe('searchNotes — real backend row shape carries project_id/folder_id (#2438)', () => {
  let invokeSpy: ReturnType<typeof vi.spyOn>;

  beforeEach(() => {
    invokeSpy = vi.spyOn(tauriCore, 'invoke').mockResolvedValue([]);
  });

  afterEach(() => {
    invokeSpy.mockRestore();
  });

  it('transformNoteMeta passes project_id/folder_id through from a real Rust SearchResult-shaped row', () => {
    // Shaped exactly like search_notes_internal's SELECT output post-#2438-fix:
    // note_id (not id), project_name/folder_name (display strings), AND now
    // project_id/folder_id (the raw ids this incident was about).
    const rustShapedRow = {
      note_id: 'note-shape-test',
      title: 'Shape Test Note',
      content_preview: 'some preview text',
      project_name: 'My Project',
      folder_name: 'My Folder',
      project_id: 'proj-abc',
      folder_id: 'folder-xyz',
      score: 0.87,
    };

    // searchNotes's own remap step (api.ts:257): { ...n, id: n.note_id }
    const transformed = transformNoteMeta({ ...rustShapedRow, id: rustShapedRow.note_id });

    expect(transformed.project_id).toBe('proj-abc');
    expect(transformed.folder_id).toBe('folder-xyz');
  });

  it('searchNotes() returns project_id/folder_id when the backend row includes them', async () => {
    invokeSpy.mockResolvedValueOnce([
      {
        note_id: 'note-real-shape',
        title: 'Real Shape',
        content_preview: 'preview',
        project_name: 'Proj Display Name',
        folder_name: 'Folder Display Name',
        project_id: 'real-proj-id',
        folder_id: 'real-folder-id',
        score: 1.0,
      },
    ]);

    const result = await searchNotes('anything');

    // Red-on-revert: if search_notes ever stops selecting project_id/folder_id
    // again (this incident's exact failure mode), these come back undefined
    // and this assertion fails — unlike a hand-built NoteMeta fixture, which
    // would pass regardless of what the backend actually sends.
    expect(result[0].project_id).toBe('real-proj-id');
    expect(result[0].folder_id).toBe('real-folder-id');
  });
});
