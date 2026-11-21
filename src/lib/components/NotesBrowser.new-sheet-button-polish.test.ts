/**
 * #2339 PR 15c — New Sheet button polish (NotesBrowser.svelte)
 *
 * Same delta as KanbanBoard.svelte and AllTasksView.svelte's equivalent tests.
 * NotesBrowser is also the one call site whose class name was inconsistent
 * with the other two before this PR (`.new-sheet-btn` vs `.add-sheet-btn`) —
 * normalized here to `.add-sheet-btn` for consistency across all three host
 * views.
 *
 * Source-text extraction pattern, matching NotesBrowser.collapse-rails.test.ts
 * and NotesBrowser.folder-selection.test.ts's own precedent for this heavy
 * component (requires full Tauri window context to mount).
 *
 * Harness: Vitest (jsdom).
 */

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { readFileSync } from 'fs';
import { resolve } from 'path';
import { render, fireEvent, screen } from '@testing-library/svelte/svelte5';
import { resolveHeaderButtonMode } from '$lib/utils/newSheetButtonLayout';
import NotesBrowser from './NotesBrowser.svelte';
import * as api from '$lib/api';
import { activeProject, projects } from '$lib/stores/app';

const COMPONENT_PATH = resolve(__dirname, 'NotesBrowser.svelte');
const SOURCE = readFileSync(COMPONENT_PATH, 'utf-8');

// vitest-setup.ts replaces `globalThis.window` with a plain mock object (Tauri
// internals shim) lacking addEventListener/removeEventListener/ResizeObserver.
// NotesBrowser registers several window listeners and a ResizeObserver in
// onMount, so both need stubs for mount/unmount to succeed (same pattern as
// ChatSidebar's echoSuppression test suite / AllTasksView + KanbanBoard's
// polish tests).
if (typeof (globalThis.window as any).addEventListener !== 'function') {
  (globalThis.window as any).addEventListener = () => {};
}
if (typeof (globalThis.window as any).removeEventListener !== 'function') {
  (globalThis.window as any).removeEventListener = () => {};
}

vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(() => Promise.resolve(() => {})),
}));

function extractCssRule(css: string, selector: string): string {
  const selectorIndex = css.indexOf(`${selector} {`);
  if (selectorIndex === -1) {
    throw new Error(`Selector "${selector}" not found`);
  }
  const openBraceIndex = css.indexOf('{', selectorIndex);
  let depth = 0;
  let i = openBraceIndex;
  for (; i < css.length; i++) {
    if (css[i] === '{') depth++;
    if (css[i] === '}') {
      depth--;
      if (depth === 0) break;
    }
  }
  return css.slice(selectorIndex, i + 1);
}

