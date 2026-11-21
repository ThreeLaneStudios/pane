/**
 * Tests for wikiLinkNavigation.ts — #2419, #3044.
 *
 * #3044 ruling (Matt): a `[[Title]]` wiki link to a note that does NOT
 * exist stays inert — no create, no confirm dialog, nothing happens.
 * `navigateWikiLink` no longer accepts `projectId`/`folderId`/`createNoteFn`/
 * `confirmFn` at all — the create/confirm path is deleted outright, not
 * merely gated.
 *
 * Red-on-revert contract:
 *
 * 1. Exact match → navigate (selectNote). Reverting the component wiring
 *    (back to () => {}) doesn't make this test red — but deleting/breaking
 *    navigateWikiLink does. The component test confirms the function is
 *    called; these tests confirm the function does the right thing.
 *
 * 2. No match → do nothing at all. No searchFn is called again, no
 *    selectNoteFn, no throw, no dialog. This is the new #3044 contract
 *    replacing the old create-on-miss branch.
 */

import { describe, it, expect, vi } from 'vitest';
import { navigateWikiLink } from '$lib/utils/wikiLinkNavigation';
import type { NoteMeta } from '$lib/types';

// ─── helpers ──────────────────────────────────────────────────────────────────

function makeNote(overrides: Partial<NoteMeta> = {}): NoteMeta {
  return {
    id: 'note-' + Math.random().toString(36).slice(2),
    title: 'Test Note',
    slug: 'test-note',
    notebook: 'TestProject',
    preview: '',
    content_preview: '',
    created_at: new Date().toISOString(),
    updated_at: new Date().toISOString(),
    pinned: false,
    word_count: 0,
    tags: [],
    project_id: 'project-1',
    folder_id: 'folder-1',
    file_path: undefined,
    task_status: undefined,
    task_priority: undefined,
    task_due_date: undefined,
    task_assignee: undefined,
    task_completed_at: undefined,
    task_sort_order: 0,
    note_type: undefined,
    ...overrides,
  };
}

function makeOpts(overrides: Partial<Parameters<typeof navigateWikiLink>[1]> = {}) {
  return {
    searchFn: vi.fn<(query: string) => Promise<NoteMeta[]>>().mockResolvedValue([]),
    selectNoteFn: vi
      .fn<(note: NoteMeta, opts: { projectId?: string; folderId?: string } | undefined) => Promise<void>>()
      .mockResolvedValue(undefined),
    getNoteRawFn: vi.fn<(id: string) => Promise<NoteMeta | null>>().mockResolvedValue(null),
    ...overrides,
  };
}

// ─── tests ────────────────────────────────────────────────────────────────────

