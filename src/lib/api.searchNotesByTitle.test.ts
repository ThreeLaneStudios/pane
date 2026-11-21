/**
 * api.searchNotesByTitle.test.ts — #2419 round-2 Finding 2 Part A (launch-blocking)
 *
 * REWORK (2026-08-05, per Reed's delta audit `2026-08-05-2434-2435-delta-audit.md`,
 * Option 3): the original PR #2435 built `title:"word"*` FTS5 syntax
 * FRONTEND-side in `buildTitlePrefixQuery`, then sent the pre-built string as
 * `request.query`. That shipped a launch-blocking regression:
 * `search_notes_internal`'s general-purpose re-quote path (notes.rs) doesn't
 * know how to leave already-FTS5-syntax strings alone — it unconditionally
 * wraps every incoming query in an extra layer of quoting for injection-safety,
 * turning `title:"De"*` into the literal phrase `"title:De*"`. That destroys
 * both column-targeting and prefix matching — EVERY wiki-link-picker query
 * returned zero results, for every query length, including full exact words.
 *
 * Fix (this file): query construction moves entirely server-side. This
 * frontend function now sends the RAW query string plus a `title_prefix: true`
 * flag on the Tauri request. `buildTitlePrefixQuery` no longer exists — no
 * FTS5 syntax is ever built frontend-side again. The full two-layer pipeline
 * (frontend query → `search_notes_internal`'s title-prefix branch → real FTS5
 * MATCH) is now covered by Rust integration tests in
 * `notes.rs::search_notes_title_prefix_tests` (rusqlite, real FTS5 table,
 * production schema) — that is the coverage gap that let the original bug
 * ship, since this file's old Section 2 tested `buildTitlePrefixQuery`'s
 * output directly against a bare `notes_fts` table, bypassing
 * `search_notes_internal`'s re-quote step entirely.
 *
 * This file now only asserts the frontend HALF of the contract: the invoke
 * call shape (raw query + `title_prefix: true`), short-circuit behavior for
 * empty queries, and that `buildTitlePrefixQuery` no longer exists as an
 * export (import failure = red-on-revert for the old frontend-syntax-building
 * approach).
 *
 * RED-ON-REVERT: reverting `searchNotesByTitle` to NOT send `title_prefix`
 * (e.g. back to a bare `{ query }` passthrough, or back to building FTS5
 * syntax frontend-side) makes every invoke-shape test in this file red.
 *
 * Scope guards: does not touch QuickSwitcher, api.searchNotes, or notes.rs's
 * WHERE clause — searchNotesByTitle has exactly one consumer (WikiLink.ts:36).
 *
 * Relates to #2419
 * spec-skip: implements Matt's picker-semantics ruling 2026-08-05, fix shape
 * per Reed delta audit option 3, notes.rs change is query construction only,
 * no schema change, no migration
 */

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { searchNotesByTitle } from '$lib/api';
import * as tauriCore from '@tauri-apps/api/core';

describe('searchNotesByTitle — sends raw query + title_prefix flag (Option 3 rework)', () => {
  let invokeSpy: ReturnType<typeof vi.spyOn>;

  beforeEach(() => {
    invokeSpy = vi.spyOn(tauriCore, 'invoke').mockResolvedValue([]);
  });

  afterEach(() => {
    invokeSpy.mockRestore();
  });

  it('sends the RAW query string (no FTS5 syntax) plus title_prefix: true', async () => {
    await searchNotesByTitle('De');
    expect(invokeSpy).toHaveBeenCalledWith('search_notes', {
      request: { query: 'De', title_prefix: true },
    });
  });

  it('does NOT build any title: or * FTS5 syntax frontend-side for multi-word queries', async () => {
    await searchNotesByTitle('Define Target');
    expect(invokeSpy).toHaveBeenCalledWith('search_notes', {
      request: { query: 'Define Target', title_prefix: true },
    });
  });

  it('does NOT pass a pre-built FTS5-syntax string (regression guard for the #2435-as-shipped bug)', async () => {
    await searchNotesByTitle('Define Target');
    const argsObj = invokeSpy.mock.calls[0]?.[1] as any;
    expect(argsObj?.request?.query).not.toContain('title:');
    expect(argsObj?.request?.query).not.toContain('"');
    expect(argsObj?.request?.query).not.toContain('*');
    expect(argsObj?.request?.query).toBe('Define Target');
  });

  it('always sets title_prefix: true (never false/absent) for this consumer', async () => {
    await searchNotesByTitle('anything');
    const argsObj = invokeSpy.mock.calls[0]?.[1] as any;
    expect(argsObj?.request?.title_prefix).toBe(true);
  });

  it('short-circuits to invoke() not being called at all for an empty query', async () => {
    const result = await searchNotesByTitle('');
    expect(invokeSpy).not.toHaveBeenCalled();
    expect(result).toEqual([]);
  });

  it('short-circuits for a whitespace-only query', async () => {
    const result = await searchNotesByTitle('   ');
    expect(invokeSpy).not.toHaveBeenCalled();
    expect(result).toEqual([]);
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
    expect(result).toHaveLength(1);
    expect(result[0].title).toBe('Found Note');
  });
});

describe('buildTitlePrefixQuery no longer exists (Option 3 rework — server-side construction only)', () => {
  it('the module has no buildTitlePrefixQuery export', async () => {
    const apiModule = await import('$lib/api');
    expect((apiModule as any).buildTitlePrefixQuery).toBeUndefined();
  });
});