describe('NotesBrowser New Sheet button polish — #2339 PR 15c', () => {
  // ── Class rename for consistency ────────────────────────────────────────

  it('the New Sheet button class is renamed from .new-sheet-btn to .add-sheet-btn (matches Kanban/AllTasksView naming)', () => {
    expect(SOURCE).not.toMatch(/class="new-sheet-btn"/);
    expect(SOURCE).toMatch(/class="add-sheet-btn btn--outline"/);
  });

  // ── Glyph swap ──────────────────────────────────────────────────────────

  it('New Sheet button carries the same plus glyph New Note uses (both share the identical path)', () => {
    const plusGlyph = '<path d="M7 2V12M2 7H12" stroke-width="1.5" stroke-linecap="round"/>';
    const occurrences = SOURCE.split(plusGlyph).length - 1;
    // New Note (full) + New Sheet (full) + New Sheet (caret menu row) = at least 3
    expect(occurrences).toBeGreaterThanOrEqual(3);
  });

  it('the old grid-glyph SVG is gone', () => {
    expect(SOURCE).not.toContain('rx="1.2" stroke-width="1.3"');
    expect(SOURCE).not.toContain('M1.5 5.2H12.5M1.5 8.8H12.5M5.2 1.5V12.5M8.8 1.5V12.5');
  });

  // ── .btn--outline, not .btn--ghost ─────────────────────────────────────

  it('never applies .btn--ghost anywhere in this component', () => {
    expect(SOURCE).not.toMatch(/btn--ghost/);
  });

  it('local .add-sheet-btn CSS no longer hardcodes its own fill (delegated to the global .btn--outline class)', () => {
    const rule = extractCssRule(SOURCE, '.add-sheet-btn');
    expect(rule).not.toMatch(/background:\s*transparent/);
    expect(rule).not.toMatch(/color:\s*#00C8C8/);
  });

  // ── Positioning: New Sheet left of New Note, New Note stays rightmost ──

  it('New Sheet button markup appears BEFORE New Note in DOM order', () => {
    const actionsStart = SOURCE.indexOf('class="grid-folder-header-actions"');
    const sheetIndex = SOURCE.indexOf('class="add-sheet-btn btn--outline"', actionsStart);
    const noteIndex = SOURCE.indexOf('class="new-note-btn"', actionsStart);
    expect(sheetIndex).toBeGreaterThan(-1);
    expect(noteIndex).toBeGreaterThan(-1);
    expect(sheetIndex).toBeLessThan(noteIndex);
  });

  it('New Note (.new-note-btn) in the folder header keeps its label unconditionally at every width', () => {
    const actionsStart = SOURCE.indexOf('class="grid-folder-header-actions"');
    const noteIndex = SOURCE.indexOf('class="new-note-btn"', actionsStart);
    const noteBlockEnd = SOURCE.indexOf('</button>', noteIndex);
    const noteBlock = SOURCE.slice(noteIndex, noteBlockEnd);
    expect(noteBlock).toContain('New Note');
    expect(noteBlock).not.toMatch(/\{#if/);
  });

  // ── Three-step responsive collapse ─────────────────────────────────────

  it('imports resolveHeaderButtonMode from the shared pure-utility module', () => {
    expect(SOURCE).toMatch(/import\s+\{\s*resolveHeaderButtonMode,\s*type HeaderButtonMode\s*\}\s+from\s+'\$lib\/utils\/newSheetButtonLayout'/);
  });

  it('derives headerButtonMode reactively from folderHeaderWidth via $derived', () => {
    expect(SOURCE).toMatch(/let headerButtonMode: HeaderButtonMode = \$derived\(resolveHeaderButtonMode\(folderHeaderWidth\)\)/);
  });

  it('attaches a ResizeObserver to the header element in onMount and disconnects it in onDestroy', () => {
    expect(SOURCE).toMatch(/new ResizeObserver\(entries => \{/);
    expect(SOURCE).toMatch(/headerResizeObs\.observe\(folderHeaderEl\)/);
    expect(SOURCE).toMatch(/onDestroy\(\(\) => \{\s*headerResizeObs\?\.disconnect\(\);\s*\}\);/);
  });

  it('the .grid-folder-header element carries the bind:this ref the ResizeObserver observes', () => {
    expect(SOURCE).toMatch(/<div class="grid-folder-header" bind:this=\{folderHeaderEl\}>/);
  });

  it('New Sheet label is conditionally rendered only in "full" mode', () => {
    expect(SOURCE).toMatch(/\{#if headerButtonMode === 'full'\}New Sheet\{\/if\}/);
  });

  it('the icon-only CSS class toggles via class:add-sheet-btn--icon-only bound to headerButtonMode === \'icon\'', () => {
    expect(SOURCE).toMatch(/class:add-sheet-btn--icon-only=\{headerButtonMode === 'icon'\}/);
  });

  // ── Caret menu (header width < 720px) ──────────────────────────────────

  it('renders the caret button only when headerButtonMode === "menu"', () => {
    expect(SOURCE).toMatch(/\{#if headerButtonMode === 'menu'\}\s*<div class="sheet-caret-wrap">/);
  });

  it('the caret sits beside New Note (New Note renders immediately before the caret wrap in "menu" mode)', () => {
    const actionsStart = SOURCE.indexOf('class="grid-folder-header-actions"');
    const noteIndex = SOURCE.indexOf('class="new-note-btn"', actionsStart);
    const caretIndex = SOURCE.indexOf('class="sheet-caret-wrap"', actionsStart);
    expect(noteIndex).toBeGreaterThan(-1);
    expect(caretIndex).toBeGreaterThan(-1);
    expect(noteIndex).toBeLessThan(caretIndex);
  });

  it('handleNewSheetFromCaret closes the caret and delegates to the real handleNewSheet (no duplicated creation logic)', () => {
    expect(SOURCE).toMatch(/function handleNewSheetFromCaret\(\) \{\s*sheetCaretOpen = false;\s*handleNewSheet\(\);\s*\}/);
  });

  // ── Existing creation behavior preserved ────────────────────────────

  it('the full/icon-mode New Sheet button is still wired to handleNewSheet (existing create-sheet behavior untouched)', () => {
    const btnStart = SOURCE.indexOf('class="add-sheet-btn btn--outline"');
    const btnBlockEnd = SOURCE.indexOf('</button>', btnStart);
    const btnBlock = SOURCE.slice(SOURCE.lastIndexOf('<button', btnStart), btnBlockEnd);
    expect(btnBlock).toMatch(/onclick=\{handleNewSheet\}/);
    expect(btnBlock).toMatch(/disabled=\{isCreatingSheet\}/);
  });

  it('the caret\'s New Sheet row is ALSO wired through to handleNewSheet (via handleNewSheetFromCaret, same code path)', () => {
    expect(SOURCE).toMatch(/onclick=\{handleNewSheetFromCaret\} disabled=\{isCreatingSheet\}/);
  });

  it('handleNewSheet itself is untouched by this PR \u2014 still calls api.createSheet with folder-resolution logic mirroring handleNewNote', () => {
    expect(SOURCE).toMatch(/async function handleNewSheet\(\) \{/);
    expect(SOURCE).toMatch(/const sheetData = await api\.createSheet\(\{/);
  });

  // ── resolveHeaderButtonMode integration sanity ─────────────────────────

  it('the imported resolveHeaderButtonMode resolves the exact three modes this component branches on', () => {
    expect(resolveHeaderButtonMode(1000)).toBe('full');
    expect(resolveHeaderButtonMode(800)).toBe('icon');
    expect(resolveHeaderButtonMode(500)).toBe('menu');
  });

  // ── Caret menu down to one row after New Folder removal (#2874 item 6) ──

  it('collapsed caret dropdown renders exactly one row: New Sheet', async () => {
    (globalThis as any).ResizeObserver = class {
      constructor(private cb: ResizeObserverCallback) {}
      observe() {
        this.cb([{ contentRect: { width: 400 } }] as any, this as any);
      }
      unobserve() {}
      disconnect() {}
    };
    activeProject.set({
      id: 'project-1',
      name: 'Project One',
      parent_id: null,
      created_at: '',
      updated_at: '',
    } as any);

    render(NotesBrowser, { props: {} });

    const caretButtons = Array.from(document.querySelectorAll('.sheet-caret-btn'));
    expect(caretButtons.length).toBeGreaterThan(0);
    await fireEvent.click(caretButtons[0] as HTMLElement);

    const items = Array.from(document.querySelectorAll('.sheet-caret-item'));
    expect(items.length).toBe(1);
    expect(items[0].textContent).toContain('New Sheet');

    activeProject.set(null);
  });
});

