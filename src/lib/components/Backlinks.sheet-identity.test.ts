/**
 * Backlinks.sheet-identity.test.ts — #2874 item 15 regression coverage.
 *
 * Backlinks.svelte itself never touches note_type or a NoteMeta object at
 * all — it forwards only a bare `title: string` from `handleBacklinkClick`
 * to its `onNavigate` prop. That prop is wired everywhere it's used
 * (Editor.svelte:2632 -> handleNoteNavigation, which just forwards to the
 * Editor's own `onNavigate` prop, itself wired to
 * KanbanBoard/AllTasksView/NotesBrowser's wiki-link handlers) straight into
 * `navigateWikiLink()` (src/lib/utils/wikiLinkNavigation.ts), which is
 * already covered end-to-end for sheet-identity preservation by
 * wikiLinkNavigation.test.ts ("preserves note_type through selectNoteFn").
 *
 * This file's job is narrower: pin that Backlinks.svelte's OWN click
 * handler is a pure pass-through with no meta-object construction anywhere
 * in it — i.e. it structurally CANNOT be a "hand-built meta object" site
 * (the bug class this whole PR fixes), because it never builds one.
 *
 * Real component render (Backlinks.svelte is tiny — 111 lines, mocking only
 * `$lib/api` is sufficient, no Tauri window/store dependencies).
 */

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, cleanup, fireEvent, screen } from '@testing-library/svelte/svelte5';
import Backlinks from './Backlinks.svelte';
import * as api from '$lib/api';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe('Backlinks — click handler is a pure title pass-through (item 15)', () => {
  it('calls onNavigate with the exact backlink title string, and nothing else', async () => {
    vi.spyOn(api, 'getBacklinks').mockResolvedValue([
      { id: 'note-a', title: 'Linked Sheet Note', project: 'Project One' },
    ]);
    const onNavigate = vi.fn();

    render(Backlinks, { props: { noteId: 'note-target', onNavigate } });

    const item = await screen.findByText('Linked Sheet Note');
    await fireEvent.click(item.closest('button')!);

    expect(onNavigate).toHaveBeenCalledTimes(1);
    expect(onNavigate).toHaveBeenCalledWith('Linked Sheet Note');
  });

  it('renders backlinks for sheet-typed source notes exactly like any other note — api.getBacklinks response shape has no note_type field to lose', async () => {
    // api.getBacklinks returns { id, title, project } only (Backlinks.svelte's
    // own type annotation, line 11) — there is no note_type in this response
    // at all, so there is nothing for Backlinks.svelte to drop. Confirms this
    // component structurally cannot be an identity-loss site.
    vi.spyOn(api, 'getBacklinks').mockResolvedValue([
      { id: 'sheet-note-1', title: 'A Sheet That Links Here', project: 'Finance' },
    ]);

    render(Backlinks, { props: { noteId: 'note-target', onNavigate: vi.fn() } });

    expect(await screen.findByText('A Sheet That Links Here')).toBeTruthy();
    expect(screen.getByText('Finance')).toBeTruthy();
  });

  it('handleBacklinkClick is a one-line forward — no meta object of any kind is constructed in Backlinks.svelte', async () => {
    const { readFileSync } = await import('fs');
    const { resolve } = await import('path');
    const source = readFileSync(resolve(__dirname, 'Backlinks.svelte'), 'utf-8');
    const fnMatch = source.match(/function handleBacklinkClick\(title: string\) \{[\s\S]*?\n  \}/);
    expect(fnMatch).toBeTruthy();
    expect(fnMatch![0]).not.toMatch(/meta:\s*\{/);
    expect(fnMatch![0]).toMatch(/onNavigate\(title\)/);
  });
});