describe('navigateWikiLink', () => {
  // ── exact match: navigate ─────────────────────────────────────────────────

  it('calls selectNote with the exact-title match', async () => {
    const matchNote = makeNote({ title: 'My Note' });
    const opts = makeOpts({
      searchFn: vi.fn().mockResolvedValue([
        makeNote({ title: 'Other Note' }),
        matchNote,
        makeNote({ title: 'Another' }),
      ]),
    });

    await navigateWikiLink('My Note', opts);

    expect(opts.selectNoteFn).toHaveBeenCalledOnce();
    expect(opts.selectNoteFn).toHaveBeenCalledWith(
      matchNote,
      { projectId: matchNote.project_id, folderId: matchNote.folder_id },
    );
  });

  it('exact-match: selectNoteFn is called with an object carrying a defined slug', async () => {
    const matchNote = makeNote({ title: 'Slug Check', slug: 'slug-check-id' });
    const opts = makeOpts({
      searchFn: vi.fn().mockResolvedValue([matchNote]),
    });

    await navigateWikiLink('Slug Check', opts);

    const [passedNote] = (opts.selectNoteFn as ReturnType<typeof vi.fn>).mock.calls[0];
    expect(passedNote.slug).toBeDefined();
    expect(passedNote.slug).toBe('slug-check-id');
  });

  it('exact match is case-sensitive — "my note" does not match "My Note", and the miss is inert', async () => {
    const matchNote = makeNote({ title: 'My Note' });
    const opts = makeOpts({
      searchFn: vi.fn().mockResolvedValue([matchNote]),
    });

    await navigateWikiLink('my note', opts);

    // #3044: no exact-case match → inert. No create, no selectNote.
    expect(opts.selectNoteFn).not.toHaveBeenCalled();
  });

  // ── #2443 re-land (nav-lockup diagnosis §7 option 2 — single-writer): exact match writes
  // canvasState via ONE selectNoteFn call, no separate navigateFn ───────────────────
  it('#2443 re-land: exact match calls selectNoteFn exactly ONCE, with the target note AND a contextOverride carrying its project_id/folder_id', async () => {
    const matchNote = makeNote({
      title: 'Target Note',
      id: 'target-note',
      project_id: 'target-proj',
      folder_id: 'target-folder',
    });
    const opts = makeOpts({
      searchFn: vi.fn().mockResolvedValue([matchNote]),
    });

    await navigateWikiLink('Target Note', opts);

    expect(opts.selectNoteFn).toHaveBeenCalledOnce();
    expect(opts.selectNoteFn).toHaveBeenCalledWith(
      matchNote,
      { projectId: 'target-proj', folderId: 'target-folder' },
    );
  });

  // ── #2438 incident: getNoteRawFn fallback when search result lacks project_id ──
  it('#2438 fallback: resolves project_id/folder_id via getNoteRawFn when search result is missing them (old backend shape)', async () => {
    const staleMatch = makeNote({
      title: 'Target Note',
      id: 'target-note',
      project_id: undefined,
      folder_id: undefined,
    });
    const opts = makeOpts({
      searchFn: vi.fn().mockResolvedValue([staleMatch]),
      getNoteRawFn: vi.fn().mockResolvedValue(
        makeNote({ id: 'target-note', project_id: 'resolved-proj', folder_id: 'resolved-folder' })
      ),
    });

    await navigateWikiLink('Target Note', opts);

    expect(opts.getNoteRawFn).toHaveBeenCalledWith('target-note');
    expect(opts.selectNoteFn).toHaveBeenCalledWith(
      expect.objectContaining({ id: 'target-note', project_id: 'resolved-proj', folder_id: 'resolved-folder' }),
      { projectId: 'resolved-proj', folderId: 'resolved-folder' },
    );
  });

  it('#2438 fallback: getNoteRawFn is NOT called when the search result already has project_id (new backend shape, happy path)', async () => {
    const freshMatch = makeNote({
      title: 'Target Note',
      id: 'target-note',
      project_id: 'search-proj',
      folder_id: 'search-folder',
    });
    const opts = makeOpts({
      searchFn: vi.fn().mockResolvedValue([freshMatch]),
    });

    await navigateWikiLink('Target Note', opts);

    expect(opts.getNoteRawFn).not.toHaveBeenCalled();
    expect(opts.selectNoteFn).toHaveBeenCalledWith(
      freshMatch,
      { projectId: 'search-proj', folderId: 'search-folder' },
    );
  });

  // ── #3044: missing note is inert — typing and reopen share this one function ──

  it('#3044: no match → does nothing. selectNoteFn is never called, nothing is created, no error thrown', async () => {
    const opts = makeOpts({
      searchFn: vi.fn().mockResolvedValue([]),
    });

    await expect(navigateWikiLink('A Note That Does Not Exist', opts)).resolves.toBeUndefined();

    expect(opts.selectNoteFn).not.toHaveBeenCalled();
    expect(opts.getNoteRawFn).not.toHaveBeenCalled();
  });

  it('#3044: no match among several non-exact results → still inert', async () => {
    const opts = makeOpts({
      searchFn: vi.fn().mockResolvedValue([
        makeNote({ title: 'Something Else' }),
        makeNote({ title: 'Partially Matching Title' }),
      ]),
    });

    await navigateWikiLink('Missing Note', opts);

    expect(opts.selectNoteFn).not.toHaveBeenCalled();
  });

  // ── searchFn receives the right title ─────────────────────────────────────────

  it('passes the wiki-link title to searchFn', async () => {
    const opts = makeOpts({
      searchFn: vi.fn().mockResolvedValue([]),
    });

    await navigateWikiLink('Specific Title', opts);

    expect(opts.searchFn).toHaveBeenCalledWith('Specific Title');
  });
});
